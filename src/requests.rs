use crate::{ApiError, AppState};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Visitors(pub(crate) Mutex<(u64, HashMap<ClientIdentity, VisitorUsage>)>);

#[derive(Default)]
pub(crate) struct VisitorUsage {
    pub(crate) requests: u32,
    pub(crate) computation: Duration,
}

// Typing triggers several requests per key, and schools or offices share an IP.
pub(crate) const REQUESTS_PER_MINUTE: u32 = 600;
// Six seconds of work per minute is about a tenth of one core.
pub(crate) const COMPUTATION_PER_MINUTE: Duration = Duration::from_secs(6);

pub(crate) const HANDWRITING_BODY_LIMIT: usize = 320 * 1024;
const UPLOAD_BUFFER_BUDGET: usize = 5 * 1024 * 1024;
const UPLOADS_PER_CLIENT: usize = 2;
pub(crate) const MAX_UPLOADS: usize = UPLOAD_BUFFER_BUDGET / HANDWRITING_BODY_LIMIT;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ClientIdentity {
    Ip(IpAddr),
    Shared,
}

impl ClientIdentity {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let mut values = headers.get_all("x-real-ip").iter();
        let ip = values
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<IpAddr>().ok());
        if values.next().is_some() {
            return Self::Shared;
        }
        match ip.map(|ip| ip.to_canonical()) {
            Some(IpAddr::V6(ip)) => {
                Self::Ip(IpAddr::V6((u128::from(ip) & (u128::MAX << 64)).into()))
            }
            Some(ip) => Self::Ip(ip),
            None => Self::Shared,
        }
    }
}

#[derive(Default)]
pub(crate) struct Uploads(pub(crate) Mutex<HashMap<ClientIdentity, usize>>);

#[derive(Default)]
pub(crate) struct Computations(
    pub(crate) Mutex<HashSet<ClientIdentity>>,
    tokio::sync::Notify,
);

impl Computations {
    async fn wait(self: &Arc<Self>, client: ClientIdentity) -> Result<ComputationPermit, ApiError> {
        tokio::time::timeout(WORKER_WAIT, async {
            loop {
                let changed = self.1.notified();
                if let Ok(permit) = self.acquire(client) {
                    return permit;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "A lookup is already running. Try again shortly.",
            )
        })
    }

    pub(crate) fn acquire(
        self: &Arc<Self>,
        client: ClientIdentity,
    ) -> Result<ComputationPermit, ApiError> {
        if !self.0.lock().unwrap().insert(client) {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "A lookup is already running. Try again shortly.",
            ));
        }
        Ok(ComputationPermit {
            computations: self.clone(),
            client,
        })
    }
}

pub(crate) struct ComputationPermit {
    computations: Arc<Computations>,
    client: ClientIdentity,
}

impl Drop for ComputationPermit {
    fn drop(&mut self) {
        self.computations.0.lock().unwrap().remove(&self.client);
        self.computations.1.notify_waiters();
    }
}

impl Uploads {
    pub(crate) fn acquire(self: &Arc<Self>, headers: &HeaderMap) -> Result<UploadPermit, ApiError> {
        let client = ClientIdentity::from_headers(headers);
        let mut counts = self.0.lock().unwrap();
        if counts.get(&client).copied().unwrap_or_default() >= UPLOADS_PER_CLIENT {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many uploads. Try again shortly.",
            ));
        }
        if counts.values().sum::<usize>() >= MAX_UPLOADS {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Server is busy. Try again shortly.",
            ));
        }
        *counts.entry(client).or_default() += 1;
        Ok(UploadPermit {
            uploads: self.clone(),
            client,
        })
    }
}

pub(crate) struct UploadPermit {
    uploads: Arc<Uploads>,
    client: ClientIdentity,
}

impl Drop for UploadPermit {
    fn drop(&mut self) {
        let mut counts = self.uploads.0.lock().unwrap();
        let count = counts.get_mut(&self.client).unwrap();
        *count -= 1;
        if *count == 0 {
            counts.remove(&self.client);
        }
    }
}

pub(crate) fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Visitors {
    fn with_usage<T>(
        &self,
        client: ClientIdentity,
        now: u64,
        task: impl FnOnce(&mut VisitorUsage) -> T,
    ) -> T {
        let mut visitors = self.0.lock().unwrap();
        let (minute, counts) = &mut *visitors;
        if *minute != now / 60 {
            *minute = now / 60;
            *counts = HashMap::new();
        }
        task(counts.entry(client).or_default())
    }

    pub(crate) fn over_limit(&self, headers: &HeaderMap, now: u64) -> Option<u64> {
        self.with_usage(ClientIdentity::from_headers(headers), now, |usage| {
            usage.requests = usage.requests.saturating_add(1);
            (usage.requests > REQUESTS_PER_MINUTE).then(|| 60 - now % 60)
        })
    }

    pub(crate) fn computation_wait(&self, client: ClientIdentity, now: u64) -> Option<u64> {
        self.with_usage(client, now, |usage| {
            (usage.computation >= COMPUTATION_PER_MINUTE).then(|| 60 - now % 60)
        })
    }

    pub(crate) fn charge(&self, client: ClientIdentity, now: u64, elapsed: Duration) {
        self.with_usage(client, now, |usage| {
            usage.computation = usage.computation.saturating_add(elapsed);
        });
    }
}

struct ComputationCharge<'a> {
    visitors: &'a Visitors,
    client: ClientIdentity,
    started: Instant,
}

impl Drop for ComputationCharge<'_> {
    fn drop(&mut self) {
        self.visitors
            .charge(self.client, unix_seconds(), self.started.elapsed());
    }
}

/// Count requests without recording what was asked.
#[derive(Default)]
pub(crate) struct Stats {
    search: AtomicU64,
    entry: AtomicU64,
    character: AtomicU64,
    handwriting: AtomicU64,
    other: AtomicU64,
    errors: AtomicU64,
    limited: AtomicU64,
}

impl Stats {
    pub(crate) fn count(&self, path: &str, status: StatusCode) {
        // Docker's health check calls this every 30 seconds
        if path == "/api/health" {
            return;
        }
        let counter = match path.split('/').nth(2).filter(|_| path.starts_with("/api/")) {
            Some("search") => &self.search,
            Some("entry") => &self.entry,
            Some("character") => &self.character,
            Some("handwriting") => &self.handwriting,
            _ => &self.other,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            self.limited.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn take_line(&self) -> Option<String> {
        let take = |counter: &AtomicU64| counter.swap(0, Ordering::Relaxed);
        let (search, entry, character, handwriting, other, errors, limited) = (
            take(&self.search),
            take(&self.entry),
            take(&self.character),
            take(&self.handwriting),
            take(&self.other),
            take(&self.errors),
            take(&self.limited),
        );
        let total = search + entry + character + handwriting + other;
        (total > 0).then(|| {
            format!(
                "Last hour: {total} requests (search {search}, entry {entry}, character {character}, handwriting {handwriting}, other {other}), {errors} errors, {limited} over the limit"
            )
        })
    }
}

pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) async fn request_timeout(
    State(limit): State<std::time::Duration>,
    request: Request,
    next: Next,
) -> Response {
    tokio::time::timeout(limit, next.run(request))
        .await
        .unwrap_or_else(|_| {
            ApiError(StatusCode::REQUEST_TIMEOUT, "The request took too long.").into_response()
        })
}

pub(crate) async fn compute<T: Send + 'static>(
    state: Arc<AppState>,
    client: ClientIdentity,
    task: impl FnOnce(Arc<AppState>) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let client_permit = state.computations.wait(client).await?;
    if state
        .visitors
        .computation_wait(client, unix_seconds())
        .is_some()
    {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests. Wait a minute and try again.",
        ));
    }
    let busy = || {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is busy. Try again shortly.",
        )
    };
    let permit = tokio::time::timeout(WORKER_WAIT, state.workers.clone().acquire_owned())
        .await
        .map_err(|_| busy())?
        .map_err(|_| busy())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _client_permit = client_permit;
        let _charge = ComputationCharge {
            visitors: &state.visitors,
            client,
            started: Instant::now(),
        };
        task(state.clone())
    })
    .await
    .map_err(|_| {
        eprintln!("Dictionary worker failed");
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Request could not be completed.",
        )
    })
}

const WORKER_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) const MAX_QUERY_CHARS: usize = 96;

pub(crate) static QUERY_TOO_LONG: LazyLock<String> =
    LazyLock::new(|| format!("Search is limited to {MAX_QUERY_CHARS} characters"));

pub(crate) const MAX_READING_BYTES: usize = 512;
/// Ranked lists are kept per query, so this only bounds how far a search can page.
pub(crate) const MAX_OFFSET: usize = 10_000;
