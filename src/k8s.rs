use k8s_openapi::api::authentication::v1::{TokenReview, TokenReviewSpec};
use kube::{api::PostParams, Api, Client};

// ── SA file paths (overridable for testing) ───────────────────────────────────

pub fn sa_token_path() -> String {
    std::env::var("NX_SA_TOKEN_PATH")
        .unwrap_or_else(|_| "/var/run/secrets/kubernetes.io/serviceaccount/token".to_string())
}

pub fn sa_namespace_path() -> String {
    std::env::var("NX_SA_NAMESPACE_PATH")
        .unwrap_or_else(|_| "/var/run/secrets/kubernetes.io/serviceaccount/namespace".to_string())
}

// ── Cluster detection ─────────────────────────────────────────────────────────

/// Returns true when the two canonical in-cluster markers are present:
/// the `KUBERNETES_SERVICE_HOST` env var and the mounted SA token file.
pub fn is_in_cluster() -> bool {
    std::env::var("KUBERNETES_SERVICE_HOST").is_ok()
        && std::path::Path::new(&sa_token_path()).exists()
}

/// Panics with a clear message if not running inside a Kubernetes cluster.
pub fn assert_in_cluster() {
    if !is_in_cluster() {
        panic!(
            "nx-cache-server must run inside a Kubernetes cluster at this security level. \
             Expected KUBERNETES_SERVICE_HOST to be set and a service account token at '{}'.\n\
             Hint: set NX_SA_TOKEN_PATH / NX_SA_NAMESPACE_PATH to override the default paths \
             (useful for testing).",
            sa_token_path()
        );
    }
}

/// Reads the current pod's namespace from the mounted SA namespace file,
/// falling back to the `NX_CACHE_NAMESPACE` env var, then `"default"`.
pub fn server_namespace() -> String {
    if let Ok(ns) = std::env::var("NX_CACHE_NAMESPACE") {
        return ns;
    }
    std::fs::read_to_string(sa_namespace_path())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "default".to_string())
}

// ── TokenReview ───────────────────────────────────────────────────────────────

pub enum ReviewOutcome {
    /// Token is valid, SA is in an allowed namespace.
    Allowed {
        namespace: String,
        service_account: String,
    },
    /// Token is valid but the SA's namespace is not in the allowlist.
    ForbiddenNamespace { namespace: String },
    /// Token was rejected by the k8s API (expired, malformed, etc.).
    Unauthenticated,
    /// We failed to reach the k8s API.
    ApiError(String),
}

/// Validates `token` via the Kubernetes TokenReview API and checks that the
/// service account belongs to one of `allowed_namespaces` (empty = allow any).
pub async fn review_token(
    client: &Client,
    token: &str,
    allowed_namespaces: &[String],
) -> ReviewOutcome {
    let api: Api<TokenReview> = Api::all(client.clone());

    let review = TokenReview {
        spec: TokenReviewSpec {
            token: Some(token.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let result = match api.create(&PostParams::default(), &review).await {
        Ok(r) => r,
        Err(e) => return ReviewOutcome::ApiError(e.to_string()),
    };

    let status = match result.status {
        Some(s) => s,
        None => return ReviewOutcome::ApiError("empty TokenReview status".to_string()),
    };

    if status.authenticated != Some(true) {
        return ReviewOutcome::Unauthenticated;
    }

    // SA token usernames follow the format:
    //   system:serviceaccount:<namespace>:<name>
    let username = status.user.and_then(|u| u.username).unwrap_or_default();

    let parts: Vec<&str> = username.splitn(4, ':').collect();
    if parts.len() != 4 || parts[0] != "system" || parts[1] != "serviceaccount" {
        return ReviewOutcome::ApiError(format!(
            "unexpected username format in TokenReview: {username}"
        ));
    }

    let namespace = parts[2].to_string();
    let service_account = parts[3].to_string();

    if !allowed_namespaces.is_empty() && !allowed_namespaces.iter().any(|n| n == &namespace) {
        return ReviewOutcome::ForbiddenNamespace { namespace };
    }

    ReviewOutcome::Allowed {
        namespace,
        service_account,
    }
}
