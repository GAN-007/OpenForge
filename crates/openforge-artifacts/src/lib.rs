use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
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
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("metadata"))?;
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
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
                .join(format!("{digest}.{}.tmp", std::process::id()));
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
            bail!("artifact {} failed digest verification", digest);
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
        let tmp = self
            .root
            .join("tmp")
            .join(format!("{}.metadata.tmp", descriptor.digest));
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
    fn stores_and_verifies_content_addressed_bytes() {
        let root = std::env::temp_dir().join(format!(
            "openforge-artifacts-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let store = ArtifactStore::open(&root).unwrap();
        let descriptor = store
            .put_bytes(
                b"openforge",
                "text/plain",
                "test",
                BTreeMap::new(),
            )
            .unwrap();
        assert!(store.verify(&descriptor.digest).unwrap());
        assert_eq!(store.get(&descriptor.digest).unwrap(), b"openforge");
        let _ = fs::remove_dir_all(&root);
    }
}
