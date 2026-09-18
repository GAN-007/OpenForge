use anyhow::{Context, Result};
use openforge_protocol::{DataClassification, ModelSpec};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenForgeConfig {
    #[serde(default = "default_state_db")]
    pub state_db: String,
    #[serde(default = "default_artifact_dir")]
    pub artifact_dir: String,
    #[serde(default = "default_worktree_dir")]
    pub worktree_dir: String,
    #[serde(default = "default_parallel")]
    pub max_parallel_agents: usize,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub browser: BrowserWorkerConfig,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

fn default_state_db() -> String { ".openforge/state.db".into() }
fn default_artifact_dir() -> String { ".openforge/artifacts".into() }
fn default_worktree_dir() -> String { ".openforge/worktrees".into() }
fn default_parallel() -> usize { 4 }

impl Default for OpenForgeConfig {
    fn default() -> Self {
        Self {
            state_db: default_state_db(),
            artifact_dir: default_artifact_dir(),
            worktree_dir: default_worktree_dir(),
            max_parallel_agents: default_parallel(),
            providers: vec![],
            mcp_servers: vec![],
            browser: BrowserWorkerConfig::default(),
            environment: BTreeMap::new(),
        }
    }
}

impl OpenForgeConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read config {}", path.as_ref().display()))?;
        serde_yaml::from_str(&raw).context("parse OpenForge config")
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_protocol_bytes")]
    pub max_response_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserWorkerConfig {
    #[serde(default = "default_browser_program")]
    pub program: String,
    #[serde(default = "default_browser_args")]
    pub args: Vec<String>,
    #[serde(default = "default_browser_timeout")]
    pub timeout_seconds: u64,
}

impl Default for BrowserWorkerConfig {
    fn default() -> Self {
        Self {
            program: default_browser_program(),
            args: default_browser_args(),
            timeout_seconds: default_browser_timeout(),
        }
    }
}

fn default_mcp_timeout() -> u64 { 60 }
fn default_protocol_bytes() -> usize { 8 * 1024 * 1024 }
fn default_browser_timeout() -> u64 { 60 }
fn default_browser_program() -> String { "node".into() }
fn default_browser_args() -> Vec<String> {
    vec!["packages/browser-worker/dist/index.js".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub region: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub family: String,
    pub context_tokens: u32,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub structured_output: bool,
    pub input_usd_per_million: f64,
    pub output_usd_per_million: f64,
    #[serde(default = "default_half")]
    pub latency_score: f64,
    #[serde(default = "default_quality")]
    pub quality_score: f64,
    #[serde(default = "default_privacy")]
    pub privacy_score: f64,
    #[serde(default)]
    pub max_data_classification: DataClassification,
}

fn default_half() -> f64 { 0.5 }
fn default_quality() -> f64 { 0.8 }
fn default_privacy() -> f64 { 0.7 }

impl ModelConfig {
    pub fn to_spec(&self, provider: &str) -> ModelSpec {
        ModelSpec {
            provider: provider.into(),
            model: self.id.clone(),
            family: self.family.clone(),
            context_tokens: self.context_tokens,
            supports_tools: self.tools,
            supports_vision: self.vision,
            supports_structured_output: self.structured_output,
            input_usd_per_million: self.input_usd_per_million,
            output_usd_per_million: self.output_usd_per_million,
            latency_score: self.latency_score,
            quality_score: self.quality_score,
            privacy_score: self.privacy_score,
            max_data_classification: self.max_data_classification,
        }
    }
}
