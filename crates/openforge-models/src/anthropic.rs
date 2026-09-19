use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<ModelSpec>,
}

pub struct AnthropicProvider {
    cfg: AnthropicConfig,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(cfg: AnthropicConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self { cfg, client })
    }
}

#[derive(Debug, Deserialize)]
struct Response {
    id: Option<String>,
    content: Vec<Block>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.cfg.provider_name
    }

    fn catalog(&self) -> &[ModelSpec] {
        &self.cfg.models
    }

    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse> {
        let system = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let messages: Vec<Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                json!({
                    "role": if m.role == "assistant" { "assistant" } else { "user" },
                    "content": m.content
                })
            })
            .collect();

        let body = json!({
            "model": model.model,
            "system": system,
            "messages": messages,
            "max_tokens": request.max_output_tokens,
            "temperature": request.temperature
        });

        let started = Instant::now();
        let res = self
            .client
            .post(format!(
                "{}/v1/messages",
                self.cfg.base_url.trim_end_matches('/')
            ))
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Anthropic request failed")?;

        let status = res.status();
        let raw = res.text().await.context("read Anthropic response")?;
        if !status.is_success() {
            anyhow::bail!("Anthropic returned {status}: {raw}");
        }

        let parsed: Response = serde_json::from_str(&raw).context("parse Anthropic response")?;
        let text = parsed
            .content
            .into_iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");

        let cost = parsed.usage.input_tokens as f64 / 1_000_000.0 * model.input_usd_per_million
            + parsed.usage.output_tokens as f64 / 1_000_000.0 * model.output_usd_per_million;

        Ok(ModelResponse {
            provider: self.name().into(),
            model: model.model.clone(),
            text,
            input_tokens: parsed.usage.input_tokens,
            output_tokens: parsed.usage.output_tokens,
            latency_ms: started.elapsed().as_millis() as u64,
            cost_usd: cost,
            provider_request_id: parsed.id,
        })
    }
}
