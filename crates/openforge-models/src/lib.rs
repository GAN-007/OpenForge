mod anthropic;
mod bedrock_cli;
mod fabric;
mod gemini;
mod openai_compatible;
mod router;

pub use anthropic::{AnthropicConfig, AnthropicProvider};
pub use bedrock_cli::{BedrockCliConfig, BedrockCliProvider};
pub use fabric::FabricProvider;
pub use gemini::{GeminiConfig, GeminiProvider};
pub use openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use router::{ModelRouter, RoutingCandidate, RoutingWeights};

use anyhow::Result;
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn catalog(&self) -> &[ModelSpec];
    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse>;
}
