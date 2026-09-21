use anyhow::{Context, Result, bail};
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
        let segments: Vec<_> = self.id.split('.').collect();
        if segments.len() < 3
            || segments.iter().enumerate().any(|(index, segment)| {
                segment.is_empty()
                    || !segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || (index > 0 && byte == b'-')
                    })
            })
        {
            bail!("plugin id must be reverse-DNS");
        }
        if self.version.trim().is_empty() || self.entrypoint.trim().is_empty() {
            bail!("plugin version and entrypoint are required");
        }
        for domain in [
            "filesystem",
            "network",
            "secrets",
            "database",
            "shell",
            "mcp",
            "acp",
            "cloud",
            "deployment",
            "browser",
            "git",
        ] {
            let mut seen = std::collections::BTreeSet::new();
            for value in capability_values(&self.capabilities, domain).expect("known domain") {
                if value.trim().is_empty() || !seen.insert(value) {
                    bail!("empty or duplicate plugin capability in {domain}");
                }
            }
        }
        let entrypoint = Path::new(&self.entrypoint);
        if self.entrypoint.contains('\\')
            || entrypoint.is_absolute()
            || entrypoint.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
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
    repository_root: PathBuf,
    manifests: BTreeMap<String, LoadedPlugin>,
}

impl PluginHost {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let requested = root.as_ref().to_path_buf();
        fs::create_dir_all(&requested)?;
        let root = requested
            .canonicalize()
            .with_context(|| format!("canonicalize plugin root {}", requested.display()))?;
        let repository_root = root
            .parent()
            .unwrap_or(&root)
            .canonicalize()
            .with_context(|| format!("canonicalize plugin repository root {}", root.display()))?;
        let mut manifests = BTreeMap::new();
        discover_manifests(&root, &mut manifests)?;
        Ok(Self {
            root,
            repository_root,
            manifests,
        })
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
                let canonical = candidate.canonicalize().with_context(|| {
                    format!("resolve plugin entrypoint {}", candidate.display())
                })?;
                if !canonical.starts_with(&self.repository_root) {
                    bail!(
                        "plugin {} entrypoint {} escapes repository root {}",
                        plugin_id,
                        canonical.display(),
                        self.repository_root.display()
                    );
                }
                if !canonical.is_file() {
                    bail!("plugin entrypoint must be a file");
                }
                return Ok(canonical);
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
        "filesystem",
        "network",
        "secrets",
        "database",
        "shell",
        "mcp",
        "acp",
        "cloud",
        "deployment",
        "browser",
        "git",
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> serde_json::Value {
        json!({"schema":"openforge.plugin/v2","id":"dev.test.plugin","version":"1.0.0",
            "runtime":"process","entrypoint":"entry.js","api":">=0.2 <1.0","capabilities":{"browser":["navigate"]}})
    }

    #[test]
    fn rejects_invalid_ids_duplicate_capabilities_and_unknown_domains() {
        for id in ["..", "dev..plugin", "dev.Test.plugin", "dev/test.plugin.id"] {
            let mut value = manifest();
            value["id"] = json!(id);
            assert!(
                serde_json::from_value::<PluginManifest>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut value = manifest();
        value["capabilities"]["browser"] = json!(["navigate", "navigate"]);
        assert!(
            serde_json::from_value::<PluginManifest>(value)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut value = manifest();
        value["capabilities"]["typo"] = json!(["read"]);
        assert!(serde_json::from_value::<PluginManifest>(value).is_err());
    }

    #[test]
    fn nested_manifest_directory_need_not_equal_plugin_id() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugins/builtin/example");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("plugin.json"), manifest().to_string()).unwrap();
        fs::write(plugin.join("entry.js"), "export {};").unwrap();
        let host = PluginHost::open(dir.path().join("plugins")).unwrap();
        assert_eq!(host.list().len(), 1);
        assert_eq!(
            host.entrypoint("dev.test.plugin").unwrap(),
            plugin.join("entry.js").canonicalize().unwrap()
        );
        assert!(
            host.validate_capability("dev.test.plugin", "browser:navigate")
                .is_ok()
        );
        assert!(
            host.validate_capability("dev.test.plugin", "browser:delete")
                .is_err()
        );
        fs::create_dir_all(dir.path().join("plugins/duplicate")).unwrap();
        fs::write(
            dir.path().join("plugins/duplicate/plugin.json"),
            manifest().to_string(),
        )
        .unwrap();
        assert!(PluginHost::open(dir.path().join("plugins")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn entrypoint_symlinks_cannot_escape_repository() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugins/example");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("plugin.json"), manifest().to_string()).unwrap();
        fs::write(outside.path().join("entry.js"), "export {};").unwrap();
        std::os::unix::fs::symlink(outside.path().join("entry.js"), plugin.join("entry.js"))
            .unwrap();
        let host = PluginHost::open(dir.path().join("plugins")).unwrap();
        assert!(host.entrypoint("dev.test.plugin").is_err());
    }
}
