use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use openforge_core::{Engine, OpenForgeConfig};
use openforge_policy::AgentPolicy;
use openforge_protocol::{
    AutonomyLevel, CapabilitySet, RpcError, RpcRequest, RpcResponse,
    PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "openforge.yaml")]
    config: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: String,
}

#[derive(Clone)]
struct AppState {
    engine: Arc<Engine>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config = OpenForgeConfig::load(&args.config).unwrap_or_default();
    let state = AppState {
        engine: Arc::new(Engine::new(config)?),
    };

    let origins = [
        "http://127.0.0.1:5173",
        "http://localhost:5173",
        "http://127.0.0.1:1420",
        "http://localhost:1420",
        "tauri://localhost",
        "http://tauri.localhost",
    ]
    .into_iter()
    .map(HeaderValue::from_static)
    .collect::<Vec<_>>();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::POST, Method::GET])
        .allow_headers([header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/rpc", post(rpc))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, "OpenForge daemon listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "protocol": PROTOCOL_VERSION}))
}

async fn rpc(
    State(state): State<AppState>,
    Json(request): Json<RpcRequest>,
) -> impl IntoResponse {
    let id = request.id.clone();
    match handle(&state, request).await {
        Ok(value) => (
            StatusCode::OK,
            Json(RpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(value),
                error: None,
            }),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(RpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(RpcError {
                    code: -32000,
                    message: error.to_string(),
                    data: None,
                }),
            }),
        ),
    }
}

async fn handle(state: &AppState, request: RpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "initialize" => {
            let mut capabilities = BTreeMap::new();
            for key in [
                "runs",
                "tasks",
                "events",
                "budgets",
                "memory",
                "policies",
                "mcp",
                "acp",
                "sandboxes",
                "model_routing",
                "completion",
            ] {
                capabilities.insert(key.into(), true);
            }
            Ok(serde_json::to_value(CapabilitySet {
                protocol_version: PROTOCOL_VERSION.into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                capabilities,
            })?)
        }
        "run/create" => {
            let repo = required_string(&request.params, "repo")?;
            let objective = required_string(&request.params, "objective")?;
            let budget = request.params.get("budget_usd").and_then(Value::as_f64).unwrap_or(10.0);
            let autonomy = parse_autonomy(
                request.params.get("autonomy").and_then(Value::as_str).unwrap_or("suggest"),
            )?;
            let run = state.engine.create_run(
                PathBuf::from(repo).as_path(),
                objective,
                autonomy,
                budget,
            ).await?;
            Ok(serde_json::to_value(run)?)
        }
        "run/plan" => {
            let repo = required_string(&request.params, "repo")?;
            let run_id = required_uuid(&request.params, "run_id")?;
            let run = state.engine.store.get_run(run_id)?.context("run not found")?;
            let tasks = state.engine.plan_run(PathBuf::from(repo).as_path(), &run).await?;
            Ok(serde_json::to_value(tasks)?)
        }
        "run/execute" => {
            let repo = required_string(&request.params, "repo")?;
            let run_id = required_uuid(&request.params, "run_id")?;
            let policy_path = request.params.get("policy_path").and_then(Value::as_str)
                .unwrap_or("config/policies/development.yaml");
            let docker = request.params.get("docker").and_then(Value::as_bool).unwrap_or(false);
            let policy = AgentPolicy::from_yaml(policy_path)?;
            let branch = state.engine.execute_run(
                PathBuf::from(repo).as_path(),
                run_id,
                policy,
                docker,
            ).await?;
            Ok(json!({"integration_branch": branch}))
        }
        "run/get" => {
            let id = required_uuid(&request.params, "run_id")?;
            Ok(serde_json::to_value(state.engine.store.get_run(id)?)?)
        }
        "task/list" => {
            let id = required_uuid(&request.params, "run_id")?;
            Ok(serde_json::to_value(state.engine.store.list_tasks(id)?)?)
        }
        "event/list" => {
            let id = required_uuid(&request.params, "run_id")?;
            let after = request.params.get("after_sequence").and_then(Value::as_i64).unwrap_or(0);
            let limit = request.params.get("limit").and_then(Value::as_u64).unwrap_or(500).min(5000) as usize;
            Ok(serde_json::to_value(state.engine.store.list_events(id, after, limit)?)?)
        }
        "budget/get" => {
            let id = required_uuid(&request.params, "run_id")?;
            Ok(json!({"run_id": id, "spent_usd": state.engine.store.run_cost(id)?}))
        }
        "memory/put" => {
            let scope = required_string(&request.params, "scope")?;
            let key = required_string(&request.params, "key")?;
            let value = request.params.get("value").cloned().context("value is required")?;
            let project_id = request.params.get("project_id").and_then(Value::as_str)
                .map(Uuid::parse_str).transpose()?;
            let repository_id = request.params.get("repository_id").and_then(Value::as_str);
            state.engine.store.memory_put(&scope, project_id, repository_id, &key, &value)?;
            Ok(json!({"ok": true}))
        }
        "memory/search" => {
            let scope = request.params.get("scope").and_then(Value::as_str);
            let query = request.params.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = request.params.get("limit").and_then(Value::as_u64).unwrap_or(100).min(1000) as usize;
            Ok(serde_json::to_value(state.engine.store.memory_search(scope, query, limit)?)?)
        }
        "memory/delete" => {
            let scope = request.params.get("scope").and_then(Value::as_str);
            let key = required_string(&request.params, "key")?;
            Ok(json!({"deleted": state.engine.store.memory_delete(scope, &key)?}))
        }
        "completion/request" => {
            let run_id = required_uuid(&request.params, "run_id")?;
            let file_path = required_string(&request.params, "file_path")?;
            let language = request.params.get("language").and_then(Value::as_str).unwrap_or("text");
            let prefix = request.params.get("prefix").and_then(Value::as_str).unwrap_or("");
            let suffix = request.params.get("suffix").and_then(Value::as_str).unwrap_or("");
            let max_output_tokens = request.params.get("max_output_tokens").and_then(Value::as_u64).unwrap_or(256).min(2048) as u32;
            let max_cost_usd = request.params.get("max_cost_usd").and_then(Value::as_f64).unwrap_or(0.05);
            let insertion = state.engine.completion(
                run_id, &file_path, language, prefix, suffix, max_output_tokens, max_cost_usd,
            ).await?;
            Ok(json!({"text": insertion}))
        }
        "model/providers" => Ok(json!({"providers": state.engine.providers()})),
        _ => anyhow::bail!("unknown RPC method {}", request.method),
    }
}

fn required_string(params: &Value, field: &str) -> Result<String> {
    let value = params.get(field).and_then(Value::as_str)
        .with_context(|| format!("{field} is required"))?.trim();
    if value.is_empty() {
        anyhow::bail!("{field} cannot be empty");
    }
    Ok(value.to_owned())
}

fn required_uuid(params: &Value, field: &str) -> Result<Uuid> {
    Ok(Uuid::parse_str(&required_string(params, field)?)?)
}

fn parse_autonomy(value: &str) -> Result<AutonomyLevel> {
    match value {
        "observe" => Ok(AutonomyLevel::Observe),
        "suggest" => Ok(AutonomyLevel::Suggest),
        "edit" => Ok(AutonomyLevel::Edit),
        "execute" => Ok(AutonomyLevel::Execute),
        "autonomous" => Ok(AutonomyLevel::Autonomous),
        other => anyhow::bail!("unknown autonomy level {other}"),
    }
}
