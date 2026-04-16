use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Router,
};
use std::sync::Arc;
use crate::cache::CacheStore;
use super::auth::AuthToken;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn CacheStore>,
    pub access_token: String,
}

pub async fn put_handler(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    AuthToken(token): AuthToken,
    body: Bytes,
) -> Response {
    if token != state.access_token {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    if state.store.exists(&hash) {
        return StatusCode::CONFLICT.into_response();
    }
    match state.store.put(&hash, &body) {
        Ok(_) => StatusCode::OK.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_handler(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    AuthToken(token): AuthToken,
) -> Response {
    if token != state.access_token {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    match state.store.get(&hash) {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .body(axum::body::Body::from(data))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn cache_routes(state: AppState) -> Router {
    Router::new()
        .route(
            "/v1/cache/:hash",
            axum::routing::put(put_handler).get(get_handler),
        )
        .with_state(state)
}
