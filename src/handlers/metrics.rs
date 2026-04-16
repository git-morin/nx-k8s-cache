use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};

async fn metrics_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        crate::metrics::render(),
    )
}

pub fn metrics_routes() -> Router {
    Router::new().route("/metrics", get(metrics_handler))
}
