use crate::ModelProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub provider_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String,String)>,
    pub models: Vec<ModelSpec>,
}

pub struct OpenAiCompatibleProvider {
    cfg: OpenAiCompatibleConfig,
    client: Client,
}
impl OpenAiCompatibleProvider {
    pub fn new(cfg:OpenAiCompatibleConfig)->Result<Self>{
        let client=Client::builder().timeout(std::time::Duration::from_secs(180)).build()?;
        Ok(Self{cfg,client})
    }
}

#[derive(Debug,Deserialize)]
struct CompletionResponse {
    id: Option<String>,
    choices: Vec<Choice>,
    usage: Option<Usage>,
}
#[derive(Debug,Deserialize)]
struct Choice { message: AssistantMessage }
#[derive(Debug,Deserialize)]
struct AssistantMessage { content: Option<String> }
#[derive(Debug,Deserialize,Default)]
struct Usage { prompt_tokens: Option<u64>, completion_tokens: Option<u64> }

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self)->&str{&self.cfg.provider_name}
    fn catalog(&self)->&[ModelSpec]{&self.cfg.models}

    async fn invoke(&self, model:&ModelSpec, request:&ModelRequest)->Result<ModelResponse>{
        let url=format!("{}/chat/completions",self.cfg.base_url.trim_end_matches('/'));
        let messages:Vec<Value>=request.messages.iter().map(|m|json!({"role":m.role,"content":m.content})).collect();
        let mut body=json!({
            "model":model.model,
            "messages":messages,
            "temperature":request.temperature,
            "max_tokens":request.max_output_tokens,
            "stream":false
        });
        if request.requirements.requires_structured_output {
            body["response_format"]=json!({"type":"json_object"});
        }
        let mut rb=self.client.post(url).json(&body);
        if let Some(key)=&self.cfg.api_key { rb=rb.bearer_auth(key); }
        for (k,v) in &self.cfg.extra_headers { rb=rb.header(k,v); }
        let started=Instant::now();
        let response=rb.send().await.context("model request failed")?;
        let status=response.status();
        let raw=response.text().await.context("read model response")?;
        if !status.is_success(){anyhow::bail!("provider {} returned {}: {}",self.name(),status,raw)}
        let parsed:CompletionResponse=serde_json::from_str(&raw).context("parse OpenAI-compatible response")?;
        let usage=parsed.usage.unwrap_or_default();
        let input=usage.prompt_tokens.unwrap_or(0);
        let output=usage.completion_tokens.unwrap_or(0);
        let cost=input as f64/1_000_000.0*model.input_usd_per_million + output as f64/1_000_000.0*model.output_usd_per_million;
        Ok(ModelResponse{
            provider:self.name().into(),model:model.model.clone(),
            text:parsed.choices.first().and_then(|c|c.message.content.clone()).unwrap_or_default(),
            input_tokens:input,output_tokens:output,latency_ms:started.elapsed().as_millis() as u64,
            cost_usd:cost,provider_request_id:parsed.id
        })
    }
}
