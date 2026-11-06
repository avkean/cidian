use crate::{ApiError, AppState, requests::unix_seconds, search::SearchQuery};
use axum::{
    body::Bytes,
    extract::{Path, Request, State},
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{self, CONTENT_TYPE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    path::Path as FilePath,
    sync::{Arc, LazyLock},
};

#[derive(Default)]
pub(crate) struct SearchCache {
    pub(crate) responses: HashMap<SearchQuery, (Bytes, u64)>,
    pub(crate) used: BTreeMap<u64, SearchQuery>,
    clock: u64,
}

pub(crate) const SEARCH_CACHE_CAPACITY: usize = 2048;

impl SearchCache {
    pub(crate) fn get(&mut self, query: &SearchQuery) -> Option<Bytes> {
        let (body, used) = self.responses.get_mut(query)?;
        self.clock += 1;
        let query = self.used.remove(used).expect("Every response has a use");
        *used = self.clock;
        self.used.insert(self.clock, query);
        Some(body.clone())
    }

    pub(crate) fn insert(&mut self, query: SearchQuery, body: Bytes) {
        if self.responses.contains_key(&query) {
            return;
        }
        if self.responses.len() == SEARCH_CACHE_CAPACITY
            && let Some((_, oldest)) = self.used.pop_first()
        {
            self.responses.remove(&oldest);
        }
        self.clock += 1;
        self.used.insert(self.clock, query.clone());
        self.responses.insert(query, (body, self.clock));
    }
}

/// every file whose contents change answers fixes the revision: served data, this binary (which holds
/// the baked-in tables and pages), and the licence texts
pub(crate) fn revision(paths: &[&FilePath]) -> std::io::Result<String> {
    use std::io::Read;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = vec![0; 1 << 20];
    for path in paths {
        let mut file = std::fs::File::open(path)?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.write(&buffer[..read]);
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

pub(crate) fn request_limit_exempt(state: &AppState, path: &str) -> bool {
    match path {
        // Health checks must work even when a visitor has used their allowance.
        "/api/health" => true,
        // The startup handler lets old pages recover after a deploy.
        "/start.js" => true,
        // Fingerprinted assets load the page's scripts, styles and icons.
        _ if ASSETS.iter().any(|asset| asset.fingerprinted == path) => true,
        // Page startup needs the current instant data file.
        _ if state
            .instant
            .get()
            .is_some_and(|instant| instant.path == path) =>
        {
            true
        }
        _ => false,
    }
}

pub(crate) async fn response_headers(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let client = crate::requests::ClientIdentity::from_headers(request.headers());
    let limited = !request_limit_exempt(&state, &path);
    let wait = limited
        .then(|| state.visitors.over_limit(request.headers(), unix_seconds()))
        .flatten();
    let mut response = if let Some(wait) = wait {
        let mut response = ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests. Wait a minute and try again.",
        )
        .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(wait));
        response
    } else if request.uri().to_string().len() > 8192 {
        ApiError(StatusCode::URI_TOO_LONG, "Request URL is too long.").into_response()
    } else {
        next.run(request).await
    };
    state.stats.count(&path, response.status());
    let busy = matches!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
    );
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if busy {
        headers.entry(header::RETRY_AFTER).or_insert_with(|| {
            HeaderValue::from(
                state
                    .visitors
                    .computation_wait(client, unix_seconds())
                    .unwrap_or(1),
            )
        });
    }
    if let Ok(revision) = HeaderValue::from_str(&state.revision)
        && !state.revision.is_empty()
    {
        headers.insert(REVISION_HEADER, revision);
    }
    response
}

pub(crate) const REVISION_HEADER: &str = "x-dictionary-revision";

pub(crate) struct Asset {
    pub(crate) path: String,
    pub(crate) fingerprinted: String,
    pub(crate) bytes: Bytes,
    pub(crate) content_type: &'static str,
    etag: HeaderValue,
}

impl Asset {
    fn new(name: &str, bytes: Bytes, content_type: &'static str) -> Self {
        let hash = content_hash(&bytes);
        let (stem, extension) = name.rsplit_once('.').expect("Assets have an extension");
        Self {
            path: format!("/{name}"),
            fingerprinted: format!("/{stem}.{hash:016x}.{extension}"),
            content_type,
            etag: etag(&bytes),
            bytes,
        }
    }

    fn module(name: &str, source: &str, dependencies: &[Asset]) -> Self {
        Self::new(
            name,
            fingerprinted(source, dependencies).into(),
            "text/javascript; charset=utf-8",
        )
    }

    pub(crate) fn serve(&self, headers: &HeaderMap, cache_control: &'static str) -> Response {
        revalidate(
            headers,
            self.etag.clone(),
            cache_control,
            ([(CONTENT_TYPE, self.content_type)], self.bytes.clone()),
        )
    }
}

pub(crate) const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub(crate) fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}

/// weak since compression changes the bytes
pub(crate) fn etag(bytes: &[u8]) -> HeaderValue {
    HeaderValue::from_str(&format!("W/\"{:x}\"", content_hash(bytes))).unwrap()
}

pub(crate) fn fingerprinted(page: &str, assets: &[Asset]) -> String {
    assets.iter().fold(page.to_string(), |page, asset| {
        page.replace(
            &format!("\"{}\"", asset.path),
            &format!("\"{}\"", asset.fingerprinted),
        )
    })
}

pub(crate) fn revalidate(
    request: &HeaderMap,
    etag: HeaderValue,
    cache_control: &'static str,
    content: impl IntoResponse,
) -> Response {
    let tag = etag.to_str().unwrap().trim_start_matches("W/");
    let unchanged = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|item| {
                let item = item.trim().trim_start_matches("W/");
                item == tag || item == "*"
            })
        });
    let content = content.into_response();
    let mut response = if unchanged {
        use tower_http::compression::{DefaultPredicate, Predicate};
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        for value in content.headers().get_all(header::VARY) {
            response.headers_mut().append(header::VARY, value.clone());
        }
        if !content.headers().contains_key(header::CONTENT_ENCODING)
            && !content.headers().contains_key(header::CONTENT_RANGE)
            && DefaultPredicate::new().should_compress(&content)
            && !content.headers().get_all(header::VARY).iter().any(|value| {
                value.to_str().is_ok_and(|value| {
                    value
                        .split(',')
                        .any(|part| part.trim().eq_ignore_ascii_case("accept-encoding"))
                })
            })
        {
            response
                .headers_mut()
                .append(header::VARY, HeaderValue::from_static("accept-encoding"));
        }
        response
    } else {
        content
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, etag);
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

pub(crate) static ASSETS: LazyLock<Vec<Asset>> = LazyLock::new(|| {
    let mut assets = vec![
        Asset::new(
            "theme.js",
            Bytes::from_static(include_bytes!("../web/theme.js")),
            "text/javascript; charset=utf-8",
        ),
        Asset::new(
            "style.css",
            Bytes::from_static(include_bytes!("../web/style.css")),
            "text/css; charset=utf-8",
        ),
        Asset::new(
            "favicon.svg",
            Bytes::from_static(include_bytes!("../web/favicon.svg")),
            "image/svg+xml",
        ),
        Asset::new(
            "favicon.ico",
            Bytes::from_static(include_bytes!("../web/favicon.ico")),
            "image/x-icon",
        ),
        Asset::new(
            "apple-touch-icon.png",
            Bytes::from_static(include_bytes!("../web/apple-touch-icon.png")),
            "image/png",
        ),
    ];
    // Dependencies come first so each importer's hash covers their URLs.
    for (name, source) in [
        ("shared.js", include_str!("../web/shared.js")),
        ("text.js", include_str!("../web/text.js")),
        ("suggestions.js", include_str!("../web/suggestions.js")),
        ("settings.js", include_str!("../web/settings.js")),
        ("search.js", include_str!("../web/search.js")),
        ("entry.js", include_str!("../web/entry.js")),
        ("handwriting.js", include_str!("../web/handwriting.js")),
        ("app.js", include_str!("../web/app.js")),
    ] {
        assets.push(Asset::module(name, source, &assets));
    }
    assets
});

pub(crate) async fn instant_file(
    State(state): State<Arc<AppState>>,
    Path(version): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if uri.query().is_some() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Query strings are not allowed.",
        ));
    }
    let instant = state
        .instant
        .get()
        .filter(|instant| instant.path.rsplit('/').next() == Some(version.as_str()))
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Not found."))?;
    // Content-Encoding keeps the compression layer from gzipping this again.
    let (encoding, body) = if accepts_gzip(&headers) {
        ("gzip", instant.gzip.clone())
    } else {
        ("identity", instant.body.clone())
    };
    let mut content = ([(CONTENT_TYPE, "application/json")], body).into_response();
    content
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    if encoding == "gzip" {
        content
            .headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    Ok(revalidate(
        &headers,
        instant.etag.clone(),
        IMMUTABLE,
        content,
    ))
}

/// Match the compression layer's choice between gzip and identity.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let (mut gzip, mut identity) = (0, None);
    for part in headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
    {
        let mut part = part.splitn(2, ';');
        let name = part.next().unwrap_or("").trim();
        let q = match part.next().map(str::trim) {
            None => 1000,
            Some(q) => match q.strip_prefix(['q', 'Q']).and_then(|q| q.strip_prefix('=')) {
                Some(number) => match quality_thousandths(number) {
                    Some(q) => q,
                    None => continue,
                },
                None => continue,
            },
        };
        if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
            gzip = gzip.max(q);
        } else if name.eq_ignore_ascii_case("identity") {
            identity = identity.max(Some(q));
        }
    }
    gzip > 0 && Some(gzip) >= identity
}

fn quality_thousandths(text: &str) -> Option<u16> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let thousandths: u16 = format!("{fraction:0<3}").parse().ok()?;
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

/// ids can change on reload, so always revalidate
pub(crate) fn revalidate_json(bytes: Vec<u8>, request: &HeaderMap) -> Response {
    revalidate(
        request,
        etag(&bytes),
        "private, no-cache",
        ([(CONTENT_TYPE, "application/json")], bytes),
    )
}
