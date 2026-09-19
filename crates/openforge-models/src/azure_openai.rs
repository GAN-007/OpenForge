use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct AzureOpenAiConfig {
    pub provider_name: String,
    pub endpoint: String,
    pub deployment: String,
    pub api_version: String,
    pub api_key: String,
    pub models: Vec<ModelSpec>,
}

pub struct AzureOpenAiProvider {
    cfg: AzureOpenAiConfig,
    client: Client,
}

impl AzureOpenAiProvider {
    pub fn new(cfg: AzureOpenAiConfig) -> Result<Self> {
        if cfg.endpoint.trim().is_empty()
            || cfg.deployment.trim().is_empty()
            || cfg.api_version.trim().is_empty()
            || cfg.api_key.trim().is_empty()
        {
            anyhow::bail!("Azure OpenAI endpoint, deployment, api_version and api_key are required");
        }
        Ok(Self {
            cfg,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CompletionResponse {
    id: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: AssistantMessage,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[async_trait]
impl ModelProvider for AzureOpenAiProvider {
    fn name(&self) -> &str {
        &self.cfg.provider_name
    }

    fn catalog(&self) -> &[ModelSpec] {
        &self.cfg.models
    }

    async fn invoke(
        &self,
        model: &ModelSpec,
        request: &ModelRequest,
    ) -> Result<ModelResponse> {
        let url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.cfg.endpoint.trim_end_matches('/'),
            self.cfg.deployment,
            self.cfg.api_version
        );
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content}))
            .collect();
        let mut body = json!({
            "messages": messages,
            "temperature": request.temperature,
            "max_tokens": request.max_output_tokens,
            "stream": false
        });
        if request.requirements.requires_structured_output {
            body["response_format"] = json!({"type": "json_object"});
        }

        let started = Instant::now();
        let response = self
            .client
            .post(url)
            .header("api-key", &self.cfg.api_key)
            .json(&body)
            .send()
            .await
            .context("Azure OpenAI request failed")?;
        let status = response.status();
        let raw = response
            .text()
            .await
            .context("read Azure OpenAI response")?;
        if !status.is_success() {
            anyhow::bail!("Azure OpenAI returned {status}: {raw}");
        }

        let parsed: CompletionResponse =
            serde_json::from_str(&raw).context("parse Azure OpenAI response")?;
        let usage = parsed.usage.unwrap_or_default();
        let input = usage.prompt_tokens.unwrap_or(0);
        let output = usage.completion_tokens.unwrap_or(0);
        let cost = input as f64 / 1_000_000.0 * model.input_usd_per_million
            + output as f64 / 1_000_000.0 * model.output_usd_per_million;

        Ok(ModelResponse {
            provider: self.name().into(),
            model: model.model.clone(),
            text: parsed
                .choices
                .first()
                .and_then(|choice| choice.message.content.clone())
                .unwrap_or_default(),
            input_tokens: input,
            output_tokens: output,
            latency_ms: started.elapsed().as_millis() as u64,
            cost_usd: cost,
            provider_request_id: parsed.id,
        })
    }
}
