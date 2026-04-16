mod cache;
mod config;
mod handlers;

use axum::{routing::get, Router};
use cache::DiskCache;
use config::Config;
use handlers::{AppState, cache_routes};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let store: Arc<dyn cache::CacheStore> = Arc::new(DiskCache::new(&config.cache_dir));

    let state = AppState {
        store,
        access_token: config.access_token,
    };

    let app = Router::new()
        .merge(cache_routes(state))
        .route("/", get(|| async { "Nx Cache Server" }));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    tracing::info!("Starting server at http://0.0.0.0:8080");
    axum::serve(listener, app).await.unwrap();
}
