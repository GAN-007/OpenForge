use anyhow::{Context, Result, bail};
use openforge_protocol::{DataClassification, ModelSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const MODEL_PROVIDER_SCHEMA: &str =
    include_str!("../../../schemas/model-provider.schema.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenForgeConfig {
    #[serde(default = "default_state_db")]
    pub state_db: String,
    #[serde(default = "default_artifact_dir")]
    pub artifact_dir: String,
    #[serde(default = "default_worktree_dir")]
    pub worktree_dir: String,
    #[serde(default = "default_plugin_dir")]
    pub plugin_dir: PathBuf,
    #[serde(default = "default_parallel")]
    pub max_parallel_agents: usize,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub browser: BrowserWorkerConfig,
    #[serde(default)]
    pub kubernetes: KubernetesRunnerConfig,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

fn default_state_db() -> String {
    ".openforge/state.db".into()
}
fn default_artifact_dir() -> String {
    ".openforge/artifacts".into()
}
fn default_worktree_dir() -> String {
    ".openforge/worktrees".into()
}
fn default_plugin_dir() -> PathBuf {
    PathBuf::from("plugins")
}
fn default_parallel() -> usize {
    4
}

impl Default for OpenForgeConfig {
    fn default() -> Self {
        Self {
            state_db: default_state_db(),
            artifact_dir: default_artifact_dir(),
            worktree_dir: default_worktree_dir(),
            plugin_dir: default_plugin_dir(),
            max_parallel_agents: default_parallel(),
            providers: vec![],
            mcp_servers: vec![],
            browser: BrowserWorkerConfig::default(),
            kubernetes: KubernetesRunnerConfig::default(),
            environment: BTreeMap::new(),
        }
    }
}

impl OpenForgeConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read config {}", path.as_ref().display()))?;
        let config: Self = serde_yaml::from_str(&raw).context("parse OpenForge config")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.max_parallel_agents == 0 {
            bail!("max_parallel_agents must be at least 1");
        }
        if self.providers.is_empty() {
            bail!("OpenForge config requires at least one model provider");
        }

        let schema: Value =
            serde_json::from_str(MODEL_PROVIDER_SCHEMA).context("parse model provider schema")?;
        let mut provider_names = BTreeSet::new();
        for (index, provider) in self.providers.iter().enumerate() {
            let value = serde_json::to_value(provider)
                .with_context(|| format!("serialize provider {}", provider.name))?;
            validate_schema_value(&value, &schema, &format!("providers[{index}]"))?;
            if !provider_names.insert(provider.name.as_str()) {
                bail!("duplicate model provider name {}", provider.name);
            }
            let mut model_ids = BTreeSet::new();
            for model in &provider.models {
                if !model_ids.insert(model.id.as_str()) {
                    bail!(
                        "duplicate model id {} in provider {}",
                        model.id,
                        provider.name
                    );
                }
            }
        }
        Ok(())
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
pub struct KubernetesRunnerConfig {
    #[serde(default = "default_kubernetes_namespace")]
    pub namespace: String,
    #[serde(default = "default_kubernetes_image")]
    pub image: String,
    #[serde(default = "default_kubernetes_wait_seconds")]
    pub wait_seconds: u64,
}

impl Default for KubernetesRunnerConfig {
    fn default() -> Self {
        Self {
            namespace: default_kubernetes_namespace(),
            image: default_kubernetes_image(),
            wait_seconds: default_kubernetes_wait_seconds(),
        }
    }
}

fn default_kubernetes_namespace() -> String {
    "default".into()
}
fn default_kubernetes_image() -> String {
    "ghcr.io/gan-007/openforge-runner:latest".into()
}
fn default_kubernetes_wait_seconds() -> u64 {
    90
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

fn default_mcp_timeout() -> u64 {
    60
}
fn default_protocol_bytes() -> usize {
    8 * 1024 * 1024
}
fn default_browser_timeout() -> u64 {
    60
}
fn default_browser_program() -> String {
    "node".into()
}
fn default_browser_args() -> Vec<String> {
    vec!["packages/browser-worker/dist/index.js".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

fn default_half() -> f64 {
    0.5
}
fn default_quality() -> f64 {
    0.8
}
fn default_privacy() -> f64 {
    0.7
}

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

fn validate_schema_value(value: &Value, schema: &Value, path: &str) -> Result<()> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            other => bail!("model-provider schema uses unsupported type {other}"),
        };
        if !matches {
            bail!("{path} must be a {expected}");
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.iter().any(|candidate| candidate == value)
    {
        bail!("{path} contains a value outside the model-provider schema enum");
    }

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|number| number < minimum)
    {
        bail!("{path} must be >= {minimum}");
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|number| number > maximum)
    {
        bail!("{path} must be <= {maximum}");
    }
    if let Some(min_length) = schema.get("minLength").and_then(Value::as_u64)
        && value
            .as_str()
            .is_some_and(|text| text.chars().count() < min_length as usize)
    {
        bail!("{path} is shorter than the schema minimum length");
    }
    if let Some(min_items) = schema.get("minItems").and_then(Value::as_u64)
        && value
            .as_array()
            .is_some_and(|items| items.len() < min_items as usize)
    {
        bail!("{path} contains fewer than {min_items} items");
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    bail!("{path}.{field} is required by model-provider.schema.json");
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, child) in object {
            if let Some(child_schema) = properties.and_then(|items| items.get(key)) {
                validate_schema_value(child, child_schema, &format!("{path}.{key}"))?;
                continue;
            }
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    bail!("{path}.{key} is not allowed by model-provider.schema.json")
                }
                Some(extra @ Value::Object(_)) => {
                    validate_schema_value(child, extra, &format!("{path}.{key}"))?;
                }
                _ => {}
            }
        }
    }

    if let Some(array) = value.as_array()
        && let Some(item_schema) = schema.get("items")
    {
        for (index, item) in array.iter().enumerate() {
            validate_schema_value(item, item_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_schema_accepts_local_ollama_shim_configuration() {
        let config: OpenForgeConfig = serde_yaml::from_str(
            r#"
providers:
  - name: local
    kind: openai-compatible
    base_url: http://127.0.0.1:11434/v1
    models:
      - id: qwen2.5-coder:7b
        family: qwen
        context_tokens: 32768
        tools: true
        structured_output: true
        input_usd_per_million: 0
        output_usd_per_million: 0
        latency_score: 0.35
        quality_score: 0.78
        privacy_score: 1.0
        max_data_classification: RESTRICTED
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn provider_schema_rejects_unknown_fields_invalid_scores_and_duplicates() {
        let unknown = serde_yaml::from_str::<OpenForgeConfig>(
            r#"
providers:
  - name: local
    kind: openai-compatible
    unsupported: true
    models: []
"#,
        );
        assert!(unknown.is_err());

        let config: OpenForgeConfig = serde_yaml::from_str(
            r#"
providers:
  - name: local
    kind: openai-compatible
    models:
      - id: one
        family: qwen
        context_tokens: 1
        input_usd_per_million: 0
        output_usd_per_million: 0
        quality_score: 1.2
      - id: one
        family: qwen
        context_tokens: 1
        input_usd_per_million: 0
        output_usd_per_million: 0
"#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }
}
