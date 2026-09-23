use crate::{ModelPreflight, ModelProvider};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, time::Instant};

#[derive(Clone, Debug)]
pub struct OllamaConfig {
    pub provider_name: String,
    pub base_url: String,
    pub models: Vec<ModelSpec>,
    pub keep_alive: String,
    pub num_ctx: Option<u32>,
    pub num_gpu: Option<i32>,
}

pub struct OllamaProvider {
    cfg: OllamaConfig,
    client: Client,
    local_endpoint: bool,
}

#[derive(Debug, Deserialize)]
struct TagList {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
    model: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RunningList {
    #[serde(default)]
    models: Vec<RunningModel>,
}

#[derive(Debug, Deserialize)]
struct RunningModel {
    name: String,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    model: Option<String>,
    message: OllamaMessage,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    done_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: String,
}

impl OllamaProvider {
    pub fn new(cfg: OllamaConfig) -> Result<Self> {
        if cfg.provider_name.trim().is_empty() {
            bail!("Ollama provider name cannot be empty");
        }
        if cfg.models.is_empty() {
            bail!("Ollama provider {} has no models", cfg.provider_name);
        }
        if cfg.keep_alive.trim().is_empty() {
            bail!("Ollama keep_alive cannot be empty");
        }
        let url = Url::parse(&cfg.base_url).context("parse Ollama base_url")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("Ollama base_url must use http or https");
        }
        if url.username() != "" || url.password().is_some() {
            bail!("Ollama base_url must not embed credentials");
        }
        let local_endpoint = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self {
            cfg,
            client,
            local_endpoint,
        })
    }

    fn api_url(&self, path: &str) -> String {
        let base = self.cfg.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        format!("{base}{path}")
    }

    async fn tags(&self) -> Result<TagList> {
        let response = self
            .client
            .get(self.api_url("/api/tags"))
            .send()
            .await
            .context("connect to Ollama")?;
        let status = response.status();
        if !status.is_success() {
            bail!("Ollama model catalog returned HTTP {status}");
        }
        response.json().await.context("parse Ollama model catalog")
    }

    async fn running(&self) -> Option<RunningList> {
        let response = self.client.get(self.api_url("/api/ps")).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json().await.ok()
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.cfg.provider_name
    }

    fn catalog(&self) -> &[ModelSpec] {
        &self.cfg.models
    }

    async fn preflight(&self, model: &ModelSpec) -> Result<ModelPreflight> {
        let tags = match self.tags().await {
            Ok(tags) => tags,
            Err(error) => {
                return Ok(ModelPreflight {
                    provider: model.provider.clone(),
                    model: model.model.clone(),
                    ready: false,
                    endpoint_reachable: Some(false),
                    installed: None,
                    loaded: None,
                    available_memory_mb: local_available_memory_mb(self.local_endpoint),
                    required_memory_mb: None,
                    detail: Some(format!("Ollama is not reachable: {error}")),
                });
            }
        };

        let installed = tags.models.iter().find(|candidate| {
            candidate.name == model.model
                || candidate.model.as_deref() == Some(model.model.as_str())
        });
        let Some(installed_model) = installed else {
            return Ok(ModelPreflight {
                provider: model.provider.clone(),
                model: model.model.clone(),
                ready: false,
                endpoint_reachable: Some(true),
                installed: Some(false),
                loaded: Some(false),
                available_memory_mb: local_available_memory_mb(self.local_endpoint),
                required_memory_mb: None,
                detail: Some(format!(
                    "model {} is not installed; run: ollama pull {}",
                    model.model, model.model
                )),
            });
        };

        let loaded = self
            .running()
            .await
            .map(|list| {
                list.models.iter().any(|candidate| {
                    candidate.name == model.model
                        || candidate.model.as_deref() == Some(model.model.as_str())
                })
            })
            .unwrap_or(false);
        let available_memory_mb = local_available_memory_mb(self.local_endpoint);
        let required_memory_mb = installed_model.size.map(required_memory_mb_for_model);

        if !loaded
            && let (Some(available), Some(required)) = (available_memory_mb, required_memory_mb)
            && available < required
        {
            return Ok(ModelPreflight {
                provider: model.provider.clone(),
                model: model.model.clone(),
                ready: false,
                endpoint_reachable: Some(true),
                installed: Some(true),
                loaded: Some(false),
                available_memory_mb: Some(available),
                required_memory_mb: Some(required),
                detail: Some(format!(
                    "insufficient free RAM for {}: {} MiB available, approximately {} MiB required",
                    model.model, available, required
                )),
            });
        }

        Ok(ModelPreflight {
            provider: model.provider.clone(),
            model: model.model.clone(),
            ready: true,
            endpoint_reachable: Some(true),
            installed: Some(true),
            loaded: Some(loaded),
            available_memory_mb,
            required_memory_mb,
            detail: Some(if loaded {
                "model is installed and already loaded".into()
            } else {
                "model is installed and passed local memory preflight".into()
            }),
        })
    }

    async fn invoke(&self, model: &ModelSpec, request: &ModelRequest) -> Result<ModelResponse> {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content}))
            .collect();
        let mut options = json!({
            "temperature": request.temperature,
            "num_ctx": self.cfg.num_ctx.unwrap_or(model.context_tokens),
        });
        if let Some(num_gpu) = self.cfg.num_gpu {
            options["num_gpu"] = json!(num_gpu);
        }

        let mut body = json!({
            "model": model.model,
            "messages": messages,
            "stream": false,
            "keep_alive": self.cfg.keep_alive,
            "options": options,
        });
        if request.requirements.requires_structured_output {
            body["format"] = request
                .response_schema
                .clone()
                .unwrap_or_else(|| json!("json"));
        }

        let started = Instant::now();
        let response = self
            .client
            .post(self.api_url("/api/chat"))
            .json(&body)
            .send()
            .await
            .context("Ollama chat request failed")?;
        let status = response.status();
        let raw = response.text().await.context("read Ollama response")?;
        if !status.is_success() {
            let error = serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "Ollama rejected the request".into());
            bail!("Ollama returned HTTP {status}: {error}");
        }

        let parsed: ChatResponse =
            serde_json::from_str(&raw).context("parse Ollama chat response")?;
        let text = parsed.message.content;
        if text.trim().is_empty() {
            if parsed.done_reason.as_deref() == Some("length") {
                bail!("Ollama exhausted the output budget before returning assistant text");
            }
            bail!("Ollama returned no assistant text");
        }
        let input = parsed.prompt_eval_count.unwrap_or(0);
        let output = parsed.eval_count.unwrap_or(0);
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
            provider_request_id: parsed.model,
        })
    }
}

fn local_available_memory_mb(local_endpoint: bool) -> Option<u64> {
    if !local_endpoint {
        return None;
    }
    let raw = fs::read_to_string("/proc/meminfo").ok()?;
    raw.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        rest.split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|kb| kb / 1024)
    })
}

fn required_memory_mb_for_model(size_bytes: u64) -> u64 {
    let size_mb = size_bytes.div_ceil(1024 * 1024);
    size_mb
        .saturating_mul(115)
        .div_ceil(100)
        .saturating_add(512)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use openforge_protocol::{DataClassification, ModelRequirements};
    use uuid::Uuid;

    fn model(base_provider: &str) -> ModelSpec {
        ModelSpec {
            provider: base_provider.into(),
            model: "qwen-test:latest".into(),
            family: "qwen".into(),
            context_tokens: 8192,
            supports_tools: true,
            supports_vision: false,
            supports_structured_output: true,
            input_usd_per_million: 0.0,
            output_usd_per_million: 0.0,
            latency_score: 0.2,
            quality_score: 0.8,
            privacy_score: 1.0,
            max_data_classification: DataClassification::Restricted,
        }
    }

    #[tokio::test]
    async fn native_ollama_preflight_and_chat_round_trip() {
        let app = Router::new()
            .route("/api/tags", get(|| async {
                Json(json!({"models":[{"name":"qwen-test:latest","model":"qwen-test:latest","size":1048576}]}))
            }))
            .route("/api/ps", get(|| async {
                Json(json!({"models":[{"name":"qwen-test:latest","model":"qwen-test:latest"}]}))
            }))
            .route("/api/chat", post(|Json(body): Json<Value>| async move {
                assert_eq!(body["stream"], false);
                assert_eq!(body["keep_alive"], "10m");
                assert_eq!(body["options"]["num_ctx"], 4096);
                assert_eq!(body["format"], "json");
                Json(json!({
                    "model":"qwen-test:latest",
                    "message":{"role":"assistant","content":"{\"ok\":true}"},
                    "prompt_eval_count":11,
                    "eval_count":4,
                    "done":true
                }))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let spec = model("local");
        let provider = OllamaProvider::new(OllamaConfig {
            provider_name: "local".into(),
            base_url: format!("http://{address}"),
            models: vec![spec.clone()],
            keep_alive: "10m".into(),
            num_ctx: Some(4096),
            num_gpu: Some(0),
        })
        .unwrap();
        let preflight = provider.preflight(&spec).await.unwrap();
        assert!(preflight.ready);
        assert_eq!(preflight.installed, Some(true));

        let request = ModelRequest {
            invocation_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            task_id: None,
            messages: vec![openforge_protocol::ChatMessage {
                role: "user".into(),
                content: "reply as JSON".into(),
            }],
            requirements: ModelRequirements {
                task_class: "test".into(),
                context_tokens: 100,
                requires_tools: false,
                requires_vision: false,
                requires_structured_output: true,
                max_cost_usd: 1.0,
                max_latency_ms: None,
                data_classification: DataClassification::Internal,
                preferred_model_families: vec![],
                excluded_model_families: vec![],
            },
            temperature: 0.0,
            max_output_tokens: 32,
            response_schema: None,
        };
        let response = provider.invoke(&spec, &request).await.unwrap();
        assert_eq!(response.text, "{\"ok\":true}");
        assert_eq!(response.input_tokens, 11);
        assert_eq!(response.output_tokens, 4);
        server.abort();
    }

    #[test]
    fn memory_guard_adds_runtime_headroom() {
        assert_eq!(required_memory_mb_for_model(1024 * 1024 * 1024), 1690);
    }
}
