mod cache;
mod config;
mod handlers;

use axum::Router;
use cache::{CacheStore, DiskCache};
use config::Config;
use handlers::{AppState, cache_routes, health_routes};
use std::sync::Arc;
use tokio::signal;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let store: Arc<dyn CacheStore> = Arc::new(DiskCache::new(&config.cache_dir));

    let state = AppState {
        store,
        access_token: config.access_token,
    };

    let app = Router::new()
        .merge(cache_routes(state.clone()))
        .merge(health_routes(state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    tracing::info!("Starting server at http://0.0.0.0:8080");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = sigterm => {},
    }

    tracing::info!("Shutdown signal received, draining in-flight requests");
}
