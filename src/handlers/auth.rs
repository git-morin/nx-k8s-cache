use crate::config::SecurityLevel;
use crate::k8s::{self, ReviewOutcome};
use axum::http::{header, HeaderMap};
use subtle::ConstantTimeEq;

pub enum AuthOutcome {
    Allowed,
    Unauthorized,     // token missing
    Forbidden,        // token present but wrong / wrong namespace
    ApiError(String), // k8s API unreachable (paranoid only)
}

/// Extracts the bare token from `Authorization: Bearer <token>`.
pub fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Static token check for Standard / Hardened levels.
/// - Standard  → plain `==` (convenient)
/// - Hardened  → constant-time comparison (timing-safe)
pub fn check_static_auth(headers: &HeaderMap, expected: &str, level: SecurityLevel) -> AuthOutcome {
    if level == SecurityLevel::Open {
        return AuthOutcome::Allowed;
    }
    match extract_token(headers) {
        None => AuthOutcome::Unauthorized,
        Some(token) => {
            let valid = if level >= SecurityLevel::Hardened {
                token.as_bytes().ct_eq(expected.as_bytes()).into()
            } else {
                token == expected
            };
            if valid {
                AuthOutcome::Allowed
            } else {
                AuthOutcome::Forbidden
            }
        }
    }
}

/// k8s TokenReview check for Paranoid level.
/// The bearer token is expected to be the caller's service account JWT.
pub async fn check_k8s_auth(
    headers: &HeaderMap,
    client: &kube::Client,
    allowed_namespaces: &[String],
) -> AuthOutcome {
    let token = match extract_token(headers) {
        Some(t) => t,
        None => return AuthOutcome::Unauthorized,
    };

    match k8s::review_token(client, &token, allowed_namespaces).await {
        ReviewOutcome::Allowed {
            namespace,
            service_account,
        } => {
            tracing::debug!(namespace, service_account, "k8s token accepted");
            AuthOutcome::Allowed
        }
        ReviewOutcome::ForbiddenNamespace { namespace } => {
            tracing::warn!(namespace, "caller namespace not in allowlist");
            AuthOutcome::Forbidden
        }
        ReviewOutcome::Unauthenticated => AuthOutcome::Forbidden,
        ReviewOutcome::ApiError(e) => {
            tracing::error!(error = %e, "TokenReview API call failed");
            AuthOutcome::ApiError(e)
        }
    }
}
