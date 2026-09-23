use crate::{ModelProvider, ollama::OllamaNativeAdapter};
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
    ollama_native: Option<OllamaNativeAdapter>,
}
impl OpenAiCompatibleProvider {
    pub fn new(cfg: OpenAiCompatibleConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        let ollama_native = OllamaNativeAdapter::detect(&cfg.base_url, client.clone());
        Ok(Self {
            cfg,
            client,
            ollama_native,
        })
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
        // Ollama exposes an OpenAI-compatible surface, but its native endpoint carries
        // local-runtime controls the shim cannot express (keep_alive, num_ctx, num_gpu)
        // and lets us reject impossible loads before paying the model startup cost.
        // When a local :11434/v1 endpoint is detected, prefer that richer path. If the
        // native endpoint itself is unavailable, fall through to the generic shim.
        if let Some(ollama) = &self.ollama_native
            && let Some(response) = ollama.invoke(self.name(), model, request).await?
        {
            return Ok(response);
        }

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
        let started = Instant::now();
        let mut corrections = 0;
        let (raw, reported_cost) = loop {
            let mut rb = self.client.post(&url).json(&body);
            if let Some(key) = &self.cfg.api_key {
                rb = rb.bearer_auth(key);
            }
            for (key, value) in &self.cfg.extra_headers {
                rb = rb.header(key, value);
            }
            let response = rb.send().await.context("model request failed")?;
            let status = response.status();
            let reported_cost = response
                .headers()
                .get("x-litellm-response-cost")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0);
            let raw = response.text().await.context("read model response")?;
            if status.is_success() {
                break (raw, reported_cost);
            }
            // Retry only explicit parameter validation errors. Never retry auth,
            // quota, context-length, server or arbitrary bad-request failures.
            if status == reqwest::StatusCode::BAD_REQUEST
                && corrections < 3
                && adapt_unsupported_parameter(&mut body, &raw)
            {
                corrections += 1;
                continue;
            }
            anyhow::bail!(
                "provider {} returned HTTP {}. {}",
                self.name(),
                status,
                safe_error_hint(status.as_u16(), &raw)
            );
        };
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

fn error_details(raw: &str) -> (String, String, String) {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let error = value.get("error").unwrap_or(&value);
    (
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase(),
        error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase(),
        error
            .get("param")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase(),
    )
}

fn adapt_unsupported_parameter(body: &mut Value, raw: &str) -> bool {
    let (message, code, param) = error_details(raw);
    let unsupported = code == "unsupported_parameter"
        || code == "unsupported_value"
        || [
            "unsupported",
            "not supported",
            "does not support",
            "not support",
            "only the default",
        ]
        .iter()
        .any(|word| message.contains(word));
    if !unsupported {
        return false;
    }
    for name in ["response_format", "temperature", "max_tokens"] {
        if (param == name || message.contains(name)) && body.get(name).is_some() {
            let value = body
                .as_object_mut()
                .expect("request object")
                .remove(name)
                .unwrap();
            if name == "max_tokens" {
                // Preserve the output cap when the selected reasoning model uses the newer name.
                body["max_completion_tokens"] = value;
            }
            return true;
        }
    }
    false
}

fn safe_error_hint(status: u16, raw: &str) -> &'static str {
    let (message, code, _) = error_details(raw);
    match status {
        401 => "Authentication failed. Reconnect the gateway with a valid key.",
        403 => "The gateway denied access to the selected model.",
        429 => "The gateway rate limit or quota was reached. Retry later or check gateway limits.",
        400 if code.contains("context_length")
            || message.contains("context length")
            || message.contains("context window") =>
        {
            "The selected model rejected the context length. Reduce the task context or select a larger-context model."
        }
        400 => {
            "The selected model rejected the request. Recognized unsupported parameters are retried automatically; check gateway request validation for other constraints."
        }
        500..=599 => "The gateway or selected model is temporarily unavailable.",
        _ => "The gateway rejected the model request.",
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn only_known_unsupported_parameters_are_adapted_and_output_cap_is_preserved() {
        let mut body = json!({"temperature":0.1,"max_tokens":3500,"response_format":{"type":"json_object"},"messages":[{"role":"user","content":"Return JSON"}]});
        assert!(!adapt_unsupported_parameter(
            &mut body,
            r#"{"error":{"message":"context length exceeded","code":"context_length_exceeded"}}"#
        ));
        assert!(!adapt_unsupported_parameter(
            &mut body,
            r#"{"error":{"message":"invalid request"}}"#
        ));
        for param in ["response_format", "temperature", "max_tokens"] {
            assert!(adapt_unsupported_parameter(
                &mut body,
                &json!({"error":{"code":"unsupported_parameter","param":param}}).to_string()
            ));
            assert!(body.get(param).is_none());
        }
        assert_eq!(body["max_completion_tokens"], 3500);
        assert_eq!(body["messages"][0]["content"], "Return JSON");
        assert!(!adapt_unsupported_parameter(
            &mut body,
            r#"{"error":{"code":"unsupported_parameter","param":"max_tokens"}}"#
        ));
        assert!(!safe_error_hint(400, "private-api-key").contains("private-api-key"));
    }

    #[tokio::test]
    async fn completion_recovers_from_model_parameter_rejection_without_retrying_auth_failures() {
        for authorized in [true, false] {
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let app = Router::new().route("/chat/completions", post(move |Json(body): Json<Value>| {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if !authorized {return (StatusCode::UNAUTHORIZED, Json(json!({"error":{"message":"unsupported temperature private-key"}})));}
                    for param in ["response_format","temperature","max_tokens"] {
                        if body.get(param).is_some() {
                            return (StatusCode::BAD_REQUEST, Json(json!({"error":{"code":"unsupported_parameter","param":param}})));
                        }
                    }
                    assert_eq!(body["max_completion_tokens"], 3500);
                    (StatusCode::OK, Json(json!({"choices":[{"message":{"content":"{\"action\":{\"type\":\"finish\",\"success\":true}}"}}]})))
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let model: ModelSpec = serde_json::from_value(json!({"provider":"sevi","model":"auto-select","family":"sevi","context_tokens":32768,"supports_tools":false,"supports_vision":false,"supports_structured_output":true,"input_usd_per_million":0.0,"output_usd_per_million":0.0,"latency_score":0.1,"quality_score":0.8,"privacy_score":0.5,"max_data_classification":"INTERNAL"})).unwrap();
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                provider_name: "sevi".into(),
                base_url: format!("http://{address}"),
                api_key: Some("fake-key".into()),
                extra_headers: vec![],
                models: vec![model.clone()],
            })
            .unwrap();
            let request: ModelRequest = serde_json::from_value(json!({"invocation_id":uuid::Uuid::new_v4(),"run_id":uuid::Uuid::new_v4(),"task_id":null,"messages":[{"role":"user","content":"Return JSON"}],"requirements":{"task_class":"test","context_tokens":100,"requires_tools":false,"requires_vision":false,"requires_structured_output":true,"max_cost_usd":1.0,"max_latency_ms":null,"data_classification":"PUBLIC","preferred_model_families":[],"excluded_model_families":[]},"temperature":0.1,"max_output_tokens":3500,"response_schema":null})).unwrap();
            let result = provider.invoke(&model, &request).await;
            if authorized {
                assert!(result.is_ok());
                assert_eq!(calls.load(Ordering::SeqCst), 4);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("Authentication failed")
                );
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
            server.abort();
        }
    }
}
