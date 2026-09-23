mod anthropic;
mod bedrock_cli;
mod fabric;
mod gemini;
mod ollama;
mod openai_compatible;
mod router;

pub use anthropic::{AnthropicConfig, AnthropicProvider};
pub use bedrock_cli::{BedrockCliConfig, BedrockCliProvider};
pub use fabric::FabricProvider;
pub use gemini::{GeminiConfig, GeminiProvider};
pub use ollama::{OllamaConfig, OllamaProvider};
pub use openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use router::{ModelRouter, RoutingCandidate, RoutingWeights};

use anyhow::Result;
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreflight {
    pub provider: String,
    pub model: String,
    pub ready: bool,
    pub endpoint_reachable: Option<bool>,
    pub installed: Option<bool>,
    pub loaded: Option<bool>,
    pub available_memory_mb: Option<u64>,
    pub required_memory_mb: Option<u64>,
    pub detail: Option<String>,
}

impl ModelPreflight {
    pub fn ready(model: &ModelSpec) -> Self {
        Self {
            provider: model.provider.clone(),
            model: model.model.clone(),
            ready: true,
            endpoint_reachable: None,
            installed: None,
            loaded: None,
            available_memory_mb: None,
            required_memory_mb: None,
            detail: None,
        }
    }

    pub fn unavailable(model: &ModelSpec, detail: impl Into<String>) -> Self {
        Self {
            provider: model.provider.clone(),
            model: model.model.clone(),
            ready: false,
            endpoint_reachable: None,
            installed: None,
            loaded: None,
            available_memory_mb: None,
            required_memory_mb: None,
            detail: Some(detail.into()),
        }
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn catalog(&self) -> &[ModelSpec];

    async fn preflight(&self, model: &ModelSpec) -> Result<ModelPreflight> {
        Ok(ModelPreflight::ready(model))
    }

    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse>;
}
