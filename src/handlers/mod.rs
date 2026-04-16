mod auth;
mod cache;
mod health;
mod metrics;

pub use cache::{cache_routes, AppState, FailureTracker};
pub use health::health_routes;
pub use metrics::metrics_routes;
