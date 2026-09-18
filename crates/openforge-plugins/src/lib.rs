use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRuntime {
    Wasm,
    Process,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginCapabilities {
    #[serde(default)]
    pub filesystem: Vec<String>,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub database: Vec<String>,
    #[serde(default)]
    pub shell: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
    #[serde(default)]
    pub acp: Vec<String>,
    #[serde(default)]
    pub cloud: Vec<String>,
    #[serde(default)]
    pub deployment: Vec<String>,
    #[serde(default)]
    pub browser: Vec<String>,
    #[serde(default)]
    pub git: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub runtime: PluginRuntime,
    pub entrypoint: String,
    pub api: String,
    #[serde(default)]
    pub capabilities: PluginCapabilities,
    #[serde(default)]
    pub contributes: BTreeMap<String, Vec<String>>,
}

impl PluginManifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read plugin manifest {}", path.as_ref().display()))?;
        let manifest: Self = serde_json::from_str(&raw).context("parse plugin manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != "openforge.plugin/v2" {
            bail!("unsupported plugin schema {}", self.schema);
        }
        if self.id.split('.').count() < 3 {
            bail!("plugin id must be reverse-DNS");
        }
        if self.version.trim().is_empty() || self.entrypoint.trim().is_empty() {
            bail!("plugin version and entrypoint are required");
        }
        if self.api.trim().is_empty() {
            bail!("plugin API compatibility range is required");
        }
        Ok(())
    }
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
