use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactDescriptor {
    pub digest: String,
    pub bytes: u64,
    pub media_type: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    stream_lock: Arc<Mutex<()>>,
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("metadata"))?;
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self {
            root,
            stream_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn put_bytes(
        &self,
        bytes: &[u8],
        media_type: impl Into<String>,
        source: impl Into<String>,
        metadata: BTreeMap<String, Value>,
    ) -> Result<ArtifactDescriptor> {
        let digest = hex::encode(Sha256::digest(bytes));
        let object = self.object_path(&digest)?;
        if !object.exists() {
            if let Some(parent) = object.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = self
                .root
                .join("tmp")
                .join(format!("{digest}.{}.tmp", uuid::Uuid::new_v4()));
            {
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&tmp)
                    .with_context(|| format!("create {}", tmp.display()))?;
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            match fs::rename(&tmp, &object) {
                Ok(()) => {}
                Err(_error) if object.exists() => {
                    let _ = fs::remove_file(&tmp);
                    let existing = fs::read(&object)?;
                    if hex::encode(Sha256::digest(&existing)) != digest {
                        bail!("artifact digest collision or corrupted object");
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }

        let descriptor = ArtifactDescriptor {
            digest: digest.clone(),
            bytes: bytes.len() as u64,
            media_type: media_type.into(),
            source: source.into(),
            created_at: Utc::now(),
            metadata,
        };
        self.write_descriptor(&descriptor)?;
        Ok(descriptor)
    }

    pub fn begin_stream(
        &self,
        media_type: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<String> {
        let upload_id = uuid::Uuid::new_v4().to_string();
        let stream_path = self.root.join("tmp").join(format!("{upload_id}.stream"));
        let metadata_path = self
            .root
            .join("tmp")
            .join(format!("{upload_id}.stream.meta.json"));

        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stream_path)
            .with_context(|| format!("create artifact stream {}", stream_path.display()))?;
        file.sync_all()?;
        fs::write(
            &metadata_path,
            serde_json::to_vec(&serde_json::json!({
                "media_type": media_type.into(),
                "source": source.into(),
                "created_at": Utc::now()
            }))?,
        )?;
        Ok(upload_id)
    }

    pub fn write_stream_chunk(&self, upload_id: &str, bytes: &[u8]) -> Result<()> {
        let _guard = self
            .stream_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("artifact stream lock poisoned"))?;
        validate_upload_id(upload_id)?;
        let path = self.root.join("tmp").join(format!("{upload_id}.stream"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .with_context(|| format!("open artifact stream {}", path.display()))?;
        file.write_all(bytes)?;
        Ok(())
    }

    pub fn commit_stream(
        &self,
        upload_id: &str,
        metadata: BTreeMap<String, Value>,
    ) -> Result<ArtifactDescriptor> {
        // Keep the hashed bytes immutable until the object has been published.
        let _guard = self
            .stream_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("artifact stream lock poisoned"))?;
        validate_upload_id(upload_id)?;
        let stream_path = self.root.join("tmp").join(format!("{upload_id}.stream"));
        let metadata_path = self
            .root
            .join("tmp")
            .join(format!("{upload_id}.stream.meta.json"));
        let stream_metadata: Value =
            serde_json::from_slice(&fs::read(&metadata_path).with_context(|| {
                format!("read artifact stream metadata {}", metadata_path.display())
            })?)?;
        let media_type = stream_metadata
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        let source = stream_metadata
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("rpc-stream")
            .to_string();

        let (digest, bytes) = hash_file(&stream_path)?;
        let object = self.object_path(&digest)?;
        if let Some(parent) = object.parent() {
            fs::create_dir_all(parent)?;
        }

        if object.exists() {
            let (existing_digest, existing_bytes) = hash_file(&object)?;
            if existing_digest != digest || existing_bytes != bytes {
                bail!("artifact digest collision or corrupted object");
            }
            fs::remove_file(&stream_path)?;
        } else {
            match fs::rename(&stream_path, &object) {
                Ok(()) => {}
                Err(_error) if object.exists() => {
                    let (existing_digest, existing_bytes) = hash_file(&object)?;
                    if existing_digest != digest || existing_bytes != bytes {
                        bail!("artifact digest collision or corrupted object");
                    }
                    let _ = fs::remove_file(&stream_path);
                }
                Err(error) => return Err(error.into()),
            }
        }

        let descriptor = ArtifactDescriptor {
            digest,
            bytes,
            media_type,
            source,
            created_at: Utc::now(),
            metadata,
        };
        self.write_descriptor(&descriptor)?;
        fs::remove_file(&metadata_path)?;
        Ok(descriptor)
    }

    pub fn abort_stream(&self, upload_id: &str) -> Result<bool> {
        let _guard = self
            .stream_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("artifact stream lock poisoned"))?;
        validate_upload_id(upload_id)?;
        let stream_path = self.root.join("tmp").join(format!("{upload_id}.stream"));
        let metadata_path = self
            .root
            .join("tmp")
            .join(format!("{upload_id}.stream.meta.json"));
        let mut removed = false;
        for path in [stream_path, metadata_path] {
            if path.exists() {
                fs::remove_file(path)?;
                removed = true;
            }
        }
        Ok(removed)
    }

    pub fn put_file(
        &self,
        path: impl AsRef<Path>,
        media_type: impl Into<String>,
        source: impl Into<String>,
        metadata: BTreeMap<String, Value>,
    ) -> Result<ArtifactDescriptor> {
        let bytes = fs::read(path.as_ref())
            .with_context(|| format!("read artifact {}", path.as_ref().display()))?;
        self.put_bytes(&bytes, media_type, source, metadata)
    }

    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        let object = self.object_path(digest)?;
        let bytes = fs::read(&object)
            .with_context(|| format!("read artifact object {}", object.display()))?;
        let calculated = hex::encode(Sha256::digest(&bytes));
        if calculated != digest {
            bail!("artifact {digest} failed digest verification");
        }
        Ok(bytes)
    }

    pub fn descriptor(&self, digest: &str) -> Result<ArtifactDescriptor> {
        validate_digest(digest)?;
        let path = self.root.join("metadata").join(format!("{digest}.json"));
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read artifact metadata {}", path.display()))?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn verify(&self, digest: &str) -> Result<bool> {
        let bytes = self.get(digest)?;
        Ok(hex::encode(Sha256::digest(bytes)) == digest)
    }

    pub fn contains(&self, digest: &str) -> Result<bool> {
        Ok(self.object_path(digest)?.exists())
    }

    pub fn delete(&self, digest: &str) -> Result<bool> {
        let object = self.object_path(digest)?;
        let metadata = self.root.join("metadata").join(format!("{digest}.json"));
        let existed = object.exists() || metadata.exists();
        if object.exists() {
            fs::remove_file(object)?;
        }
        if metadata.exists() {
            fs::remove_file(metadata)?;
        }
        Ok(existed)
    }

    fn write_descriptor(&self, descriptor: &ArtifactDescriptor) -> Result<()> {
        let path = self
            .root
            .join("metadata")
            .join(format!("{}.json", descriptor.digest));
        let tmp = self.root.join("tmp").join(format!(
            "{}.{}.metadata.tmp",
            descriptor.digest,
            uuid::Uuid::new_v4()
        ));
        fs::write(&tmp, serde_json::to_vec_pretty(descriptor)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    fn object_path(&self, digest: &str) -> Result<PathBuf> {
        validate_digest(digest)?;
        Ok(self
            .root
            .join("objects")
            .join(&digest[0..2])
            .join(&digest[2..4])
            .join(digest))
    }
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("open artifact for hashing {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

fn validate_upload_id(upload_id: &str) -> Result<()> {
    if upload_id.len() != 36
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || uuid::Uuid::parse_str(upload_id).is_err()
    {
        bail!("invalid artifact upload id");
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid sha256 digest");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_chunk_and_commit_publish_only_consistent_objects() {
        for _ in 0..8 {
            let dir = tempfile::tempdir().unwrap();
            let store = ArtifactStore::open(dir.path()).unwrap();
            let upload = store
                .begin_stream("application/octet-stream", "test")
                .unwrap();
            store
                .write_stream_chunk(&upload, &vec![1; 1024 * 1024])
                .unwrap();
            let other = store.clone();
            let id = upload.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let start = barrier.clone();
            let writer = std::thread::spawn(move || {
                start.wait();
                other.write_stream_chunk(&id, &vec![2; 1024 * 1024])
            });
            barrier.wait();
            let descriptor = store.commit_stream(&upload, BTreeMap::new()).unwrap();
            let _ = writer.join().unwrap(); // A chunk after commit is correctly rejected.
            let bytes = store.get(&descriptor.digest).unwrap();
            assert_eq!(bytes.len() as u64, descriptor.bytes);
            assert_eq!(hex::encode(Sha256::digest(&bytes)), descriptor.digest);
        }
    }

    #[test]
    fn streamed_upload_is_finalized_without_buffering_the_whole_payload() {
        let root = std::env::temp_dir().join(format!(
            "openforge-artifacts-stream-{}",
            uuid::Uuid::new_v4()
        ));
        let store = ArtifactStore::open(&root).unwrap();
        let upload = store.begin_stream("text/plain", "stream-test").unwrap();
        store.write_stream_chunk(&upload, b"open").unwrap();
        store.write_stream_chunk(&upload, b"forge").unwrap();
        let descriptor = store.commit_stream(&upload, BTreeMap::new()).unwrap();
        assert_eq!(descriptor.bytes, 9);
        assert_eq!(store.get(&descriptor.digest).unwrap(), b"openforge");
        assert!(!root.join("tmp").join(format!("{upload}.stream")).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_writers_share_content_without_temp_file_collisions() {
        let root = std::env::temp_dir().join(format!(
            "openforge-artifacts-concurrent-{}",
            uuid::Uuid::new_v4()
        ));
        let store = ArtifactStore::open(&root).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .put_bytes(b"shared", "text/plain", "test", BTreeMap::new())
                        .unwrap()
                })
            })
            .collect();
        for writer in writers {
            let descriptor = writer.join().unwrap();
            assert!(store.verify(&descriptor.digest).unwrap());
            assert_eq!(store.descriptor(&descriptor.digest).unwrap().bytes, 6);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stores_and_verifies_content_addressed_bytes() {
        let root = std::env::temp_dir().join(format!("openforge-artifacts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = ArtifactStore::open(&root).unwrap();
        let descriptor = store
            .put_bytes(b"openforge", "text/plain", "test", BTreeMap::new())
            .unwrap();
        assert!(store.verify(&descriptor.digest).unwrap());
        assert_eq!(store.get(&descriptor.digest).unwrap(), b"openforge");
        let _ = fs::remove_dir_all(&root);
    }
}
