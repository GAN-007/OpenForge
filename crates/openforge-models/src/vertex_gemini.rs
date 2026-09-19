use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct VertexGeminiConfig {
    pub provider_name: String,
    pub project: String,
    pub location: String,
    pub access_token: String,
    pub base_url: Option<String>,
    pub models: Vec<ModelSpec>,
}

pub struct VertexGeminiProvider {
    cfg: VertexGeminiConfig,
    client: Client,
}

impl VertexGeminiProvider {
    pub fn new(cfg: VertexGeminiConfig) -> Result<Self> {
        if cfg.project.trim().is_empty()
            || cfg.location.trim().is_empty()
            || cfg.access_token.trim().is_empty()
        {
            anyhow::bail!("Vertex project, location and access token are required");
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
struct Response {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "usageMetadata")]
    usage: Option<Usage>,
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
impl ModelProvider for VertexGeminiProvider {
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
        let system = request
            .messages
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let contents = request
            .messages
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| {
                json!({
                    "role": if message.role == "assistant" { "model" } else { "user" },
                    "parts": [{"text": message.content}]
                })
            })
            .collect::<Vec<_>>();
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

        let base = self.cfg.base_url.clone().unwrap_or_else(|| {
            format!(
                "https://{}-aiplatform.googleapis.com/v1",
                self.cfg.location
            )
        });
        let url = format!(
            "{}/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
            base.trim_end_matches('/'),
            self.cfg.project,
            self.cfg.location,
            model.model
        );
        let started = Instant::now();
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.cfg.access_token)
            .json(&body)
            .send()
            .await
            .context("Vertex Gemini request failed")?;
        let status = response.status();
        let raw = response
            .text()
            .await
            .context("read Vertex Gemini response")?;
        if !status.is_success() {
            anyhow::bail!("Vertex Gemini returned {status}: {raw}");
        }
        let parsed: Response =
            serde_json::from_str(&raw).context("parse Vertex Gemini response")?;
        let usage = parsed.usage.unwrap_or_default();
        let input = usage.input.unwrap_or(0);
        let output = usage.output.unwrap_or(0);
        let text = parsed
            .candidates
            .into_iter()
            .flat_map(|candidate| candidate.content.parts)
            .filter_map(|part| part.text)
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
            provider_request_id: None,
        })
    }
}
