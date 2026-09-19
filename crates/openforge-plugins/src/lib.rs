use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

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
        let entrypoint = Path::new(&self.entrypoint);
        if entrypoint.is_absolute()
            || entrypoint.components().any(|component| {
                matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
            })
        {
            bail!("plugin entrypoint must be repository-relative and cannot traverse directories");
        }
        if self.api.trim().is_empty() {
            bail!("plugin API compatibility range is required");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginCapabilityDeclaration {
    pub domain: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone)]
struct LoadedPlugin {
    manifest_path: PathBuf,
    manifest: PluginManifest,
}

pub struct PluginHost {
    root: PathBuf,
    manifests: BTreeMap<String, LoadedPlugin>,
}

impl PluginHost {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let mut manifests = BTreeMap::new();
        discover_manifests(&root, &mut manifests)?;
        Ok(Self { root, manifests })
    }

    pub fn list(&self) -> Vec<PluginManifest> {
        self.manifests
            .values()
            .map(|loaded| loaded.manifest.clone())
            .collect()
    }

    pub fn get(&self, plugin_id: &str) -> Option<&PluginManifest> {
        self.manifests.get(plugin_id).map(|loaded| &loaded.manifest)
    }

    pub fn entrypoint(&self, plugin_id: &str) -> Result<PathBuf> {
        let loaded = self
            .manifests
            .get(plugin_id)
            .with_context(|| format!("unknown plugin {plugin_id}"))?;
        let manifest_dir = loaded
            .manifest_path
            .parent()
            .context("plugin manifest has no parent directory")?;

        let candidates = [
            manifest_dir.join(&loaded.manifest.entrypoint),
            self.root
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&loaded.manifest.entrypoint),
        ];
        for candidate in candidates {
            if candidate.exists() {
                return candidate
                    .canonicalize()
                    .with_context(|| format!("resolve plugin entrypoint {}", candidate.display()));
            }
        }
        bail!(
            "plugin {} entrypoint {} does not exist",
            plugin_id,
            loaded.manifest.entrypoint
        )
    }

    pub fn validate_capability(
        &self,
        plugin_id: &str,
        capability: &str,
    ) -> Result<PluginCapabilityDeclaration> {
        let manifest = self
            .get(plugin_id)
            .with_context(|| format!("unknown plugin {plugin_id}"))?;
        let (domain, requested) = split_capability(capability);
        let values = capability_values(&manifest.capabilities, domain)
            .with_context(|| format!("unknown plugin capability domain {domain}"))?;
        if let Some(requested) = requested {
            if !values.iter().any(|declared| declared == requested) {
                bail!("plugin {plugin_id} does not declare capability {domain}:{requested}");
            }
        } else if values.is_empty() {
            bail!("plugin {plugin_id} declares no {domain} capabilities");
        }
        Ok(PluginCapabilityDeclaration {
            domain: domain.to_string(),
            values: values.to_vec(),
        })
    }
}

fn discover_manifests(
    directory: &Path,
    manifests: &mut BTreeMap<String, LoadedPlugin>,
) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read plugin directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            discover_manifests(&path, manifests)?;
            continue;
        }
        if entry.file_name() != "plugin.json" {
            continue;
        }
        let manifest = PluginManifest::load(&path)?;
        if manifests.contains_key(&manifest.id) {
            bail!("duplicate plugin id {}", manifest.id);
        }
        manifests.insert(
            manifest.id.clone(),
            LoadedPlugin {
                manifest_path: path,
                manifest,
            },
        );
    }
    Ok(())
}

fn split_capability(capability: &str) -> (&str, Option<&str>) {
    const DOMAINS: [&str; 11] = [
        "filesystem", "network", "secrets", "database", "shell", "mcp", "acp",
        "cloud", "deployment", "browser", "git",
    ];
    for domain in DOMAINS {
        if capability == domain {
            return (domain, None);
        }
        if let Some(value) = capability.strip_prefix(&format!("{domain}:")) {
            return (domain, Some(value));
        }
        if let Some(value) = capability.strip_prefix(&format!("{domain}.")) {
            return (domain, Some(value));
        }
    }
    (capability, None)
}

fn capability_values<'a>(
    capabilities: &'a PluginCapabilities,
    domain: &str,
) -> Option<&'a [String]> {
    Some(match domain {
        "filesystem" => &capabilities.filesystem,
        "network" => &capabilities.network,
        "secrets" => &capabilities.secrets,
        "database" => &capabilities.database,
        "shell" => &capabilities.shell,
        "mcp" => &capabilities.mcp,
        "acp" => &capabilities.acp,
        "cloud" => &capabilities.cloud,
        "deployment" => &capabilities.deployment,
        "browser" => &capabilities.browser,
        "git" => &capabilities.git,
        _ => return None,
    })
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
