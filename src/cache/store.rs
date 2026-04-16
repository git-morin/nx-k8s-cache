use std::fs;
use std::path::Path;
use super::errors::CacheError;

pub trait CacheStore: Send + Sync {
    fn put(&self, hash: &str, data: &[u8]) -> Result<(), CacheError>;
    fn get(&self, hash: &str) -> Result<Vec<u8>, CacheError>;
    fn exists(&self, hash: &str) -> bool;
    fn is_accessible(&self) -> bool;
}

pub struct DiskCache {
    cache_dir: String,
}

impl DiskCache {
    pub fn new(cache_dir: &str) -> Self {
        DiskCache {
            cache_dir: cache_dir.to_string(),
        }
    }
}

impl CacheStore for DiskCache {
    fn put(&self, hash: &str, data: &[u8]) -> Result<(), CacheError> {
        let path = Path::new(&self.cache_dir).join(hash);
        fs::write(path, data)?;
        Ok(())
    }

    fn get(&self, hash: &str) -> Result<Vec<u8>, CacheError> {
        let path = Path::new(&self.cache_dir).join(hash);
        if !path.exists() {
            return Err(CacheError::NotFound(hash.to_string()));
        }
        Ok(fs::read(path)?)
    }

    fn exists(&self, hash: &str) -> bool {
        Path::new(&self.cache_dir).join(hash).exists()
    }

    fn is_accessible(&self) -> bool {
        Path::new(&self.cache_dir).is_dir()
    }
}
