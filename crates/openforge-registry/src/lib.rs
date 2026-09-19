use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub schema: String,
    pub generated_at: String,
    pub entries: Vec<RegistryEntry>,
    #[serde(default)]
    pub revoked_digests: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub id: String,
    pub version: Version,
    pub api: VersionReq,
    pub package_url: String,
    pub sha256: String,
    pub signature_base64: String,
    pub signer_public_key_base64: String,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub id: String,
    pub version: Version,
    pub sha256: String,
    pub package_path: String,
    pub installed_at: String,
    pub source_url: String,
}

pub struct RegistryClient {
    http: reqwest::Client,
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl RegistryClient {
    pub async fn fetch_index(&self, url: &str) -> Result<RegistryIndex> {
        let index = self
            .http
            .get(url)
            .send()
            .await
            .context("fetch OpenForge registry index")?
            .error_for_status()
            .context("registry index returned an error status")?
            .json::<RegistryIndex>()
            .await
            .context("parse OpenForge registry index")?;
        validate_index(&index)?;
        Ok(index)
    }

    pub fn select<'a>(
        &self,
        index: &'a RegistryIndex,
        id: &str,
        requirement: &VersionReq,
        api_version: &Version,
    ) -> Result<&'a RegistryEntry> {
        index
            .entries
            .iter()
            .filter(|entry| entry.id == id)
            .filter(|entry| requirement.matches(&entry.version))
            .filter(|entry| entry.api.matches(api_version))
            .filter(|entry| !index.revoked_digests.contains(&entry.sha256))
            .max_by(|left, right| left.version.cmp(&right.version))
            .with_context(|| {
                format!(
                    "no compatible non-revoked registry entry for {id} matching {requirement}"
                )
            })
    }

    pub async fn download_verified(&self, entry: &RegistryEntry) -> Result<Vec<u8>> {
        let bytes = self
            .http
            .get(&entry.package_url)
            .send()
            .await
            .context("download OpenForge plugin package")?
            .error_for_status()
            .context("plugin package returned an error status")?
            .bytes()
            .await?
            .to_vec();
        verify_package(entry, &bytes)?;
        Ok(bytes)
    }
}

pub struct InstalledPackageStore {
    root: PathBuf,
}

impl InstalledPackageStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("packages"))?;
        fs::create_dir_all(root.join("metadata"))?;
        Ok(Self { root })
    }

    pub fn install(&self, entry: &RegistryEntry, bytes: &[u8]) -> Result<InstalledPackage> {
        verify_package(entry, bytes)?;
        let package_path = self
            .root
            .join("packages")
            .join(&entry.sha256);
        if !package_path.exists() {
            let tmp = package_path.with_extension(format!("{}.tmp", std::process::id()));
            fs::write(&tmp, bytes)?;
            fs::rename(&tmp, &package_path)?;
        }

        let installed = InstalledPackage {
            id: entry.id.clone(),
            version: entry.version.clone(),
            sha256: entry.sha256.clone(),
            package_path: package_path.display().to_string(),
            installed_at: chrono_like_now(),
            source_url: entry.package_url.clone(),
        };
        let metadata_path = self
            .root
            .join("metadata")
            .join(format!("{}-{}.json", entry.id.replace('/', "_"), entry.version));
        fs::write(metadata_path, serde_json::to_vec_pretty(&installed)?)?;
        Ok(installed)
    }

    pub fn uninstall(&self, installed: &InstalledPackage) -> Result<bool> {
        let path = PathBuf::from(&installed.package_path);
        let existed = path.exists();
        if existed {
            fs::remove_file(path)?;
        }
        let metadata_path = self
            .root
            .join("metadata")
            .join(format!(
                "{}-{}.json",
                installed.id.replace('/', "_"),
                installed.version
            ));
        if metadata_path.exists() {
            fs::remove_file(metadata_path)?;
        }
        Ok(existed)
    }
}

pub fn verify_package(entry: &RegistryEntry, bytes: &[u8]) -> Result<()> {
    let digest = hex::encode(Sha256::digest(bytes));
    if digest != entry.sha256 {
        bail!(
            "plugin package digest mismatch for {} {}",
            entry.id,
            entry.version
        );
    }

    let key_bytes = BASE64
        .decode(entry.signer_public_key_base64.as_bytes())
        .context("decode plugin signer public key")?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("plugin signer key must be 32 bytes"))?;
    let key = VerifyingKey::from_bytes(&key_array)
        .context("invalid plugin signer public key")?;
    let signature = Signature::from_slice(
        &BASE64
            .decode(entry.signature_base64.as_bytes())
            .context("decode plugin package signature")?,
    )
    .context("invalid plugin package signature")?;
    key.verify(bytes, &signature)
        .context("plugin package signature verification failed")
}

fn validate_index(index: &RegistryIndex) -> Result<()> {
    if index.schema != "openforge.registry/v1" {
        bail!("unsupported registry schema {}", index.schema);
    }
    for entry in &index.entries {
        if entry.id.trim().is_empty()
            || entry.package_url.trim().is_empty()
            || entry.sha256.len() != 64
        {
            bail!("invalid registry entry for {}", entry.id);
        }
    }
    Ok(())
}

fn chrono_like_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}
