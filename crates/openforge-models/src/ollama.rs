use anyhow::{Context, Result, bail};
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, fs, time::Instant};

const DEFAULT_KEEP_ALIVE: &str = "30m";
const DEFAULT_MEMORY_HEADROOM_MB: u64 = 512;
const MIB: u64 = 1024 * 1024;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone)]
pub(crate) struct OllamaNativeAdapter {
    base_url: String,
    client: Client,
    keep_alive: String,
    num_ctx: Option<u32>,
    num_gpu: Option<i32>,
    memory_headroom_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TaggedModel>,
}

#[derive(Debug, Deserialize)]
struct TaggedModel {
    name: String,
    model: Option<String>,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    model: Option<String>,
    created_at: Option<String>,
    message: ChatResponseMessage,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: u64,
    #[serde(default)]
    eval_count: u64,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: String,
}

impl OllamaNativeAdapter {
    pub(crate) fn detect(base_url: &str, client: Client) -> Option<Self> {
        let mode = env::var("OPENFORGE_OLLAMA_NATIVE")
            .unwrap_or_else(|_| "auto".to_string())
            .to_ascii_lowercase();
        if matches!(mode.as_str(), "0" | "false" | "off" | "disabled") {
            return None;
        }

        let mut url = Url::parse(base_url).ok()?;
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url.port_or_known_default()?;
        let path = url.path().trim_end_matches('/');
        let forced = matches!(mode.as_str(), "1" | "true" | "on" | "enabled");
        let local_ollama = matches!(
            host.as_str(),
            "127.0.0.1" | "localhost" | "::1" | "host.docker.internal"
        ) && port == 11434;

        if (!local_ollama && !forced) || (path != "/v1" && !forced) {
            return None;
        }

        url.set_path("/");
        url.set_query(None);
        url.set_fragment(None);
        let base_url = url.as_str().trim_end_matches('/').to_string();
        Some(Self::new(base_url, client))
    }

    fn new(base_url: String, client: Client) -> Self {
        Self {
            base_url,
            client,
            keep_alive: env::var("OPENFORGE_OLLAMA_KEEP_ALIVE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_KEEP_ALIVE.to_string()),
            num_ctx: env::var("OPENFORGE_OLLAMA_NUM_CTX")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| *value > 0),
            num_gpu: env::var("OPENFORGE_OLLAMA_NUM_GPU")
                .ok()
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|value| *value >= 0),
            memory_headroom_bytes: env::var("OPENFORGE_OLLAMA_MEMORY_HEADROOM_MB")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_MEMORY_HEADROOM_MB)
                .saturating_mul(MIB),
        }
    }

    #[cfg(test)]
    fn for_test(base_url: String, client: Client) -> Self {
        Self {
            base_url,
            client,
            keep_alive: "45m".into(),
            num_ctx: Some(8192),
            num_gpu: Some(0),
            memory_headroom_bytes: 0,
        }
    }

    pub(crate) async fn invoke(
        &self,
        provider_name: &str,
        model: &ModelSpec,
        request: &ModelRequest,
    ) -> Result<Option<ModelResponse>> {
        let tagged = match self.find_installed_model(&model.model).await? {
            Some(model) => model,
            None => return Ok(None),
        };
        self.enforce_memory_guard(model, request, tagged.size)?;

        let url = format!("{}/api/chat", self.base_url);
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content}))
            .collect();

        let requested_context = self
            .num_ctx
            .unwrap_or(request.requirements.context_tokens)
            .min(model.context_tokens)
            .max(1);
        let mut options = json!({
            "temperature": request.temperature,
            "num_predict": request.max_output_tokens,
            "num_ctx": requested_context,
        });
        if let Some(num_gpu) = self.num_gpu {
            options["num_gpu"] = json!(num_gpu);
        }

        let mut body = json!({
            "model": model.model,
            "messages": messages,
            "stream": false,
            "keep_alive": self.keep_alive,
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
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Ollama native /api/chat request failed")?;
        let status = response.status();
        let raw = response.text().await.context("read Ollama response")?;
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
        ) {
            return Ok(None);
        }
        if !status.is_success() {
            bail!(
                "Ollama native request for {} returned HTTP {}. {}",
                model.model,
                status,
                ollama_error_hint(status, &raw)
            );
        }

        let parsed: ChatResponse =
            serde_json::from_str(&raw).context("parse Ollama native response")?;
        let text = parsed.message.content;
        if text.trim().is_empty() {
            if parsed.done_reason.as_deref() == Some("length") {
                bail!("Ollama exhausted the output token budget before returning assistant text");
            }
            bail!("Ollama returned no assistant text");
        }

        let cost = parsed.prompt_eval_count as f64 / 1_000_000.0 * model.input_usd_per_million
            + parsed.eval_count as f64 / 1_000_000.0 * model.output_usd_per_million;
        Ok(Some(ModelResponse {
            provider: provider_name.to_string(),
            model: parsed.model.unwrap_or_else(|| model.model.clone()),
            text,
            input_tokens: parsed.prompt_eval_count,
            output_tokens: parsed.eval_count,
            latency_ms: started.elapsed().as_millis() as u64,
            cost_usd: cost,
            provider_request_id: parsed.created_at,
        }))
    }

    async fn find_installed_model(&self, requested: &str) -> Result<Option<TaggedModel>> {
        let response = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .context("Ollama model preflight failed: cannot reach /api/tags")?;
        let status = response.status();
        let raw = response.text().await.context("read Ollama model catalog")?;
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
        ) {
            return Ok(None);
        }
        if !status.is_success() {
            bail!(
                "Ollama model preflight returned HTTP {}. {}",
                status,
                ollama_error_hint(status, &raw)
            );
        }
        let tags: TagsResponse = serde_json::from_str(&raw).context("parse Ollama /api/tags")?;
        let tagged = tags
            .models
            .into_iter()
            .find(|candidate| model_name_matches(requested, candidate));
        tagged.with_context(|| {
            format!(
                "Ollama model {requested} is configured in OpenForge but is not installed; run `ollama pull {requested}`"
            )
        }).map(Some)
    }

    fn enforce_memory_guard(
        &self,
        model: &ModelSpec,
        request: &ModelRequest,
        model_size_bytes: u64,
    ) -> Result<()> {
        if model_size_bytes == 0 {
            return Ok(());
        }
        let Some(available) = available_memory_bytes() else {
            return Ok(());
        };
        let context_tokens = self
            .num_ctx
            .unwrap_or(request.requirements.context_tokens)
            .min(model.context_tokens) as u64;
        let kv_cache_estimate = context_tokens.saturating_mul(8 * 1024);
        let runtime_overhead = model_size_bytes / 10;
        let required = model_size_bytes
            .saturating_add(runtime_overhead)
            .saturating_add(kv_cache_estimate)
            .saturating_add(self.memory_headroom_bytes);
        if available < required {
            bail!(
                "Ollama preflight rejected {}: estimated {:.1} GiB required for model + context + headroom, but only {:.1} GiB is available; OpenForge will try the next eligible fallback model",
                model.model,
                required as f64 / GIB,
                available as f64 / GIB,
            );
        }
        Ok(())
    }
}

fn model_name_matches(requested: &str, candidate: &TaggedModel) -> bool {
    candidate.name == requested
        || candidate.model.as_deref() == Some(requested)
        || (!requested.contains(':') && candidate.name == format!("{requested}:latest"))
        || (!requested.contains(':')
            && candidate.model.as_deref() == Some(&format!("{requested}:latest")))
}

fn available_memory_bytes() -> Option<u64> {
    let raw = fs::read_to_string("/proc/meminfo").ok()?;
    raw.lines().find_map(|line| {
        let value = line.strip_prefix("MemAvailable:")?.trim();
        let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(kib.saturating_mul(1024))
    })
}

fn ollama_error_hint(status: StatusCode, raw: &str) -> &'static str {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("pull") {
        "The selected model is not available locally; pull it with `ollama pull <model>`."
    } else if lower.contains("memory") || lower.contains("system memory") {
        "The model does not fit available memory; use a smaller model or free RAM."
    } else if status.is_server_error() {
        "The local Ollama service or model failed while handling the request."
    } else {
        "The local Ollama service rejected the request."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use openforge_protocol::{ChatMessage, DataClassification, ModelRequirements};
    use std::sync::{Arc, Mutex};

    fn spec(model: &str) -> ModelSpec {
        ModelSpec {
            provider: "local".into(),
            model: model.into(),
            family: "qwen".into(),
            context_tokens: 32_768,
            supports_tools: true,
            supports_vision: false,
            supports_structured_output: true,
            input_usd_per_million: 0.0,
            output_usd_per_million: 0.0,
            latency_score: 0.35,
            quality_score: 0.78,
            privacy_score: 1.0,
            max_data_classification: DataClassification::Restricted,
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            invocation_id: uuid::Uuid::new_v4(),
            run_id: uuid::Uuid::new_v4(),
            task_id: None,
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "reply json".into(),
            }],
            requirements: ModelRequirements {
                task_class: "test".into(),
                context_tokens: 4096,
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
            response_schema: Some(json!({"type":"object"})),
        }
    }

    #[tokio::test]
    async fn native_adapter_maps_keep_alive_context_gpu_and_structured_format() {
        let captured = Arc::new(Mutex::new(Value::Null));
        let seen = captured.clone();
        let app = Router::new()
            .route("/api/tags", get(|| async {
                Json(json!({"models":[{"name":"qwen2.5-coder:7b","model":"qwen2.5-coder:7b","size":1}]}))
            }))
            .route("/api/chat", post(move |Json(body): Json<Value>| {
                let seen = seen.clone();
                async move {
                    *seen.lock().unwrap() = body;
                    Json(json!({
                        "model":"qwen2.5-coder:7b",
                        "created_at":"2026-09-23T00:00:00Z",
                        "message":{"role":"assistant","content":"{\"ok\":true}"},
                        "done":true,
                        "prompt_eval_count":7,
                        "eval_count":4
                    }))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::builder().build().unwrap();
        let adapter = OllamaNativeAdapter::for_test(format!("http://{address}"), client);

        let response = adapter
            .invoke("local", &spec("qwen2.5-coder:7b"), &request())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.text, "{\"ok\":true}");
        assert_eq!(response.input_tokens, 7);
        assert_eq!(response.output_tokens, 4);
        let body = captured.lock().unwrap().clone();
        assert_eq!(body["keep_alive"], "45m");
        assert_eq!(body["options"]["num_ctx"], 8192);
        assert_eq!(body["options"]["num_gpu"], 0);
        assert_eq!(body["format"], json!({"type":"object"}));
        server.abort();
    }

    #[tokio::test]
    async fn missing_configured_model_fails_preflight_before_chat() {
        let app = Router::new().route("/api/tags", get(|| async { Json(json!({"models":[]})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let adapter = OllamaNativeAdapter::for_test(
            format!("http://{address}"),
            Client::builder().build().unwrap(),
        );
        let error = adapter
            .invoke("local", &spec("missing:7b"), &request())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ollama pull missing:7b"), "{error}");
        server.abort();
    }
}
