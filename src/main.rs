mod cache;
mod handwriting;
mod instant;
mod pages;
mod requests;
mod search;
mod state;
mod traditional;

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequest, Path, Query, Request, State, rejection::QueryRejection,
    },
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cache::{
    ASSETS, IMMUTABLE, content_hash, etag, instant_file, response_headers, revalidate_json,
    revision,
};
use handwriting::HandwritingIndex;
use pages::{index_html, license_text, licenses_page, not_found, start_script};
use requests::{
    ClientIdentity, HANDWRITING_BODY_LIMIT, MAX_OFFSET, MAX_QUERY_CHARS, MAX_READING_BYTES,
    QUERY_TOO_LONG, REQUEST_TIMEOUT, compute, request_timeout,
};
use search::{SearchIndex, SearchQuery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use state::AppState;
use std::{
    collections::{BTreeMap, HashSet},
    net::{IpAddr, SocketAddr},
    path::{Path as FilePath, PathBuf},
    sync::Arc,
    time::Instant,
};
use tower_http::compression::CompressionLayer;

struct Config {
    data: PathBuf,
    address: SocketAddr,
    feedback_url: Option<String>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let data =
            std::env::var_os("CIDIAN_DATA").map_or_else(|| PathBuf::from("data"), PathBuf::from);
        let host: IpAddr = std::env::var("CIDIAN_HOST")
            .unwrap_or_else(|_| "127.0.0.1".into())
            .parse()
            .map_err(|_| "CIDIAN_HOST must be an IPv4 or IPv6 address")?;
        let port: u16 = std::env::var("CIDIAN_PORT")
            .unwrap_or_else(|_| "3000".into())
            .parse()
            .map_err(|_| "CIDIAN_PORT must be a port number")?;
        Ok(Self {
            data,
            address: SocketAddr::new(host, port),
            feedback_url: feedback_url(&std::env::var("CIDIAN_FEEDBACK_URL").unwrap_or_default())?,
        })
    }
}

fn feedback_url(value: &str) -> Result<Option<String>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    if ["https://", "http://", "mailto:"]
        .iter()
        .any(|prefix| value.starts_with(prefix) && value.len() > prefix.len())
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        Ok(Some(value.into()))
    } else {
        Err("CIDIAN_FEEDBACK_URL must be an http(s) or mailto URL, or empty".into())
    }
}

struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

/// The container image has no shell for a health probe.
fn healthy(config: &Config) -> bool {
    use std::io::{Read, Write};
    let mut address = config.address;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let timeout = std::time::Duration::from_secs(3);
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    let mut reply = String::new();
    stream.set_read_timeout(Some(timeout)).is_ok()
        && stream
            .write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .is_ok()
        && stream.take(4096).read_to_string(&mut reply).is_ok()
        && reply.split(' ').nth(1) == Some("200")
        && reply.contains("\"ok\":true")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();
    let config = Config::from_env()?;
    if std::env::args().nth(1).as_deref() == Some("--health") {
        std::process::exit(if healthy(&config) { 0 } else { 1 });
    }
    let started = Instant::now();
    let dictionary_path = config.data.join("dictionary.jsonl");
    let dictionary = SearchIndex::load(&dictionary_path)
        .map_err(|error| format!("Could not load {}: {error}", dictionary_path.display()))?;
    let characters_path = config.data.join("characters.jsonl");
    let has_characters = characters_path.exists();
    let handwriting = std::thread::scope(|scope| {
        scope.spawn(|| dictionary.prepare_fulltext());
        if has_characters {
            HandwritingIndex::load(&characters_path).map(Some)
        } else {
            eprintln!("Handwriting and character details are off: characters.jsonl was not found");
            Ok(None)
        }
    })?;
    let mut groups = Vec::new();
    if handwriting.is_some() {
        groups.push("characters");
    }
    let licenses_folder = config.data.join("licenses");
    let licenses = load_licenses(&licenses_folder, &groups)?;
    let mut inputs = vec![
        dictionary_path,
        config.data.join("english-support.tsv"),
        std::env::current_exe()?,
    ];
    if has_characters {
        inputs.push(characters_path);
    }
    inputs.extend(licenses.keys().map(|name| licenses_folder.join(name)));
    let revision = revision(&inputs.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
    let workers = std::thread::available_parallelism()
        .map_or(2, usize::from)
        .clamp(1, 8);
    let mut state = AppState::new(
        dictionary,
        handwriting,
        licenses,
        workers,
        config.feedback_url.as_deref(),
    );
    state.revision = revision;
    let state = Arc::new(state);
    release_free_memory();
    // Common searches take seconds to build, so they must not delay startup.
    let builder = state.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let queries = instant::queries(&builder);
        let file = instant::build(&builder, &queries);
        println!(
            "Common searches ready: {} searches, {} KB, in {:.1}s",
            queries.len(),
            file.body.len() / 1000,
            started.elapsed().as_secs_f32()
        );
        let _ = builder.instant.set(file);
        release_free_memory();
    });
    let reporter = state.clone();
    tokio::spawn(async move {
        let mut hour = tokio::time::interval(std::time::Duration::from_secs(3600));
        hour.tick().await;
        loop {
            hour.tick().await;
            if let Some(line) = reporter.stats.take_line() {
                println!("{line}");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    println!(
        "Dictionary ready: {} words, loaded in {:.1}s. Open http://{}",
        state.dictionary.words.len(),
        started.elapsed().as_secs_f32(),
        config.address
    );
    serve(listener, router(state), HEADER_TIMEOUT, shutdown_signal()).await;
    println!("Stopped");
    Ok(())
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(location) = info.location() {
            eprintln!("Panic at {location}");
        } else {
            eprintln!("Panic at an unknown location");
        }
    }));
}

/// glibc keeps freed memory in per-thread arenas unless asked to release it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_free_memory() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    // SAFETY: malloc_trim only hands free heap pages back to the system
    unsafe { malloc_trim(0) };
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_free_memory() {}

fn load_licenses(folder: &FilePath, groups: &[&str]) -> std::io::Result<BTreeMap<String, String>> {
    let requirements: BTreeMap<String, Vec<String>> =
        serde_json::from_str(include_str!("../data/required-notices.json"))
            .expect("checked-in notice requirements");
    let mut licenses = BTreeMap::new();
    for name in std::iter::once("always")
        .chain(groups.iter().copied())
        .flat_map(|group| &requirements[group])
    {
        let path = folder.join(name);
        let missing = |detail: String| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Missing licence notice: {}{detail}", path.display()),
            )
        };
        let text =
            std::fs::read_to_string(&path).map_err(|error| missing(format!(" ({error})")))?;
        if text.trim().is_empty() {
            return Err(missing(String::new()));
        }
        licenses.insert(name.clone(), text);
    }
    Ok(licenses)
}

// also the idle limit between requests on a kept-open connection
const HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// axum::serve has no header timeout.
async fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
    header_timeout: std::time::Duration,
    shutdown: impl Future<Output = ()>,
) {
    use hyper_util::{rt::TokioIo, rt::TokioTimer, service::TowerToHyperService};
    let mut connections = tokio::task::JoinSet::new();
    let (stop, stopped) = tokio::sync::watch::channel(());
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            Some(_) = connections.join_next() => {}
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // out of file descriptors and the like: wait instead of spinning
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                };
                let service = TowerToHyperService::new(app.clone());
                let mut stopped = stopped.clone();
                connections.spawn(async move {
                    let connection = hyper::server::conn::http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(header_timeout)
                        .serve_connection(TokioIo::new(stream), service);
                    tokio::pin!(connection);
                    tokio::select! {
                        _ = connection.as_mut() => return,
                        _ = stopped.changed() => connection.as_mut().graceful_shutdown(),
                    }
                    let _ = connection.await;
                });
            }
        }
    }
    drop(listener);
    drop(stopped);
    let _ = stop.send(());
    let _ = tokio::time::timeout(SHUTDOWN_WAIT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
}

fn router(state: Arc<AppState>) -> Router {
    router_with_timeout(state, REQUEST_TIMEOUT)
}

fn router_with_timeout(state: Arc<AppState>, limit: std::time::Duration) -> Router {
    let mut router = Router::new();
    for (path, handler) in routes() {
        router = router.route(path, handler);
    }
    router
        .layer(DefaultBodyLimit::max(HANDWRITING_BODY_LIMIT))
        .fallback(not_found)
        .method_not_allowed_fallback(|| async {
            ApiError(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.")
        })
        .layer(middleware::from_fn_with_state(limit, request_timeout))
        // Refused requests must not pay for compression.
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            response_headers,
        ))
        .with_state(state)
}

// The router and request-limit coverage test use the same route list.
fn routes() -> Vec<(&'static str, axum::routing::MethodRouter<Arc<AppState>>)> {
    let mut routes = vec![
        ("/", get(index_html)),
        ("/start.js", get(start_script)),
        ("/licenses", get(licenses_page)),
        ("/licenses/{filename}", get(license_text)),
        ("/api/health", get(health)),
        ("/api/info", get(info)),
        ("/api/search", get(search)),
        ("/api/entry/{id}", get(entry)),
        ("/api/character/{character}", get(character)),
        ("/api/handwriting", post(recognize)),
        ("/api/instant/{version}", get(instant_file)),
    ];
    for asset in ASSETS.iter() {
        routes.push((
            asset.path.as_str(),
            get(move |headers: HeaderMap| async move { asset.serve(&headers, "no-cache") }),
        ));
        routes.push((
            asset.fingerprinted.as_str(),
            get(move |headers: HeaderMap| async move { asset.serve(&headers, IMMUTABLE) }),
        ));
    }
    routes
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Could not listen for termination");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({ "ok": true, "count": state.dictionary.words.len(), "handwriting": state.handwriting.is_some() }),
    )
}

async fn info(State(state): State<Arc<AppState>>) -> Json<Value> {
    let mut info = state.info.clone();
    if let Some(instant) = state.instant.get() {
        info["instant"] = json!(instant.path);
    }
    Json(info)
}

async fn search(
    State(state): State<Arc<AppState>>,
    params: Result<Query<SearchQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Query(params) =
        params.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Invalid search parameters."))?;
    if [&params.q, &params.word]
        .iter()
        .any(|text| text.chars().count() > MAX_QUERY_CHARS)
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, QUERY_TOO_LONG.as_str()));
    }
    if params.offset > MAX_OFFSET {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Offset is too large."));
    }
    if params.reading.len() > MAX_READING_BYTES {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Saved reading is too long.",
        ));
    }
    if !matches!(
        params.mode.as_str(),
        "" | "auto" | "chinese" | "english" | "fulltext"
    ) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Unknown search mode."));
    }
    let query = params.normalized();
    let cached = state.cached_search(&query);
    let body = match cached {
        Some(body) => body,
        None => {
            compute(
                state,
                ClientIdentity::from_headers(&headers),
                move |state| state.search(query),
            )
            .await?
        }
    };
    Ok(([(CONTENT_TYPE, "application/json")], body).into_response())
}

#[derive(Serialize)]
struct EntryDetail<'a> {
    entry: &'a search::Word,
    readings: Vec<search::Reading>,
    characters: Vec<EntryCharacter>,
    related: Vec<search::Summary>,
    related_total: usize,
}

#[derive(Serialize)]
struct EntryCharacter {
    character: String,
    pinyin: Vec<String>,
}

#[derive(Default, Deserialize)]
struct EntryQuery {
    #[serde(default)]
    offset: usize,
}

async fn entry(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    params: Result<Query<EntryQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Query(params) =
        params.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Invalid entry parameters."))?;
    if params.offset > MAX_OFFSET {
        return Err(ApiError(StatusCode::BAD_REQUEST, "Offset is too large."));
    }
    let id: u32 = id
        .parse()
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "Word not found."))?;
    let client = ClientIdentity::from_headers(&headers);
    let result = compute(state, client, move |state| {
        let word = state.dictionary.get(id)?;
        let mut seen = HashSet::new();
        let characters = word
            .simplified
            .chars()
            .chain(word.traditional_form().chars())
            .filter(|c| search::is_hanzi(*c) && seen.insert(*c))
            .map(|character| {
                let text = character.to_string();
                let mut pinyin: Vec<String> = state
                    .dictionary
                    .exact_summaries(&text)
                    .into_iter()
                    .map(|summary| summary.pinyin)
                    .collect();
                if pinyin.is_empty()
                    && let Some(details) = state
                        .handwriting
                        .as_ref()
                        .and_then(|index| index.lookup(character))
                {
                    pinyin.extend(details.pinyin.clone());
                }
                let mut unique = HashSet::new();
                pinyin.retain(|reading| unique.insert(reading.to_lowercase()));
                EntryCharacter {
                    character: text,
                    pinyin,
                }
            })
            .collect();
        let (related, related_total) = state.dictionary.related(word, params.offset);
        Some(
            serde_json::to_vec(&state.traditional.display(&EntryDetail {
                entry: &state.dictionary.with_examples(word),
                readings: state.dictionary.readings(word),
                characters,
                related,
                related_total,
            }))
            .expect("Entry details are serializable"),
        )
    })
    .await?;
    result
        .map(|bytes| revalidate_json(bytes, &headers))
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Word not found."))
}

async fn character(
    State(state): State<Arc<AppState>>,
    Path(text): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let mut chars = text.chars();
    let (Some(character), None) = (chars.next().filter(|c| search::is_hanzi(*c)), chars.next())
    else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter one Chinese character.",
        ));
    };
    let body = compute(
        state,
        ClientIdentity::from_headers(&headers),
        move |state| {
            let index = state.handwriting.as_ref().ok_or(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Character data is unavailable.",
            ))?;
            let details = index.lookup(character);
            let entries = state.dictionary.exact_summaries(&text);
            if details.is_none() && entries.is_empty() {
                return Err(ApiError(
                    StatusCode::NOT_FOUND,
                    "Character details not found.",
                ));
            }
            let body = json!({ "character": details, "entries": entries });
            let body = state.traditional.display(&body);
            Ok(body.to_string().into_bytes())
        },
    )
    .await??;
    Ok(revalidate_json(body, &headers))
}

#[derive(Deserialize)]
struct HandwritingRequest {
    strokes: Vec<Vec<[f32; 2]>>,
}

async fn recognize(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let client = ClientIdentity::from_headers(request.headers());
    let permit = state.uploads.acquire(request.headers())?;
    let Json(payload) = Json::<HandwritingRequest>::from_request(request, &state)
        .await
        .map_err(|error| ApiError(error.status(), "Invalid handwriting strokes."))?;
    if !handwriting::valid_strokes(&payload.strokes) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use 1–48 strokes and at most 4096 points, with coordinates between 0 and 1.",
        ));
    }
    if state.handwriting.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Handwriting data is unavailable.",
        ));
    }
    let result = compute(state, client, move |state| {
        let _permit = permit;
        json!({ "candidates": state.handwriting.as_ref().unwrap().recognize(&payload.strokes) })
    })
    .await?;
    Ok(Json(result))
}
