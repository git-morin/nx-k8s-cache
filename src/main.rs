mod cache;
mod config;
mod handlers;
mod k8s;
mod metrics;

use axum::{extract::DefaultBodyLimit, Router};
use cache::{CacheStore, DiskCache, ObjectStoreCache};
use config::{CacheBackend, Config, SecurityLevel};
use handlers::{cache_routes, health_routes, metrics_routes, AppState, FailureTracker};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{signal, sync::Mutex};
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};

#[tokio::main]
async fn main() {
    let config = Config::from_env();
    init_logging(&config.log_format);
    metrics::init();

    // Cluster detection — required at Paranoid, advisory at other levels.
    if config.security >= SecurityLevel::Paranoid {
        k8s::assert_in_cluster();
    } else if k8s::is_in_cluster() {
        tracing::info!("running inside a Kubernetes cluster");
    } else {
        tracing::warn!("not running inside a Kubernetes cluster — k8s auth unavailable");
    }

    // k8s client is only created (and required) at the Paranoid level.
    let k8s_client: Option<kube::Client> = if config.security >= SecurityLevel::Paranoid {
        Some(
            kube::Client::try_default()
                .await
                .expect("failed to initialise k8s client"),
        )
    } else {
        None
    };

    let write_once = config.security >= SecurityLevel::Standard;
    let verify_integrity = config.security >= SecurityLevel::Hardened;
    let store: Arc<dyn CacheStore> = match &config.backend {
        CacheBackend::Disk => Arc::new(DiskCache::new(
            &config.cache_dir,
            write_once,
            verify_integrity,
        )),
        CacheBackend::S3(s3) => {
            let mut builder =
                object_store::aws::AmazonS3Builder::from_env().with_bucket_name(&s3.bucket);
            if let Some(endpoint) = &s3.endpoint {
                builder = builder.with_endpoint(endpoint).with_allow_http(true);
            }
            if let Some(region) = &s3.region {
                builder = builder.with_region(region);
            }
            let client = builder.build().expect("failed to build S3 client");
            Arc::new(ObjectStoreCache::new(
                Arc::new(client),
                s3.prefix.clone(),
                write_once,
                verify_integrity,
            ))
        }
    };

    let failure_tracker: Option<FailureTracker> = if config.security >= SecurityLevel::Paranoid {
        Some(Arc::new(Mutex::new(HashMap::new())))
    } else {
        None
    };

    tracing::info!(
        addr = "0.0.0.0:8080",
        backend = %config.backend,
        security = %config.security,
        server_namespace = %config.server_namespace,
        max_body_mb = config.max_body_bytes / 1024 / 1024,
        "starting nx-cache-server"
    );

    let state = AppState {
        store,
        access_token: config.access_token,
        security: config.security,
        failure_tracker,
        k8s_client,
        allowed_namespaces: config.allowed_namespaces,
    };

    let app = Router::new()
        .merge(cache_routes(state.clone()))
        .merge(health_routes(state))
        .merge(metrics_routes())
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(60),
        ))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .unwrap();
}

fn init_logging(format: &str) {
    let subscriber = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
    );
    if format == "json" {
        subscriber.json().init();
    } else {
        subscriber.init();
    }
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

    tracing::info!("shutdown signal received, draining in-flight requests");
}
