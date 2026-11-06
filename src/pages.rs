use crate::{
    ApiError, AppState,
    cache::{ASSETS, etag, fingerprinted, revalidate},
    requests::{MAX_QUERY_CHARS, QUERY_TOO_LONG},
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) const DICTIONARY_PAGE: &str = include_str!("../web/index.html");
pub(crate) const DICTIONARY_BODY: &str = "<body class=\"dictionary\"";

pub(crate) async fn start_script(headers: HeaderMap) -> Response {
    const SCRIPT: &str = include_str!("../web/start.js");
    revalidate(
        &headers,
        etag(SCRIPT.as_bytes()),
        "no-cache",
        ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], SCRIPT),
    )
}

fn error_page(state: &AppState, status: StatusCode, heading: &str, message: &str) -> Response {
    let page = state
        .not_found_page
        .replace("Page not found", heading)
        .replace(
            "This link may be out of date. Search for a word instead.",
            message,
        );
    (status, Html(page)).into_response()
}

#[derive(Deserialize)]
pub(crate) struct PageQuery {
    q: Option<String>,
}

pub(crate) async fn index_html(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    params: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid link",
            "This link could not be read. Search for a word instead.",
        );
    };
    if params
        .q
        .as_ref()
        .is_some_and(|q| q.chars().count() > MAX_QUERY_CHARS)
    {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Search too long",
            &format!("{}. Try a shorter search.", *QUERY_TOO_LONG),
        );
    }
    let page = match params.q.as_deref().map(str::trim) {
        Some(q) if !q.is_empty() => search_page(&state.dictionary_page, q),
        Some(_) => state.dictionary_page.clone(),
        None => state.home_page.clone(),
    };
    let instant = state.instant.get().map_or("", |instant| &instant.path);
    let page = page.replacen(
        "data-instant=\"\" data-revision=\"\"",
        &format!(
            "data-instant=\"{instant}\" data-revision=\"{}\"",
            state.revision
        ),
        1,
    );
    revalidate(&headers, etag(page.as_bytes()), "no-cache", Html(page))
}

pub(crate) const SITE_TITLE: &str = "avkean 词典 · Chinese–English dictionary";

fn search_page(page: &str, query: &str) -> String {
    let address: String = query
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    page.replace(SITE_TITLE, &format!("{} · avkean 词典", escape_html(query)))
        .replace(
            "<link rel=\"canonical\" href=\"/\">",
            &format!("<link rel=\"canonical\" href=\"/?q={address}\">"),
        )
}

pub(crate) fn page_html(template: &str, feedback_url: Option<&str>, words: usize) -> String {
    let feedback = feedback_url.map_or_else(String::new, |url| {
        format!(
            "<a href=\"{}\" rel=\"noopener noreferrer\">Feedback</a>",
            escape_html(url)
        )
    });
    let digits = words.to_string();
    let mut count = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            count.push(',');
        }
        count.push(digit);
    }
    fingerprinted(
        &template
            .replace("<!-- feedback -->", &feedback)
            .replace("<!-- words -->", &format!("<span>{count} words</span>"))
            .replace("MAX_QUERY_CHARS", &MAX_QUERY_CHARS.to_string()),
        &ASSETS,
    )
}

pub(crate) async fn not_found(State(state): State<Arc<AppState>>, request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        ApiError(StatusCode::NOT_FOUND, "Not found.").into_response()
    } else {
        (StatusCode::NOT_FOUND, Html(state.not_found_page.clone())).into_response()
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) const SOURCE_NAMES: [(&str, &str); 3] = [
    ("CC", "CC-CEDICT"),
    ("WIKT", "Wiktionary"),
    ("CORE", "avkean 词典"),
];

pub(crate) const NOTICE: &str = "NOTICE";

fn notice_parts(line: &str) -> Vec<(&str, Option<&str>)> {
    let mut parts = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|at| open + at) else {
            break;
        };
        let Some(end) = rest[mid..].find(')').map(|at| mid + at) else {
            break;
        };
        parts.push((&rest[..open], None));
        parts.push((&rest[open + 1..mid], Some(&rest[mid + 2..end])));
        rest = &rest[end + 1..];
    }
    parts.push((rest, None));
    parts
}

// Hyphens in NOTICE text don't break lines, so "CC BY-SA" and "LGPL-3.0" stay whole.
fn notice_html(line: &str) -> String {
    notice_parts(line)
        .into_iter()
        .map(|(text, link)| (text.replace('-', "\u{2011}"), link))
        .map(|(text, link)| match link {
            Some(url) if url.starts_with('/') => {
                format!(
                    "<a href=\"{}\">{}</a>",
                    escape_html(url),
                    escape_html(&text)
                )
            }
            Some(url) => format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                escape_html(url),
                escape_html(&text)
            ),
            None => escape_html(&text),
        })
        .collect()
}

// One row per source: its name, then its licence or attribution note in small text.
fn source_row(name: &str, note: &str) -> String {
    let note = if note.is_empty() {
        String::new()
    } else {
        format!("<small>{}</small>", notice_html(note))
    };
    format!("<li>{}{note}</li>", notice_html(name))
}

pub(crate) fn sources_page(licenses: &BTreeMap<String, String>) -> String {
    let mut sources = String::new();
    let mut about = String::new();
    let mut notes = String::new();
    let mut credits = String::new();
    let mut in_notes = false;
    for line in licenses.get(NOTICE).map_or("", String::as_str).lines() {
        if line == "Licence texts" {
            in_notes = true;
        } else if line.trim().is_empty() {
            continue;
        } else if in_notes {
            let (name, note) = line.split_once(": ").unwrap_or((line, ""));
            notes += &source_row(name, note);
        } else if let Some(source) = line.strip_prefix("- ") {
            let (name, licence) = source.split_once(" | ").unwrap_or((source, ""));
            sources += &source_row(name, licence);
        } else if sources.is_empty() {
            about += &format!("<p>{}</p>", notice_html(line));
        } else {
            // a line after the sources is the credit and changes note
            credits += &format!("<p class=\"source-note\">{}</p>", notice_html(line));
        }
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\"><meta name=\"description\" content=\"Data sources, attribution and licences for avkean 词典.\"><title>Sources · avkean 词典</title><link rel=\"icon\" href=\"/favicon.ico\" sizes=\"32x32\"><link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\"><script src=\"/theme.js\"></script><link rel=\"stylesheet\" href=\"/style.css\"></head><body class=\"page\"><main class=\"page-content\"><a class=\"page-back\" href=\"/\"><svg aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m15 5-7 7 7 7\"/></svg><span>avkean <span lang=\"zh\">词典</span></span></a><h1>Sources</h1>{about}<ul class=\"source-list\">{sources}</ul>{credits}<details class=\"notices\"><summary>Licence texts<svg aria-hidden=\"true\" viewBox=\"0 0 24 24\"><path d=\"m9 5 7 7-7 7\"/></svg></summary><ul class=\"source-list disclosure-body\">{notes}</ul></details></main></body></html>"
    )
}

pub(crate) async fn licenses_page(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(state.sources_page.clone())
}

pub(crate) async fn license_text(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let text = state
        .licenses
        .get(&filename)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Licence not found."))?
        .clone();
    Ok(([(CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}
