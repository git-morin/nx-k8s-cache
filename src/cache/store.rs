use super::errors::CacheError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use object_store::{path::Path as StorePath, ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::{fs, io::AsyncWriteExt};

#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn put(&self, hash: &str, data: Bytes) -> Result<(), CacheError>;
    async fn get(&self, hash: &str) -> Result<Bytes, CacheError>;
    async fn is_accessible(&self) -> bool;
    async fn evict_older_than(&self, ttl: std::time::Duration) -> Result<u64, CacheError>;
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Returns true if `data` matches `expected` digest. Logs a structured error on mismatch.
fn integrity_ok(hash: &str, data: &[u8], expected: &str) -> bool {
    let actual = sha256_hex(data);
    if actual == expected {
        return true;
    }
    tracing::error!(
        hash,
        actual = %actual,
        expected = %expected,
        "SHA-256 mismatch — purging corrupted entry"
    );
    false
}

pub struct DiskCache {
    cache_dir: String,
    /// When true, `O_CREAT|O_EXCL` ensures each hash can only be written once.
    write_once: bool,
    /// When true, a SHA-256 sidecar is stored on PUT and verified on GET.
    verify_integrity: bool,
}

impl DiskCache {
    pub fn new(cache_dir: &str, write_once: bool, verify_integrity: bool) -> Self {
        DiskCache {
            cache_dir: cache_dir.to_string(),
            write_once,
            verify_integrity,
        }
    }

    fn artifact_path(&self, hash: &str) -> PathBuf {
        Path::new(&self.cache_dir).join(hash)
    }

    fn sidecar_path(&self, hash: &str) -> PathBuf {
        Path::new(&self.cache_dir).join(format!("{hash}.sha256"))
    }
}

#[async_trait]
impl CacheStore for DiskCache {
    async fn put(&self, hash: &str, data: Bytes) -> Result<(), CacheError> {
        let artifact = self.artifact_path(hash);

        if self.write_once {
            // O_CREAT|O_EXCL: only the first writer wins.
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&artifact)
                .await
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => {
                        CacheError::AlreadyExists(hash.to_string())
                    }
                    _ => CacheError::Io(e),
                })?;

            let result = async {
                f.write_all(&data).await?;
                f.flush().await
            }
            .await;

            if let Err(e) = result {
                let _ = fs::remove_file(&artifact).await;
                return Err(CacheError::Io(e));
            }
        } else {
            fs::write(&artifact, &data).await?;
        }

        if self.verify_integrity {
            let sidecar = self.sidecar_path(hash);
            let digest = sha256_hex(&data);
            if let Err(e) = fs::write(&sidecar, digest.as_bytes()).await {
                let _ = fs::remove_file(&artifact).await;
                return Err(CacheError::Io(e));
            }
        }

        Ok(())
    }

    async fn get(&self, hash: &str) -> Result<Bytes, CacheError> {
        let artifact = self.artifact_path(hash);

        let raw = fs::read(&artifact).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CacheError::NotFound(hash.to_string()),
            _ => CacheError::Io(e),
        })?;
        let data = Bytes::from(raw);

        if self.verify_integrity {
            let sidecar = self.sidecar_path(hash);
            let expected = fs::read_to_string(&sidecar)
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if !integrity_ok(hash, &data, &expected) {
                let _ = fs::remove_file(&artifact).await;
                let _ = fs::remove_file(&sidecar).await;
                return Err(CacheError::Corrupted(hash.to_string()));
            }
        }

        Ok(data)
    }

    async fn is_accessible(&self) -> bool {
        fs::metadata(&self.cache_dir)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }

    async fn evict_older_than(&self, ttl: std::time::Duration) -> Result<u64, CacheError> {
        let cutoff = SystemTime::now()
            .checked_sub(ttl)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut count = 0u64;
        let mut rd = fs::read_dir(&self.cache_dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "sha256") {
                continue;
            }
            let modified = match entry.metadata().await.and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if modified <= cutoff {
                let _ = fs::remove_file(&path).await;
                let _ = fs::remove_file(path.with_extension("sha256")).await;
                count += 1;
            }
        }
        Ok(count)
    }
}

pub struct ObjectStoreCache {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    write_once: bool,
    verify_integrity: bool,
}

impl ObjectStoreCache {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        prefix: Option<String>,
        write_once: bool,
        verify_integrity: bool,
    ) -> Self {
        ObjectStoreCache {
            store,
            prefix: prefix.unwrap_or_default(),
            write_once,
            verify_integrity,
        }
    }

    fn artifact_path(&self, hash: &str) -> StorePath {
        if self.prefix.is_empty() {
            StorePath::from(hash)
        } else {
            StorePath::from(format!("{}/{hash}", self.prefix))
        }
    }

    fn sidecar_path(&self, hash: &str) -> StorePath {
        if self.prefix.is_empty() {
            StorePath::from(format!("{hash}.sha256"))
        } else {
            StorePath::from(format!("{}/{hash}.sha256", self.prefix))
        }
    }
}

#[async_trait]
impl CacheStore for ObjectStoreCache {
    async fn put(&self, hash: &str, data: Bytes) -> Result<(), CacheError> {
        let path = self.artifact_path(hash);
        let payload = object_store::PutPayload::from(data.clone());

        if self.write_once {
            let opts = PutOptions {
                mode: PutMode::Create,
                ..Default::default()
            };
            self.store
                .put_opts(&path, payload, opts)
                .await
                .map_err(|e| match e {
                    object_store::Error::AlreadyExists { .. } => {
                        CacheError::AlreadyExists(hash.to_string())
                    }
                    _ => CacheError::ObjectStore(e),
                })?;
        } else {
            self.store
                .put(&path, payload)
                .await
                .map_err(CacheError::ObjectStore)?;
        }

        if self.verify_integrity {
            let sidecar = self.sidecar_path(hash);
            let digest = sha256_hex(&data);
            let sidecar_payload = object_store::PutPayload::from(Bytes::from(digest.into_bytes()));
            if let Err(e) = self.store.put(&sidecar, sidecar_payload).await {
                let _ = self.store.delete(&path).await;
                return Err(CacheError::ObjectStore(e));
            }
        }

        Ok(())
    }

    async fn get(&self, hash: &str) -> Result<Bytes, CacheError> {
        let path = self.artifact_path(hash);

        if self.verify_integrity {
            // Fetch artifact and sidecar concurrently to halve round-trip latency.
            let sidecar = self.sidecar_path(hash);
            let (artifact_res, sidecar_res) =
                tokio::join!(self.store.get(&path), self.store.get(&sidecar));

            let data = artifact_res
                .map_err(|e| match e {
                    object_store::Error::NotFound { .. } => CacheError::NotFound(hash.to_string()),
                    _ => CacheError::ObjectStore(e),
                })?
                .bytes()
                .await
                .map_err(CacheError::ObjectStore)?;

            let expected = match sidecar_res {
                Ok(r) => r
                    .bytes()
                    .await
                    .map(|b| String::from_utf8_lossy(&b[..]).trim().to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };

            if !integrity_ok(hash, &data, &expected) {
                let _ = self.store.delete(&path).await;
                let _ = self.store.delete(&sidecar).await;
                return Err(CacheError::Corrupted(hash.to_string()));
            }

            Ok(data)
        } else {
            self.store
                .get(&path)
                .await
                .map_err(|e| match e {
                    object_store::Error::NotFound { .. } => CacheError::NotFound(hash.to_string()),
                    _ => CacheError::ObjectStore(e),
                })?
                .bytes()
                .await
                .map_err(CacheError::ObjectStore)
        }
    }

    async fn is_accessible(&self) -> bool {
        match self.store.head(&StorePath::from("__nx_healthz")).await {
            Ok(_) | Err(object_store::Error::NotFound { .. }) => true,
            Err(_) => false,
        }
    }

    async fn evict_older_than(&self, ttl: std::time::Duration) -> Result<u64, CacheError> {
        let cutoff = SystemTime::now()
            .checked_sub(ttl)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let prefix = if self.prefix.is_empty() {
            None
        } else {
            Some(StorePath::from(self.prefix.as_str()))
        };
        let mut list = self.store.list(prefix.as_ref());
        let mut to_delete: Vec<StorePath> = Vec::new();
        let mut artifact_count = 0u64;
        while let Some(result) = list.next().await {
            let meta = result.map_err(CacheError::ObjectStore)?;
            let last_mod: SystemTime = meta.last_modified.into();
            if last_mod <= cutoff {
                let is_sidecar: &str = meta.location.as_ref();
                if !is_sidecar.ends_with(".sha256") {
                    artifact_count += 1;
                }
                to_delete.push(meta.location);
            }
        }
        for path in to_delete {
            let _ = self.store.delete(&path).await;
        }
        Ok(artifact_count)
    }
}
