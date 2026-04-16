pub struct Config {
    pub access_token: String,
    pub cache_dir: String,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            access_token: std::env::var("NX_CACHE_TOKEN")
                .expect("NX_CACHE_TOKEN must be set"),
            cache_dir: std::env::var("NX_CACHE_DIR")
                .unwrap_or_else(|_| "/var/cache/nx".to_string()),
        }
    }
}
