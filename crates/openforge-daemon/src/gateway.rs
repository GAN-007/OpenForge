//! Sevi setup is session-only: credentials never enter config, SQLite or responses.
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

pub(super) fn status(engine: &Engine) -> Value {
    json!({
        "connected": engine.has_provider_override(),
        "base_url": BASE_URL,
        "model": MODEL,
        "credential_storage": "daemon_memory",
        "pricing": "gateway_reported_or_unpriced"
    })
}

pub(super) async fn connect(engine: &Engine, params: &Value) -> Result<Value> {
    let key = params
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let provider = verified_provider(key, BASE_URL).await?;
    engine.set_provider_override(Some(provider));
    Ok(status(engine))
}

async fn verified_provider(key: &str, base_url: &str) -> Result<Arc<dyn ModelProvider>> {
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| (33..=126).contains(&byte)) {
        bail!("Paste the actual gateway API key, not the masked placeholder.");
    }
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
    // This user-triggered setup request contains no repository or conversation data.
    let request = ModelRequest {
        invocation_id: Uuid::new_v4(),
        run_id: Uuid::nil(),
        task_id: None,
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Connection test. Return only this JSON object: {\"ok\":true}".into(),
        }],
        requirements: ModelRequirements {
            task_class: "gateway_setup".into(),
            context_tokens: 128,
            requires_tools: false,
            requires_vision: false,
            requires_structured_output: true,
            max_cost_usd: f64::MAX,
            max_latency_ms: None,
            data_classification: DataClassification::Public,
            preferred_model_families: vec![],
            excluded_model_families: vec![],
        },
        temperature: 0.0,
        max_output_tokens: 32,
        response_schema: None,
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.invoke(&model, &request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Gateway connection timed out. Check access and try again."))??;
    if !serde_json::from_str::<Value>(&response.text).is_ok_and(|value| value.is_object()) {
        bail!(
            "Gateway responded but did not return the JSON output required for OpenForge planning."
        );
    }
    Ok(Arc::new(provider))
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
        assert!(!status(&engine).to_string().contains("test-key"));
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
