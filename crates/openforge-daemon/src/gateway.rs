//! Sevi connection settings. Only an owner-protected user file persists credentials.
mod credentials;
use anyhow::{Result, bail};
use openforge_core::Engine;
use openforge_models::{ModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use openforge_protocol::{
    ChatMessage, DataClassification, ModelRequest, ModelRequirements, ModelSpec,
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

const BASE_URL: &str = "https://model.sevi.io/cursor";
const MODEL: &str = "auto-select";

pub(super) struct GatewaySettings {
    credentials: credentials::CredentialFile,
    saved: std::sync::atomic::AtomicBool,
    mutation: tokio::sync::Mutex<()>,
}

impl GatewaySettings {
    pub(super) fn new(path: std::path::PathBuf) -> Self {
        Self {
            credentials: credentials::CredentialFile::new(path),
            saved: false.into(),
            mutation: tokio::sync::Mutex::new(()),
        }
    }

    pub(super) fn restore(&self, engine: &Engine) -> Result<bool> {
        let Some(key) = self.credentials.read()? else {
            return Ok(false);
        };
        engine.set_provider_override(Some(provider_for_key(&key, BASE_URL)?));
        self.saved.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(true)
    }

    pub(super) fn status(&self, engine: &Engine) -> Value {
        json!({
            "connected": engine.has_provider_override(), "base_url": BASE_URL, "model": MODEL,
            "credential_storage": "user_config_file",
            "credential_persisted": self.saved.load(std::sync::atomic::Ordering::SeqCst),
            "pricing": "gateway_reported_or_unpriced"
        })
    }

    pub(super) async fn connect(&self, engine: &Engine, params: &Value) -> Result<Value> {
        let key = params
            .get("api_key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        self.connect_at(engine, key, BASE_URL).await
    }

    async fn connect_at(&self, engine: &Engine, key: &str, base_url: &str) -> Result<Value> {
        let _guard = self.mutation.lock().await;
        let provider = verified_provider(key, base_url).await?;
        // Persist before switching: a failed save must not claim a remembered connection.
        self.credentials.save(key)?;
        engine.set_provider_override(Some(provider));
        self.saved.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(self.status(engine))
    }

    pub(super) async fn disconnect(&self, engine: &Engine) -> Result<Value> {
        let _guard = self.mutation.lock().await;
        self.credentials.remove()?;
        engine.set_provider_override(None);
        self.saved.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(self.status(engine))
    }
}

pub(super) fn credential_path() -> Result<std::path::PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|value| std::path::PathBuf::from(value).join(".config"))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("Set HOME or XDG_CONFIG_HOME for private gateway credential storage")
        })?;
    if !root.is_absolute() {
        bail!("Gateway configuration directory must be an absolute path");
    }
    Ok(root.join("openforge").join("sevi-gateway.key"))
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 16_384 || !key.bytes().all(|byte| (33..=126).contains(&byte)) {
        bail!("Paste the actual gateway API key, not the masked placeholder.");
    }
    Ok(())
}

fn provider_for_key(key: &str, base_url: &str) -> Result<Arc<dyn ModelProvider>> {
    validate_key(key)?;
    let model = ModelSpec {
        provider: "sevi".into(),
        model: MODEL.into(),
        family: "sevi-gateway".into(),
        // Client routing defaults, not a promise about the model chosen by Sevi.
        context_tokens: 32_768,
        supports_tools: false,
        supports_vision: false,
        supports_structured_output: true,
        input_usd_per_million: 0.0,
        output_usd_per_million: 0.0,
        latency_score: 0.05,
        quality_score: 0.8,
        privacy_score: 0.5,
        max_data_classification: DataClassification::Internal,
    };
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        provider_name: "sevi".into(),
        base_url: base_url.into(),
        api_key: Some(key.into()),
        extra_headers: vec![],
        models: vec![model.clone()],
    })?;
    Ok(Arc::new(provider))
}

async fn verified_provider(key: &str, base_url: &str) -> Result<Arc<dyn ModelProvider>> {
    let provider = provider_for_key(key, base_url)?;
    let model = provider.catalog().first().expect("Sevi model preset");
    // Authentication does not depend on how an auto-routed model formats JSON.
    // This user-triggered setup request contains no repository or conversation data.
    let request = ModelRequest {
        invocation_id: Uuid::new_v4(),
        run_id: Uuid::nil(),
        task_id: None,
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Connection test. Reply briefly with OK.".into(),
        }],
        requirements: ModelRequirements {
            task_class: "gateway_setup".into(),
            context_tokens: 128,
            requires_tools: false,
            requires_vision: false,
            requires_structured_output: false,
            max_cost_usd: f64::MAX,
            max_latency_ms: None,
            data_classification: DataClassification::Public,
            preferred_model_families: vec![],
            excluded_model_families: vec![],
        },
        temperature: 0.0,
        max_output_tokens: 512,
        response_schema: None,
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.invoke(model, &request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Gateway connection timed out. Check access and try again."))??;
    if response.text.trim().is_empty() {
        bail!(
            "Gateway accepted the request but returned no assistant text. Check the model's output budget and access."
        );
    }
    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn gateway_uses_exact_path_model_and_bearer_then_routes_real_engine_calls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route("/cursor/chat/completions", post(|State(calls): State<Arc<AtomicUsize>>, headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer test-key");
            assert_eq!(body["model"], "auto-select");
            calls.fetch_add(1, Ordering::SeqCst);
            ([("x-litellm-response-cost", "0.0123")], Json(json!({"choices":[{"message":{"content":"{\"ok\":true}"}}],"usage":{"prompt_tokens":5,"completion_tokens":4}})))
        })).with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/cursor", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = verified_provider("test-key", &url).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut config = openforge_core::OpenForgeConfig::load("../../openforge.yaml").unwrap();
        config.state_db = dir.path().join("state.db").to_string_lossy().into();
        let engine = Engine::new(config).unwrap();
        assert!(!engine.has_provider_override());
        engine.set_provider_override(Some(provider));
        assert_eq!(engine.providers(), vec!["sevi"]);
        assert_eq!(engine.models()[0].model, "auto-select");
        let settings = GatewaySettings::new(dir.path().join("private/sevi.key"));
        assert!(!settings.status(&engine).to_string().contains("test-key"));
        let run_id = Uuid::new_v4();
        let run = serde_json::from_value(json!({
            "id":run_id,"project_id":Uuid::new_v4(),"objective":"test","base_sha":"abc",
            "status":"planning","autonomy":"suggest","budget":{"currency":"USD","hard_limit":10.0,"spent":0.0},
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"
        })).unwrap();
        engine.store.create_run(&run).unwrap();
        let text = engine
            .completion(openforge_core::CompletionInput {
                run_id,
                file_path: "test.rs".into(),
                language: "rust".into(),
                prefix: "fn".into(),
                suffix: String::new(),
                max_output_tokens: 32,
                max_cost_usd: 0.1,
            })
            .await
            .unwrap();
        assert!(text.contains("ok"));
        assert_eq!(engine.store.run_cost(run_id).unwrap(), 0.0123);
        let snapshot = engine.model_provider();
        engine.set_provider_override(None);
        assert_eq!(engine.providers(), vec!["local"]);
        assert_eq!(snapshot.catalog()[0].model, "auto-select");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn invalid_credentials_are_rejected_without_echoing_provider_body() {
        for key in ["", "••••••", "has spaces", "has\nnewline"] {
            assert!(verified_provider(key, BASE_URL).await.is_err());
        }
        let app = Router::new().route(
            "/cursor/chat/completions",
            post(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    "upstream diagnostic: secret-test-key",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/cursor", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = verified_provider("secret-test-key", &url)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("401"));
        assert!(!error.contains("secret-test-key"));
        server.abort();
    }
}

#[cfg(all(test, unix))]
mod persistence_tests {
    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn engine(directory: &std::path::Path) -> Engine {
        let mut config = openforge_core::OpenForgeConfig::load("../../openforge.yaml").unwrap();
        config.state_db = directory.join("state.db").to_string_lossy().into();
        Engine::new(config).unwrap()
    }

    #[tokio::test]
    async fn plain_text_connect_is_saved_restored_and_forgotten_without_retesting() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route("/cursor/chat/completions", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                assert!(body.get("response_format").is_none());
                assert!(body["max_tokens"].as_u64().unwrap() >= 256);
                if headers["authorization"] == "Bearer bad-key" {
                    return (StatusCode::UNAUTHORIZED, "do not echo bad-key").into_response();
                }
                Json(json!({"choices":[{"message":{"content":"OK! Connection successful."},"finish_reason":"stop"}]})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/cursor", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/sevi.key");
        let first = engine(dir.path());
        let settings = GatewaySettings::new(path.clone());
        let status = settings
            .connect_at(&first, "fake-secret-key", &url)
            .await
            .unwrap();
        assert_eq!(status["connected"], true);
        assert_eq!(status["credential_persisted"], true);
        assert!(!status.to_string().contains("fake-secret-key"));
        // A rejected replacement must preserve both current and saved credentials.
        assert!(settings.connect_at(&first, "bad-key", &url).await.is_err());
        assert_eq!(
            settings.credentials.read().unwrap().as_deref(),
            Some("fake-secret-key")
        );
        assert!(first.has_provider_override());
        let restored_engine = engine(dir.path());
        let restored = GatewaySettings::new(path.clone());
        assert!(restored.restore(&restored_engine).unwrap());
        assert_eq!(restored_engine.models()[0].model, "auto-select");
        assert_eq!(calls.load(Ordering::SeqCst), 2); // Restore makes no billable probe.
        let disconnected = restored.disconnect(&restored_engine).await.unwrap();
        assert_eq!(disconnected["credential_persisted"], false);
        assert!(!path.exists());
        let next = GatewaySettings::new(path);
        assert!(!next.restore(&engine(dir.path())).unwrap());
        server.abort();
    }

    #[tokio::test]
    async fn fenced_json_is_a_valid_connection_reply_but_empty_content_is_not() {
        for (content, success) in [("```json\n{\"ok\":true}\n```", true), ("", false)] {
            let app =
                Router::new().route(
                    "/cursor/chat/completions",
                    post(move || async move {
                        Json(json!({"choices":[{"message":{"content":content}}]}))
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/cursor", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            assert_eq!(verified_provider("fake-key", &url).await.is_ok(), success);
            server.abort();
        }
    }
}
