use std::{fmt, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SecurityLevel {
    /// No authentication, overwrites allowed, no integrity checks.
    /// Suitable for local development or fully trusted private networks.
    Open = 0,
    /// Bearer token required (plain comparison), write-once per hash.
    /// Suitable for internal CI / single-team use.
    Standard = 1,
    /// Constant-time token comparison, SHA-256 integrity sidecar on every
    /// PUT/GET, Content-Length validation.
    /// Suitable for shared infrastructure or multi-team environments.
    Hardened = 2,
    /// k8s service account tokens validated via TokenReview API, namespace
    /// allowlist enforced, per-IP rate limiting on auth failures.
    /// Requires running inside a Kubernetes cluster. No shared secret needed.
    Paranoid = 3,
}

impl SecurityLevel {
    fn from_env() -> Self {
        match std::env::var("NX_CACHE_SECURITY_LEVEL")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "0" | "open" => SecurityLevel::Open,
            "1" | "standard" => SecurityLevel::Standard,
            "2" | "hardened" => SecurityLevel::Hardened,
            "3" | "paranoid" => SecurityLevel::Paranoid,
            _ => SecurityLevel::Standard,
        }
    }
}

impl fmt::Display for SecurityLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityLevel::Open => write!(f, "open"),
            SecurityLevel::Standard => write!(f, "standard"),
            SecurityLevel::Hardened => write!(f, "hardened"),
            SecurityLevel::Paranoid => write!(f, "paranoid"),
        }
    }
}

pub struct EvictionConfig {
    pub ttl: Duration,
    pub interval: Duration,
}

impl EvictionConfig {
    fn from_env() -> Option<Self> {
        let ttl_secs = std::env::var("NX_EVICTION_TTL_SECS")
            .ok()?
            .parse::<u64>()
            .ok()?;
        let interval_secs = std::env::var("NX_EVICTION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(3600);
        Some(EvictionConfig {
            ttl: Duration::from_secs(ttl_secs),
            interval: Duration::from_secs(interval_secs),
        })
    }
}

// ── Storage backend ───────────────────────────────────────────────────────────

/// S3-compatible object storage configuration.
/// Set `NX_CACHE_BACKEND=s3` to activate; credentials are read from standard
/// AWS env vars or the pod's workload identity / IRSA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Config {
    /// Bucket name — required (`NX_S3_BUCKET`).
    pub bucket: String,
    /// Custom endpoint for MinIO / localstack (`NX_S3_ENDPOINT`).
    pub endpoint: Option<String>,
    /// AWS region (`NX_S3_REGION`). Falls back to `AWS_DEFAULT_REGION`.
    pub region: Option<String>,
    /// Key prefix applied to all stored objects (`NX_S3_PREFIX`).
    pub prefix: Option<String>,
}

impl S3Config {
    fn from_env() -> Option<Self> {
        let bucket = std::env::var("NX_S3_BUCKET").ok()?;
        Some(S3Config {
            bucket,
            endpoint: std::env::var("NX_S3_ENDPOINT").ok(),
            region: std::env::var("NX_S3_REGION").ok(),
            prefix: std::env::var("NX_S3_PREFIX").ok(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheBackend {
    Disk,
    S3(S3Config),
}

impl CacheBackend {
    fn from_env() -> Self {
        match std::env::var("NX_CACHE_BACKEND")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "s3" => {
                let cfg = S3Config::from_env()
                    .expect("NX_S3_BUCKET must be set when NX_CACHE_BACKEND=s3");
                CacheBackend::S3(cfg)
            }
            _ => CacheBackend::Disk,
        }
    }
}

impl fmt::Display for CacheBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheBackend::Disk => write!(f, "disk"),
            CacheBackend::S3(_) => write!(f, "s3"),
        }
    }
}

// ── Top-level config ──────────────────────────────────────────────────────────

pub struct Config {
    /// Static bearer token (used at Standard and Hardened levels).
    /// Not required at Open or Paranoid (k8s SA tokens are used instead).
    pub access_token: String,
    /// Storage backend and its settings.
    pub backend: CacheBackend,
    /// Local cache directory (used only when backend = disk).
    pub cache_dir: String,
    pub max_body_bytes: usize,
    pub log_format: String,
    pub security: SecurityLevel,
    /// Namespace this server is running in (auto-detected from SA file).
    pub server_namespace: String,
    /// Namespaces callers are allowed to originate from.
    /// Empty means all namespaces are accepted (Paranoid level only).
    pub allowed_namespaces: Vec<String>,
    pub eviction: Option<EvictionConfig>,
}

impl Config {
    pub fn from_env() -> Self {
        let security = SecurityLevel::from_env();
        let access_token = match security {
            SecurityLevel::Standard | SecurityLevel::Hardened => std::env::var("NX_CACHE_TOKEN")
                .expect("NX_CACHE_TOKEN must be set for standard / hardened security levels"),
            _ => std::env::var("NX_CACHE_TOKEN").unwrap_or_default(),
        };

        if security >= SecurityLevel::Hardened
            && security < SecurityLevel::Paranoid
            && access_token.len() < 32
        {
            panic!(
                "NX_CACHE_TOKEN must be at least 32 characters at security level HARDENED or above \
                 (got {} characters)",
                access_token.len()
            );
        }

        let allowed_namespaces = std::env::var("NX_ALLOWED_NAMESPACES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();

        Config {
            access_token,
            backend: CacheBackend::from_env(),
            cache_dir: std::env::var("NX_CACHE_DIR")
                .unwrap_or_else(|_| "/var/cache/nx".to_string()),
            max_body_bytes: std::env::var("NX_MAX_BODY_MB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(512)
                * 1024
                * 1024,
            log_format: std::env::var("LOG_FORMAT").unwrap_or_default(),
            security,
            server_namespace: crate::k8s::server_namespace(),
            allowed_namespaces,
            eviction: EvictionConfig::from_env(),
        }
    }
}
