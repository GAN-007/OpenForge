use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use serde_json::{Value, json};
use std::time::Instant;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct BedrockCliConfig {
    pub provider_name: String,
    pub region: String,
    pub models: Vec<ModelSpec>,
}

pub struct BedrockCliProvider {
    cfg: BedrockCliConfig,
}

impl BedrockCliProvider {
    pub fn new(cfg: BedrockCliConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl ModelProvider for BedrockCliProvider {
    fn name(&self) -> &str {
        &self.cfg.provider_name
    }

    fn catalog(&self) -> &[ModelSpec] {
        &self.cfg.models
    }

    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse> {
        let system: Vec<Value> = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| json!({"text": m.content}))
            .collect();

        let messages: Vec<Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                json!({
                    "role": if m.role == "assistant" { "assistant" } else { "user" },
                    "content": [{"text": m.content}]
                })
            })
            .collect();

        let mut args = vec![
            "bedrock-runtime".to_string(),
            "converse".into(),
            "--region".into(),
            self.cfg.region.clone(),
            "--model-id".into(),
            model.model.clone(),
            "--messages".into(),
            serde_json::to_string(&messages)?,
            "--inference-config".into(),
            serde_json::to_string(&json!({
                "maxTokens": request.max_output_tokens,
                "temperature": request.temperature
            }))?,
            "--output".into(),
            "json".into(),
        ];

        if !system.is_empty() {
            args.push("--system".into());
            args.push(serde_json::to_string(&system)?);
        }

        let started = Instant::now();
        let out = Command::new("aws")
            .args(&args)
            .output()
            .await
            .context("execute AWS CLI for Bedrock")?;

        if !out.status.success() {
            anyhow::bail!(
                "AWS Bedrock CLI failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let v: Value = serde_json::from_slice(&out.stdout).context("parse Bedrock response")?;
        let text = v
            .pointer("/output/message/content")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let input = v
            .pointer("/usage/inputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = v
            .pointer("/usage/outputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);

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
