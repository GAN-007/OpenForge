use anyhow::{Context, Result, bail};
use openforge_mcp::{McpProcessConfig, McpStdioClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{process::Command, sync::RwLock, time::Duration};

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

impl PluginCapabilities {
    pub fn domains(&self) -> BTreeMap<&'static str, BTreeSet<String>> {
        BTreeMap::from([
            ("filesystem", self.filesystem.iter().cloned().collect()),
            ("network", self.network.iter().cloned().collect()),
            ("secrets", self.secrets.iter().cloned().collect()),
            ("database", self.database.iter().cloned().collect()),
            ("shell", self.shell.iter().cloned().collect()),
            ("mcp", self.mcp.iter().cloned().collect()),
            ("acp", self.acp.iter().cloned().collect()),
            ("cloud", self.cloud.iter().cloned().collect()),
            ("deployment", self.deployment.iter().cloned().collect()),
            ("browser", self.browser.iter().cloned().collect()),
            ("git", self.git.iter().cloned().collect()),
        ])
    }

    pub fn require(&self, domain: &str, value: &str) -> Result<()> {
        let domains = self.domains();
        let allowed = domains
            .get(domain)
            .with_context(|| format!("unknown plugin capability domain {domain}"))?;
        if allowed.iter().any(|pattern| pattern_match(pattern, value)) {
            Ok(())
        } else {
            bail!("plugin capability {domain}:{value} is not declared")
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub root: String,
    pub entrypoint_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInvocation {
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub requested_capabilities: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    60
}

#[derive(Clone, Default)]
pub struct PluginRuntimeManager {
    loaded: Arc<RwLock<HashMap<String, LoadedPlugin>>>,
}

impl PluginRuntimeManager {
    pub async fn load(&self, root: impl AsRef<Path>) -> Result<LoadedPlugin> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("plugin root not found")?;
        let manifest = PluginManifest::load(root.join("plugin.json"))?;
        let entrypoint = safe_entrypoint(&root, &manifest.entrypoint)?;
        let loaded = LoadedPlugin {
            manifest,
            root: root.display().to_string(),
            entrypoint_sha256: sha256_file(entrypoint)?,
        };

        self.loaded
            .write()
            .await
            .insert(loaded.manifest.id.clone(), loaded.clone());
        Ok(loaded)
    }

    pub async fn unload(&self, id: &str) -> bool {
        self.loaded.write().await.remove(id).is_some()
    }

    pub async fn list(&self) -> Vec<LoadedPlugin> {
        let mut values = self
            .loaded
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        values
    }

    pub async fn invoke(&self, id: &str, invocation: PluginInvocation) -> Result<Value> {
        if invocation.method.trim().is_empty() {
            bail!("plugin invocation method cannot be empty");
        }

        let loaded = self
            .loaded
            .read()
            .await
            .get(id)
            .cloned()
            .with_context(|| format!("plugin {id} is not loaded"))?;
        validate_requested_capabilities(
            &loaded.manifest.capabilities,
            &invocation.requested_capabilities,
        )?;
        let root = PathBuf::from(&loaded.root);
        let entrypoint = safe_entrypoint(&root, &loaded.manifest.entrypoint)?;
        let digest = sha256_file(&entrypoint)?;
        if digest != loaded.entrypoint_sha256 {
            bail!("plugin entrypoint changed after load; reload is required");
        }

        match loaded.manifest.runtime {
            PluginRuntime::Process => invoke_process(&loaded, &entrypoint, invocation).await,
            PluginRuntime::Wasm => invoke_wasm(&loaded, &entrypoint, invocation).await,
            PluginRuntime::Mcp => invoke_mcp(&loaded, &entrypoint, invocation).await,
        }
    }
}

async fn invoke_process(
    loaded: &LoadedPlugin,
    entrypoint: &Path,
    invocation: PluginInvocation,
) -> Result<Value> {
    let root = PathBuf::from(&loaded.root);
    let mut command = Command::new(entrypoint);
    command
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();

    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("OPENFORGE_PLUGIN_ID", &loaded.manifest.id);
    command.env(
        "OPENFORGE_PLUGIN_CAPABILITIES",
        serde_json::to_string(&loaded.manifest.capabilities)?,
    );

    let mut child = command.spawn().context("spawn process plugin")?;
    let payload = serde_json::to_vec(&serde_json::json!({
        "method": invocation.method,
        "params": invocation.params
    }))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(&payload).await?;
        stdin.write_all(b"\n").await?;
    }

    let output = tokio::time::timeout(
        Duration::from_secs(invocation.timeout_seconds.clamp(1, 3600)),
        child.wait_with_output(),
    )
    .await
    .context("process plugin timed out")??;
    if !output.status.success() {
        bail!(
            "process plugin failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(
        serde_json::from_slice(&output.stdout)
            .context("process plugin stdout is not valid JSON")?,
    )
}

async fn invoke_wasm(
    loaded: &LoadedPlugin,
    entrypoint: &Path,
    invocation: PluginInvocation,
) -> Result<Value> {
    if !loaded.manifest.capabilities.network.is_empty() {
        bail!("WASM plugins cannot request ambient network access");
    }
    let root = PathBuf::from(&loaded.root);
    let input = serde_json::to_string(&serde_json::json!({
        "method": invocation.method,
        "params": invocation.params
    }))?;

    let mut child = Command::new("wasmtime")
        .arg("run")
        .arg("--dir")
        .arg(&root)
        .arg(entrypoint)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .spawn()
        .context("spawn wasmtime plugin runtime")?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }

    let output = tokio::time::timeout(
        Duration::from_secs(invocation.timeout_seconds.clamp(1, 3600)),
        child.wait_with_output(),
    )
    .await
    .context("WASM plugin timed out")??;
    if !output.status.success() {
        bail!(
            "WASM plugin failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(serde_json::from_slice(&output.stdout).context("WASM plugin stdout is not valid JSON")?)
}

async fn invoke_mcp(
    loaded: &LoadedPlugin,
    entrypoint: &Path,
    invocation: PluginInvocation,
) -> Result<Value> {
    let mut config = McpProcessConfig::new(entrypoint.display().to_string(), Vec::new());
    config.cwd = Some(PathBuf::from(&loaded.root));
    config.request_timeout = Duration::from_secs(invocation.timeout_seconds.clamp(1, 3600));
    let mut client = McpStdioClient::spawn_with_config(config).await?;
    let result = async {
        client
            .initialize("openforge-plugin-runtime", env!("CARGO_PKG_VERSION"))
            .await?;
        let response = client
            .call_tool(&invocation.method, invocation.params)
            .await?;
        Ok::<Value, anyhow::Error>(serde_json::to_value(response)?)
    }
    .await;
    let shutdown = client.shutdown().await;
    match (result, shutdown) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error.context("MCP plugin shutdown failed")),
        (Err(error), _) => Err(error),
    }
}

fn validate_requested_capabilities(
    declared: &PluginCapabilities,
    requested: &BTreeMap<String, Vec<String>>,
) -> Result<()> {
    for (domain, values) in requested {
        for value in values {
            declared.require(domain, value)?;
        }
    }
    Ok(())
}

fn safe_entrypoint(root: &Path, relative: &str) -> Result<PathBuf> {
    let candidate = root.join(relative);
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("plugin entrypoint {} does not exist", candidate.display()))?;
    if !canonical.starts_with(root) {
        bail!("plugin entrypoint escapes plugin root");
    }
    Ok(canonical)
}

fn pattern_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == "**" || pattern == value {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return value.ends_with(suffix);
    }
    false
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
