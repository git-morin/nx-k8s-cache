use super::auth::{check_k8s_auth, check_static_auth, AuthOutcome};
use crate::cache::{CacheError, CacheStore};
use crate::config::SecurityLevel;
use crate::metrics::CACHE_OPS;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const RATE_WINDOW: Duration = Duration::from_secs(60);
const RATE_MAX_FAILURES: u32 = 10;
pub type FailureTracker = Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>;

async fn is_rate_limited(tracker: &FailureTracker, ip: IpAddr) -> bool {
    let map = tracker.lock().await;
    match map.get(&ip) {
        Some((count, start)) if start.elapsed() < RATE_WINDOW => *count >= RATE_MAX_FAILURES,
        _ => false,
    }
}

async fn record_failure(tracker: &FailureTracker, ip: IpAddr) {
    let mut map = tracker.lock().await;
    let entry = map.entry(ip).or_insert((0, Instant::now()));
    if entry.1.elapsed() >= RATE_WINDOW {
        *entry = (0, Instant::now());
    }
    entry.0 += 1;

    if map.len() > 10_000 {
        map.retain(|_, (_, start)| start.elapsed() < RATE_WINDOW);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn CacheStore>,
    /// Used at Standard / Hardened levels. Not consulted at Paranoid.
    pub access_token: String,
    pub security: SecurityLevel,
    /// Populated only at SecurityLevel::Paranoid.
    pub failure_tracker: Option<FailureTracker>,
    /// k8s client for TokenReview at Paranoid level.
    pub k8s_client: Option<kube::Client>,
    /// Namespaces callers must belong to (empty = any). Paranoid only.
    pub allowed_namespaces: Vec<String>,
}

/// Hash must be a non-empty hex string, max 128 chars.
/// Prevents path traversal and rejects malformed requests before any I/O.
fn is_valid_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= 128 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Runs the appropriate auth check for the current security level and returns
/// `Some(response)` to reject the request, or `None` to let it through.
async fn enforce_auth(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    client_ip: IpAddr,
    op: &str,
) -> Option<Response> {
    if let Some(tracker) = &state.failure_tracker {
        if is_rate_limited(tracker, client_ip).await {
            CACHE_OPS.with_label_values(&[op, "rate_limited"]).inc();
            return Some(StatusCode::TOO_MANY_REQUESTS.into_response());
        }
    }

    let outcome = if state.security >= SecurityLevel::Paranoid {
        match &state.k8s_client {
            Some(client) => check_k8s_auth(headers, client, &state.allowed_namespaces).await,
            None => AuthOutcome::ApiError("k8s client not initialised".to_string()),
        }
    } else {
        check_static_auth(headers, &state.access_token, state.security)
    };

    match outcome {
        AuthOutcome::Allowed => None,
        AuthOutcome::Unauthorized => {
            tracing::info!(op, %client_ip, "request rejected: missing authorization");
            CACHE_OPS.with_label_values(&[op, "unauthorized"]).inc();
            Some((StatusCode::UNAUTHORIZED, "Missing authorization header").into_response())
        }
        AuthOutcome::Forbidden => {
            if let Some(tracker) = &state.failure_tracker {
                record_failure(tracker, client_ip).await;
            }
            tracing::info!(op, %client_ip, "request rejected: forbidden");
            CACHE_OPS.with_label_values(&[op, "forbidden"]).inc();
            Some((StatusCode::FORBIDDEN, "Forbidden").into_response())
        }
        AuthOutcome::ApiError(e) => {
            tracing::error!(error = %e, "auth backend unavailable");
            CACHE_OPS.with_label_values(&[op, "error"]).inc();
            Some(StatusCode::SERVICE_UNAVAILABLE.into_response())
        }
    }
}

pub async fn put_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Path(hash): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let client_ip = addr.ip();
    if !is_valid_hash(&hash) {
        tracing::info!(%client_ip, hash, "put rejected: invalid hash");
        CACHE_OPS.with_label_values(&["put", "invalid"]).inc();
        return (StatusCode::BAD_REQUEST, "Invalid cache hash").into_response();
    }

    if let Some(r) = enforce_auth(&state, &headers, client_ip, "put").await {
        return r;
    }

    if state.security >= SecurityLevel::Hardened {
        if let Some(cl) = headers.get(header::CONTENT_LENGTH) {
            if let Ok(declared) = cl.to_str().unwrap_or("").parse::<usize>() {
                if declared != body.len() {
                    CACHE_OPS.with_label_values(&["put", "invalid"]).inc();
                    return (StatusCode::BAD_REQUEST, "Content-Length mismatch").into_response();
                }
            }
        }
    }

    let body_len = body.len();
    match state.store.put(&hash, body).await {
        Ok(_) => {
            tracing::info!(%client_ip, hash, bytes = body_len, "cache put: stored");
            CACHE_OPS.with_label_values(&["put", "stored"]).inc();
            StatusCode::OK.into_response()
        }
        Err(CacheError::AlreadyExists(_)) => {
            tracing::info!(%client_ip, hash, "cache put: already exists");
            CACHE_OPS.with_label_values(&["put", "conflict"]).inc();
            StatusCode::CONFLICT.into_response()
        }
        Err(e) => {
            tracing::error!(hash, error = %e, "failed to store cache entry");
            CACHE_OPS.with_label_values(&["put", "error"]).inc();
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn get_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Path(hash): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let client_ip = addr.ip();
    if !is_valid_hash(&hash) {
        tracing::info!(%client_ip, hash, "get rejected: invalid hash");
        CACHE_OPS.with_label_values(&["get", "invalid"]).inc();
        return (StatusCode::BAD_REQUEST, "Invalid cache hash").into_response();
    }

    if let Some(r) = enforce_auth(&state, &headers, client_ip, "get").await {
        return r;
    }

    match state.store.get(&hash).await {
        Ok(data) => {
            tracing::info!(%client_ip, hash, bytes = data.len(), "cache get: hit");
            CACHE_OPS.with_label_values(&["get", "hit"]).inc();
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .body(axum::body::Body::from(data))
                .unwrap()
        }
        Err(CacheError::NotFound(_)) => {
            tracing::info!(%client_ip, hash, "cache get: miss");
            CACHE_OPS.with_label_values(&["get", "miss"]).inc();
            StatusCode::NOT_FOUND.into_response()
        }
        Err(CacheError::Corrupted(_)) => {
            tracing::info!(%client_ip, hash, "cache get: corrupted entry");
            CACHE_OPS.with_label_values(&["get", "corrupted"]).inc();
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!(hash, error = %e, "failed to read cache entry");
            CACHE_OPS.with_label_values(&["get", "error"]).inc();
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub fn cache_routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/v1/cache/{hash}",
            axum::routing::put(put_handler).get(get_handler),
        )
        .with_state(state)
}
