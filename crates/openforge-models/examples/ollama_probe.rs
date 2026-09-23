use anyhow::{Context, Result};
use openforge_models::{
    FabricProvider, ModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use openforge_protocol::{
    ChatMessage, DataClassification, ModelRequest, ModelRequirements, ModelSpec,
};
use std::{env, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
    let model_name = env::args()
        .nth(1)
        .context("usage: cargo run -p openforge-models --example ollama_probe -- <model>")?;
    let root = env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = ModelSpec {
        provider: "local".into(),
        model: model_name.clone(),
        family: "local".into(),
        context_tokens: 32_768,
        supports_tools: false,
        supports_vision: false,
        supports_structured_output: true,
        input_usd_per_million: 0.0,
        output_usd_per_million: 0.0,
        latency_score: 0.0,
        quality_score: 0.8,
        privacy_score: 1.0,
        max_data_classification: DataClassification::Restricted,
    };
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_name: "local".into(),
        base_url: format!("{}/v1", root.trim_end_matches('/')),
        api_key: None,
        extra_headers: vec![],
        models: vec![model.clone()],
    })?;
    let fabric = FabricProvider::new(vec![Arc::new(provider)])?;
    let request = ModelRequest {
        invocation_id: uuid::Uuid::new_v4(),
        run_id: uuid::Uuid::new_v4(),
        task_id: None,
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Reply with exactly PONG".into(),
        }],
        requirements: ModelRequirements {
            task_class: "diagnostic".into(),
            context_tokens: 4096,
            requires_tools: false,
            requires_vision: false,
            requires_structured_output: false,
            max_cost_usd: 0.0,
            max_latency_ms: None,
            data_classification: DataClassification::Internal,
            preferred_model_families: vec![],
            excluded_model_families: vec![],
        },
        temperature: 0.0,
        max_output_tokens: 16,
        response_schema: None,
    };
    let response = fabric.invoke(&model, &request).await?;
    println!("{}", response.text.trim());
    Ok(())
}
