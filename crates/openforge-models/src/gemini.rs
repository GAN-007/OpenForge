use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<ModelSpec>,
}

pub struct GeminiProvider {
    cfg: GeminiConfig,
    client: Client,
}

impl GeminiProvider {
    pub fn new(cfg: GeminiConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self { cfg, client })
    }
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "usageMetadata")]
    usage: Option<Usage>,
    #[serde(rename = "responseId")]
    response_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Content,
}

#[derive(Debug, Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct Part {
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
    #[serde(rename = "promptTokenCount")]
    input: Option<u64>,
    #[serde(rename = "candidatesTokenCount")]
    output: Option<u64>,
}

#[async_trait]
impl ModelProvider for GeminiProvider {
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

        let contents: Vec<Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                json!({
                    "role": if m.role == "assistant" { "model" } else { "user" },
                    "parts": [{"text": m.content}]
                })
            })
            .collect();

        let mut body = json!({
            "contents": contents,
            "generationConfig": {
                "temperature": request.temperature,
                "maxOutputTokens": request.max_output_tokens
            }
        });

        if !system.is_empty() {
            body["systemInstruction"] = json!({"parts": [{"text": system}]});
        }
        if request.requirements.requires_structured_output {
            body["generationConfig"]["responseMimeType"] = json!("application/json");
        }

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.cfg.base_url.trim_end_matches('/'),
            model.model,
            self.cfg.api_key
        );

        let started = Instant::now();
        let res = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .context("Gemini request failed")?;

        let status = res.status();
        let raw = res.text().await.context("read Gemini response")?;
        if !status.is_success() {
            anyhow::bail!("Gemini returned {status}: {raw}");
        }

        let parsed: Response = serde_json::from_str(&raw).context("parse Gemini response")?;
        let usage = parsed.usage.unwrap_or_default();
        let input = usage.input.unwrap_or(0);
        let output = usage.output.unwrap_or(0);
        let text = parsed
            .candidates
            .into_iter()
            .flat_map(|c| c.content.parts)
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        let cost = input as f64 / 1_000_000.0 * model.input_usd_per_million
            + output as f64 / 1_000_000.0 * model.output_usd_per_million;

        Ok(ModelResponse {
            provider: self.name().into(),
            model: model.model.clone(),
            text,
            input_tokens: input,
            output_tokens: output,
            latency_ms: started.elapsed().as_millis() as u64,
            cost_usd: cost,
            provider_request_id: parsed.response_id,
        })
    }
}
