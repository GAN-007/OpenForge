use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Clone)]
pub struct OpenAiCompatibleConfig {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub models: Vec<ModelSpec>,
}

impl std::fmt::Debug for OpenAiCompatibleConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleConfig")
            .field("provider_name", &self.provider_name)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("extra_headers", &"[REDACTED]")
            .field("models", &self.models)
            .finish()
    }
}

pub struct OpenAiCompatibleProvider {
    cfg: OpenAiCompatibleConfig,
    client: Client,
}
impl OpenAiCompatibleProvider {
    pub fn new(cfg: OpenAiCompatibleConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self { cfg, client })
    }
}

#[derive(Debug, Deserialize)]
struct CompletionResponse {
    id: Option<String>,
    choices: Vec<Choice>,
    usage: Option<Usage>,
}
#[derive(Debug, Deserialize)]
struct Choice {
    message: AssistantMessage,
    finish_reason: Option<String>,
}
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Option<Value>,
}
#[derive(Debug, Deserialize, Default)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.cfg.provider_name
    }
    fn catalog(&self) -> &[ModelSpec] {
        &self.cfg.models
    }

    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({"role":m.role,"content":m.content}))
            .collect();
        let mut body = json!({
            "model":model.model,
            "messages":messages,
            "temperature":request.temperature,
            "max_tokens":request.max_output_tokens,
            "stream":false
        });
        if request.requirements.requires_structured_output {
            body["response_format"] = json!({"type":"json_object"});
        }
        let mut rb = self.client.post(url).json(&body);
        if let Some(key) = &self.cfg.api_key {
            rb = rb.bearer_auth(key);
        }
        for (k, v) in &self.cfg.extra_headers {
            rb = rb.header(k, v);
        }
        let started = Instant::now();
        let response = rb.send().await.context("model request failed")?;
        let status = response.status();
        let reported_cost = response
            .headers()
            .get("x-litellm-response-cost")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let raw = response.text().await.context("read model response")?;
        if !status.is_success() {
            anyhow::bail!(
                "provider {} returned HTTP {}. Check the API key, model access and gateway quota.",
                self.name(),
                status
            )
        }
        let parsed: CompletionResponse =
            serde_json::from_str(&raw).context("parse OpenAI-compatible response")?;
        let usage = parsed.usage.unwrap_or_default();
        let input = usage.prompt_tokens.unwrap_or(0);
        let output = usage.completion_tokens.unwrap_or(0);
        let cost = input as f64 / 1_000_000.0 * model.input_usd_per_million
            + output as f64 / 1_000_000.0 * model.output_usd_per_million;
        Ok(ModelResponse {
            provider: self.name().into(),
            model: model.model.clone(),
            text: assistant_text(&parsed.choices)?,
            input_tokens: input,
            output_tokens: output,
            latency_ms: started.elapsed().as_millis() as u64,
            cost_usd: reported_cost.unwrap_or(cost),
            provider_request_id: parsed.id,
        })
    }
}

fn assistant_text(choices: &[Choice]) -> Result<String> {
    let choice = choices
        .first()
        .context("provider returned no assistant choice")?;
    let text = match &choice.message.content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    part.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    if text.trim().is_empty() {
        if choice.finish_reason.as_deref() == Some("length") {
            anyhow::bail!(
                "provider exhausted its output token budget before returning assistant text"
            );
        }
        anyhow::bail!("provider returned no assistant text");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_text_content_accepts_strings_and_text_parts() {
        for content in [
            json!("OK"),
            json!([{"type":"text","text":"O"},{"type":"text","text":"K"}]),
        ] {
            let choices: Vec<Choice> =
                serde_json::from_value(json!([{"message":{"content":content}}])).unwrap();
            assert_eq!(assistant_text(&choices).unwrap(), "OK");
        }
    }

    #[test]
    fn empty_or_reasoning_only_responses_are_not_successful_completions() {
        let choices: Vec<Choice> = serde_json::from_value(json!([{"message":{"content":null,"reasoning_content":"private reasoning"},"finish_reason":"length"}])).unwrap();
        assert!(
            assistant_text(&choices)
                .unwrap_err()
                .to_string()
                .contains("token budget")
        );
        assert!(assistant_text(&[]).is_err());
    }
}
