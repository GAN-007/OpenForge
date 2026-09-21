//! Sevi credentials are persisted in the user's private config directory and never enter project config, SQLite, logs, or API responses.
use anyhow::{Result, bail};
use openforge_core::Engine;
use openforge_models::{ModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use openforge_protocol::{
    ChatMessage, DataClassification, ModelRequest, ModelRequirements, ModelSpec,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const BASE_URL: &str = "https://model.sevi.io/cursor";
const MODEL: &str = "auto-select";

pub(super) fn status(engine: &Engine) -> Value {
    let credential_path = credential_path();
    json!({
        "connected": engine.has_provider_override(),
        "base_url": BASE_URL,
        "model": MODEL,
        "credential_storage": "user_config_file",
        "credential_persisted": credential_path.exists(),
        "pricing": "gateway_reported_or_unpriced"
    })
}

pub(super) fn restore(engine: &Engine) -> Result<bool> {
    let path = credential_path();
    let Some(key) = read_persisted_key(&path)? else {
        return Ok(false);
    };
    engine.set_provider_override(Some(provider_for_key(&key, BASE_URL)?));
    Ok(true)
}

pub(super) async fn connect(engine: &Engine, params: &Value) -> Result<Value> {
    let key = params
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let provider = verified_provider(key, BASE_URL).await?;
    persist_key(&credential_path(), key)?;
    engine.set_provider_override(Some(provider));
    Ok(status(engine))
}

pub(super) fn disconnect(engine: &Engine) -> Result<Value> {
    remove_persisted_key(&credential_path())?;
    engine.set_provider_override(None);
    Ok(status(engine))
}

async fn verified_provider(key: &str, base_url: &str) -> Result<Arc<dyn ModelProvider>> {
    let provider = provider_for_key(key, base_url)?;
    let model = provider
        .catalog()
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Sevi provider has no configured model"))?;

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
    Ok(provider)
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
    Ok(Arc::new(OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig {
            provider_name: "sevi".into(),
            base_url: base_url.into(),
            api_key: Some(key.into()),
            extra_headers: vec![],
            models: vec![model],
        },
    )?))
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 4096
        || !key.bytes().all(|byte| (33..=126).contains(&byte))
    {
        bail!("Paste the actual gateway API key, not the masked placeholder.");
    }
    Ok(())
}

fn credential_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OPENFORGE_GATEWAY_CREDENTIAL_PATH") {
        return PathBuf::from(path);
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(root)
            .join("openforge")
            .join("sevi-gateway.key");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("openforge")
            .join("sevi-gateway.key");
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("OpenForge")
            .join("sevi-gateway.key");
    }
    PathBuf::from(".openforge/sevi-gateway.key")
}

fn read_persisted_key(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(key) => {
            validate_key(&key)?;
            Ok(Some(key))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn persist_key(path: &Path, key: &str) -> Result<()> {
    validate_key(key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }

    let temporary = path.with_extension(format!("{}.tmp", Uuid::now_v7().simple()));
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?
    };
    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;

    file.write_all(key.as_bytes())?;
    file.sync_all()?;

    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }

    fs::rename(&temporary, path)?;
    Ok(())
}

fn remove_persisted_key(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
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

    #[test]
    fn persisted_gateway_key_round_trips_without_being_embedded_in_status() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sevi.key");
        persist_key(&path, "test-persisted-key").unwrap();
        assert_eq!(
            read_persisted_key(&path).unwrap().as_deref(),
            Some("test-persisted-key")
        );
        remove_persisted_key(&path).unwrap();
        assert!(read_persisted_key(&path).unwrap().is_none());
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
