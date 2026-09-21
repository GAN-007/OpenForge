mod gateway;
mod http_api;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clap::Parser;
use openforge_artifacts::ArtifactStore;
use openforge_context::RepositoryIndex;
use openforge_core::{CompletionInput, Engine, OpenForgeConfig, RunnerBackend};
use openforge_cost::{BudgetGuard, BudgetLimits};
use openforge_events::verify_event_chain;
use openforge_mcp::{McpProcessConfig, McpStdioClient};
use openforge_plugins::PluginHost;
use openforge_policy::{AgentPolicy, CapabilityRequest};
use openforge_protocol::{
    AutonomyLevel, CapabilitySet, EventEnvelope, PROTOCOL_VERSION, RpcError, RpcRequest,
    RpcResponse,
};
use openforge_search::SearchIndex;
use openforge_secrets::{EnvironmentSecretBroker, SecretBroker};
use openforge_symbols::SymbolGraph;
use openforge_telemetry::TelemetryRegistry;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
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
    artifacts: ArtifactStore,
    telemetry: TelemetryRegistry,
    api_token: Option<Arc<str>>,
    started_at: Instant,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config = OpenForgeConfig::load(&args.config)?;
    let artifacts = ArtifactStore::open(&config.artifact_dir)?;
    let api_token = std::env::var("OPENFORGE_API_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Arc::<str>::from);

    let state = AppState {
        engine: Arc::new(Engine::new(config)?),
        artifacts,
        telemetry: TelemetryRegistry::default(),
        api_token,
        started_at: Instant::now(),
    };

    let origins = allowed_origins()?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::POST, Method::GET, Method::DELETE])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::HeaderName::from_static("last-event-id"),
        ]);

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/rpc", post(rpc))
        .merge(http_api::routes())
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(
        listen = %args.listen,
        authenticated = state.api_token.is_some(),
        "OpenForge daemon listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "authenticated": state.api_token.is_some()
    }))
}

async fn rpc(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RpcRequest>,
) -> impl IntoResponse {
    let id = request.id.clone();

    if let Err(error) = authorize(&state, &headers) {
        state.telemetry.increment("rpc.unauthorized", 1);
        return (
            StatusCode::UNAUTHORIZED,
            Json(RpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(RpcError {
                    code: -32001,
                    message: error.to_string(),
                    data: None,
                }),
            }),
        );
    }

    let method = request.method.clone();
    let started = Instant::now();
    state.telemetry.increment("rpc.calls", 1);

    let result = handle(&state, request).await;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    state.telemetry.observe("rpc.latency_ms", elapsed_ms);

    let mut attributes = BTreeMap::new();
    attributes.insert("method".into(), Value::String(method));
    attributes.insert("latency_ms".into(), json!(elapsed_ms));

    match result {
        Ok(value) => {
            attributes.insert("ok".into(), Value::Bool(true));
            state.telemetry.event("rpc.completed", attributes);
            (
                StatusCode::OK,
                Json(RpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: Some(value),
                    error: None,
                }),
            )
        }
        Err(error) => {
            state.telemetry.increment("rpc.errors", 1);
            attributes.insert("ok".into(), Value::Bool(false));
            state.telemetry.event("rpc.completed", attributes);
            (
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
            )
        }
    }
}

async fn handle(state: &AppState, request: RpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "initialize" => {
            let capabilities = [
                "runs",
                "tasks",
                "events",
                "event_integrity",
                "event_stream",
                "run_listing",
                "rest_api",
                "gateway_settings",
                "budgets",
                "memory",
                "policies",
                "mcp",
                "acp",
                "sandboxes",
                "model_routing",
                "completion",
                "repository_index",
                "repository_search",
                "symbols",
                "artifacts",
                "telemetry",
                "secrets",
                "plugins",
                "artifact_stream",
                "budget_reservations",
                "kubernetes_runner",
            ]
            .into_iter()
            .map(|name| (name.to_string(), true))
            .collect();

            Ok(serde_json::to_value(CapabilitySet {
                protocol_version: PROTOCOL_VERSION.into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                capabilities,
            })?)
        }
        "gateway/status" => Ok(gateway::status(&state.engine)),
        "gateway/connect" => gateway::connect(&state.engine, &request.params).await,
        "gateway/disconnect" => {
            state.engine.set_provider_override(None);
            Ok(gateway::status(&state.engine))
        }
        "run/create" => {
            let repo = required_string(&request.params, "repo")?;
            let objective = required_string(&request.params, "objective")?;
            let budget = request
                .params
                .get("budget_usd")
                .and_then(Value::as_f64)
                .unwrap_or(10.0);
            let autonomy = parse_autonomy(
                request
                    .params
                    .get("autonomy")
                    .and_then(Value::as_str)
                    .unwrap_or("suggest"),
            )?;
            let run = state
                .engine
                .create_run(PathBuf::from(repo).as_path(), objective, autonomy, budget)
                .await?;
            Ok(serde_json::to_value(run)?)
        }
        "run/plan" => {
            let repo = required_string(&request.params, "repo")?;
            let run_id = required_uuid(&request.params, "run_id")?;
            let run = state
                .engine
                .store
                .get_run(run_id)?
                .context("run not found")?;
            let tasks = state
                .engine
                .plan_run(PathBuf::from(repo).as_path(), &run)
                .await?;
            Ok(serde_json::to_value(tasks)?)
        }
        "run/execute" => {
            let repo = required_string(&request.params, "repo")?;
            let run_id = required_uuid(&request.params, "run_id")?;
            let policy_path = request
                .params
                .get("policy_path")
                .and_then(Value::as_str)
                .unwrap_or("config/policies/development.yaml");
            let docker = request
                .params
                .get("docker")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let runner_backend = request
                .params
                .get("runner_backend")
                .and_then(Value::as_str)
                .map(RunnerBackend::parse)
                .transpose()?
                .unwrap_or(if docker {
                    RunnerBackend::Docker
                } else {
                    RunnerBackend::Local
                });
            let policy = AgentPolicy::from_yaml(policy_path)?;
            let branch = state
                .engine
                .execute_run_with_backend(
                    PathBuf::from(repo).as_path(),
                    run_id,
                    policy,
                    runner_backend,
                )
                .await?;
            Ok(json!({"integration_branch": branch}))
        }
        "run/list" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(1000) as usize;
            let offset = request
                .params
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Ok(serde_json::to_value(
                state
                    .engine
                    .store
                    .list_runs(limit, usize::try_from(offset)?)?,
            )?)
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
            let after = request
                .params
                .get("after_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(500)
                .min(5000) as usize;
            Ok(serde_json::to_value(
                state.engine.store.list_events(id, after, limit)?,
            )?)
        }
        "event/verify" => {
            let events = load_global_events(state, 100_000)?;
            Ok(serde_json::to_value(verify_event_chain(&events)?)?)
        }
        "budget/get" => {
            let id = required_uuid(&request.params, "run_id")?;
            Ok(json!({
                "run_id": id,
                "spent_usd": state.engine.store.run_cost(id)?
            }))
        }
        "memory/put" => {
            let scope = required_string(&request.params, "scope")?;
            let key = required_string(&request.params, "key")?;
            let value = request
                .params
                .get("value")
                .cloned()
                .context("value is required")?;
            let project_id = request
                .params
                .get("project_id")
                .and_then(Value::as_str)
                .map(Uuid::parse_str)
                .transpose()?;
            let repository_id = request.params.get("repository_id").and_then(Value::as_str);
            state
                .engine
                .store
                .memory_put(&scope, project_id, repository_id, &key, &value)?;
            Ok(json!({"ok": true}))
        }
        "memory/search" => {
            let scope = request.params.get("scope").and_then(Value::as_str);
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(1000) as usize;
            Ok(serde_json::to_value(
                state.engine.store.memory_search(scope, query, limit)?,
            )?)
        }
        "memory/delete" => {
            let scope = request.params.get("scope").and_then(Value::as_str);
            let key = required_string(&request.params, "key")?;
            Ok(json!({
                "deleted": state.engine.store.memory_delete(scope, &key)?
            }))
        }
        "completion/request" => {
            let run_id = required_uuid(&request.params, "run_id")?;
            let file_path = required_string(&request.params, "file_path")?;
            let language = request
                .params
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("text");
            let prefix = request
                .params
                .get("prefix")
                .and_then(Value::as_str)
                .unwrap_or("");
            let suffix = request
                .params
                .get("suffix")
                .and_then(Value::as_str)
                .unwrap_or("");
            let max_output_tokens = request
                .params
                .get("max_output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(256)
                .min(2048) as u32;
            let max_cost_usd = request
                .params
                .get("max_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.05);
            let insertion = state
                .engine
                .completion(CompletionInput {
                    run_id,
                    file_path,
                    language: language.to_string(),
                    prefix: prefix.to_string(),
                    suffix: suffix.to_string(),
                    max_output_tokens,
                    max_cost_usd,
                })
                .await?;
            Ok(json!({"text": insertion}))
        }
        "repository/index" => {
            let repo = required_string(&request.params, "repo")?;
            Ok(serde_json::to_value(RepositoryIndex::build(repo)?)?)
        }
        "search/query" => {
            let repo = required_string(&request.params, "repo")?;
            let query = required_string(&request.params, "query")?;
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(25)
                .min(250) as usize;
            let index = SearchIndex::build(repo)?;
            Ok(json!({
                "stats": index.stats(),
                "hits": index.query(&query, limit)
            }))
        }
        "symbols/query" => {
            let repo = required_string(&request.params, "repo")?;
            let query = required_string(&request.params, "query")?;
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(25)
                .min(250) as usize;
            let graph = SymbolGraph::build(repo)?;
            Ok(json!({
                "stats": graph.stats(),
                "symbols": graph.search(&query, limit)
            }))
        }
        "symbols/graph" => {
            let repo = required_string(&request.params, "repo")?;
            Ok(serde_json::to_value(SymbolGraph::build(repo)?)?)
        }
        "artifact/put" => {
            let encoded = required_string(&request.params, "base64")?;
            let bytes = BASE64
                .decode(encoded.as_bytes())
                .context("artifact base64 is invalid")?;
            let media_type = request
                .params
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let source = request
                .params
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("rpc");
            let metadata: BTreeMap<String, Value> = serde_json::from_value(
                request
                    .params
                    .get("metadata")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .context("artifact metadata must be an object")?;
            Ok(serde_json::to_value(
                state
                    .artifacts
                    .put_bytes(&bytes, media_type, source, metadata)?,
            )?)
        }
        "artifact/get" => {
            let digest = required_string(&request.params, "sha256")?;
            let descriptor = state.artifacts.descriptor(&digest)?;
            let bytes = state.artifacts.get(&digest)?;
            Ok(json!({
                "descriptor": descriptor,
                "base64": BASE64.encode(bytes)
            }))
        }
        "artifact/descriptor" => {
            let digest = required_string(&request.params, "sha256")?;
            Ok(serde_json::to_value(state.artifacts.descriptor(&digest)?)?)
        }
        "artifact/delete" => {
            let digest = required_string(&request.params, "sha256")?;
            Ok(json!({"deleted": state.artifacts.delete(&digest)?}))
        }
        "policy/evaluate" => {
            let policy_path = required_string(&request.params, "policy_path")?;
            let capability = required_string(&request.params, "capability")?;
            let policy = AgentPolicy::from_yaml(policy_path)?;
            Ok(json!({
                "decision": evaluate_policy(&policy, &capability, &request.params)?
            }))
        }
        "telemetry/snapshot" => Ok(serde_json::to_value(state.telemetry.snapshot())?),
        "secret/lease" => {
            let secret_name = required_string(&request.params, "secret_name")?;
            let audience = required_string(&request.params, "audience")?;
            let ttl_seconds = request
                .params
                .get("ttl_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(60)
                .clamp(1, 300);
            let policy_path = request
                .params
                .get("policy_path")
                .and_then(Value::as_str)
                .unwrap_or("config/policies/development.yaml");
            let policy = AgentPolicy::from_yaml(policy_path)?;
            let capability = format!("secret://{secret_name}");
            match policy.evaluate(CapabilityRequest::Secret(&capability)) {
                openforge_policy::Decision::Allow => {}
                openforge_policy::Decision::Ask => {
                    anyhow::bail!(
                        "secret {secret_name} requires approval under policy {policy_path}"
                    );
                }
                openforge_policy::Decision::Deny => {
                    anyhow::bail!("secret {secret_name} is denied by policy {policy_path}");
                }
            }

            let mut allowed = policy.allowed_secrets();
            if !allowed.iter().any(|name| name == &secret_name) {
                allowed.push(secret_name.clone());
            }
            let broker = EnvironmentSecretBroker::with_max_ttl(allowed, 300);
            let lease = broker.lease(&secret_name, &audience, ttl_seconds).await?;
            let value = lease.expose()?.to_string();
            state.engine.store.record_secret_lease(&lease.descriptor)?;
            Ok(json!({
                "lease": lease.descriptor,
                "value": value
            }))
        }
        "secret/revoke" => {
            let lease_id = required_uuid(&request.params, "lease_id")?;
            Ok(json!({
                "revoked": state.engine.store.revoke_secret_lease(lease_id)?
            }))
        }
        "secret/list" => Ok(serde_json::to_value(
            state.engine.store.list_secret_leases()?,
        )?),
        "acp/spawn" => {
            let program = required_string(&request.params, "program")?;
            let args = optional_string_array(&request.params, "args")?;
            let cwd = request
                .params
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let environment: BTreeMap<String, String> = serde_json::from_value(
                request
                    .params
                    .get("environment")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .context("ACP environment must be an object of string values")?;
            let policy_path = request
                .params
                .get("policy_path")
                .and_then(Value::as_str)
                .unwrap_or("config/policies/development.yaml");
            let policy = AgentPolicy::from_yaml(policy_path)?;
            match policy.evaluate(CapabilityRequest::Acp(&program)) {
                openforge_policy::Decision::Allow => {}
                openforge_policy::Decision::Ask => {
                    anyhow::bail!("ACP spawn requires approval for program {program}");
                }
                openforge_policy::Decision::Deny => {
                    anyhow::bail!("ACP program {program} is denied by policy");
                }
            }

            let mut config = openforge_acp::AcpProcessConfig::new(&program, args);
            config.cwd = cwd;
            config.environment = environment;
            config.request_timeout = Duration::from_secs(
                request
                    .params
                    .get("timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(120)
                    .clamp(1, 600),
            );
            let mut client = openforge_acp::AcpAgentClient::spawn_with_config(config).await?;
            let process_id = match state.engine.store.register_acp_process(&program) {
                Ok(process_id) => process_id,
                Err(error) => {
                    let _ = client.terminate().await;
                    return Err(error);
                }
            };
            state
                .engine
                .acp_clients
                .lock()
                .await
                .insert(process_id, Arc::new(tokio::sync::Mutex::new(client)));
            Ok(json!({"process_id": process_id}))
        }
        "acp/request" => {
            let process_id = required_uuid(&request.params, "process_id")?;
            let method = required_string(&request.params, "method")?;
            let params = request.params.get("params").cloned().unwrap_or(Value::Null);
            let client = {
                let clients = state.engine.acp_clients.lock().await;
                clients
                    .get(&process_id)
                    .cloned()
                    .context("ACP process not found")?
            };
            let mut client = client.lock().await;
            let response = client.request(&method, params).await?;
            Ok(json!({"result": response}))
        }
        "acp/notify" => {
            let process_id = required_uuid(&request.params, "process_id")?;
            let method = required_string(&request.params, "method")?;
            let params = request.params.get("params").cloned().unwrap_or(Value::Null);
            let client = {
                let clients = state.engine.acp_clients.lock().await;
                clients
                    .get(&process_id)
                    .cloned()
                    .context("ACP process not found")?
            };
            client.lock().await.notify(&method, params).await?;
            Ok(json!({"ok": true}))
        }
        "acp/close" => {
            let process_id = required_uuid(&request.params, "process_id")?;
            let client = state.engine.acp_clients.lock().await.remove(&process_id);
            if let Some(client) = client {
                client.lock().await.terminate().await?;
                let persisted = state.engine.store.deregister_acp_process(process_id)?;
                Ok(json!({"closed": true, "persisted": persisted}))
            } else {
                Ok(json!({"closed": false, "reason": "process not found"}))
            }
        }
        "acp/list" => Ok(serde_json::to_value(
            state.engine.store.list_acp_processes()?,
        )?),
        "mcp/list_tools" => {
            let server_name = required_string(&request.params, "server_name")?;
            enforce_mcp_policy(
                &request.params,
                &format!("{server_name}/tools/list"),
                "MCP tool discovery",
            )?;
            let mut client = initialized_mcp_client(state, &server_name).await?;
            let result = client.list_tools().await;
            let cleanup = client.shutdown().await;
            let result = result?;
            cleanup?;
            Ok(serde_json::to_value(result)?)
        }
        "mcp/call_tool" => {
            let server_name = required_string(&request.params, "server_name")?;
            let tool_name = required_string(&request.params, "tool_name")?;
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            enforce_mcp_policy(
                &request.params,
                &tool_name,
                &format!("MCP tool {tool_name}"),
            )?;
            state
                .engine
                .tool_bus
                .mcp_call(&server_name, &tool_name, arguments)
                .await
        }
        "mcp/list_resources" => {
            let server_name = required_string(&request.params, "server_name")?;
            enforce_mcp_policy(
                &request.params,
                &format!("{server_name}/resources/list"),
                "MCP resource discovery",
            )?;
            let mut client = initialized_mcp_client(state, &server_name).await?;
            let result = client.list_resources().await;
            let cleanup = client.shutdown().await;
            let result = result?;
            cleanup?;
            Ok(result)
        }
        "mcp/read_resource" => {
            let server_name = required_string(&request.params, "server_name")?;
            let uri = required_string(&request.params, "uri")?;
            enforce_mcp_policy(
                &request.params,
                &format!("{server_name}/resources/read/{uri}"),
                &format!("MCP resource {uri}"),
            )?;
            let mut client = initialized_mcp_client(state, &server_name).await?;
            let result = client.read_resource(&uri).await;
            let cleanup = client.shutdown().await;
            let result = result?;
            cleanup?;
            Ok(result)
        }
        "mcp/list_prompts" => {
            let server_name = required_string(&request.params, "server_name")?;
            enforce_mcp_policy(
                &request.params,
                &format!("{server_name}/prompts/list"),
                "MCP prompt discovery",
            )?;
            let mut client = initialized_mcp_client(state, &server_name).await?;
            let result = client.list_prompts().await;
            let cleanup = client.shutdown().await;
            let result = result?;
            cleanup?;
            Ok(result)
        }
        "budget/reserve" => {
            let run_id = required_uuid(&request.params, "run_id")?;
            let estimated = request
                .params
                .get("estimated_usd")
                .and_then(Value::as_f64)
                .context("estimated_usd is required")?;
            let limits = budget_limits_for_run(state, run_id)?;
            let (_, settled) = state.engine.store.budget_usage(run_id)?;
            let (_, daily_settled) = state.engine.store.daily_budget_usage()?;
            let run_spent = state.engine.store.run_cost(run_id)? + settled;
            let daily_spent = state.engine.store.daily_cost()? + daily_settled;
            let guard = BudgetGuard::with_usage(limits.clone(), 0.0, run_spent, daily_spent)?;
            let _reservation = guard.reserve(estimated).await?;
            let reservation_id = state.engine.store.register_budget_reservation(
                run_id,
                estimated,
                limits.per_run,
                limits.daily,
            )?;
            Ok(json!({
                "reservation_id": reservation_id,
                "estimated_usd": estimated
            }))
        }
        "budget/settle" => {
            let reservation_id = required_uuid(&request.params, "reservation_id")?;
            let actual = request
                .params
                .get("actual_usd")
                .and_then(Value::as_f64)
                .context("actual_usd is required")?;
            let reservation = state
                .engine
                .store
                .budget_reservation(reservation_id)?
                .context("budget reservation not found")?;
            let limits = budget_limits_for_run(state, reservation.run_id)?;
            let (_, settled) = state.engine.store.budget_usage(reservation.run_id)?;
            let (_, daily_settled) = state.engine.store.daily_budget_usage()?;
            let run_spent = state.engine.store.run_cost(reservation.run_id)? + settled;
            let daily_spent = state.engine.store.daily_cost()? + daily_settled;
            let guard = BudgetGuard::with_usage(limits, 0.0, run_spent, daily_spent)?;
            guard.reserve(actual).await?.settle(actual).await?;
            let settled = state
                .engine
                .store
                .record_settled_cost(reservation_id, actual)?;
            Ok(serde_json::to_value(settled)?)
        }
        "budget/snapshot" => {
            let run_id = required_uuid(&request.params, "run_id")?;
            let limits = budget_limits_for_run(state, run_id)?;
            let (reserved, settled) = state.engine.store.budget_usage(run_id)?;
            let (daily_reserved, daily_settled) = state.engine.store.daily_budget_usage()?;
            let run_spent = state.engine.store.run_cost(run_id)? + settled;
            let daily_spent = state.engine.store.daily_cost()? + daily_settled;
            let guard = BudgetGuard::with_usage(limits.clone(), 0.0, run_spent, daily_spent)?;
            let (task, run, daily) = guard.snapshot().await;
            Ok(json!({
                "run_id": run_id,
                "task_spent_usd": task,
                "run_spent_usd": run,
                "daily_spent_usd": daily,
                "reserved_usd": reserved,
                "daily_reserved_usd": daily_reserved,
                "limits": limits
            }))
        }
        "artifact/stream/begin" => {
            let media_type = request
                .params
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let source = request
                .params
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("rpc-stream");
            let upload_id = state.artifacts.begin_stream(media_type, source)?;
            Ok(json!({"upload_id": upload_id}))
        }
        "artifact/stream/chunk" => {
            let upload_id = required_string(&request.params, "upload_id")?;
            let encoded = required_string(&request.params, "base64")?;
            let bytes = BASE64
                .decode(encoded.as_bytes())
                .context("artifact stream chunk base64 is invalid")?;
            state.artifacts.write_stream_chunk(&upload_id, &bytes)?;
            Ok(json!({
                "upload_id": upload_id,
                "bytes_written": bytes.len()
            }))
        }
        "artifact/stream/commit" => {
            let upload_id = required_string(&request.params, "upload_id")?;
            let metadata: BTreeMap<String, Value> = serde_json::from_value(
                request
                    .params
                    .get("metadata")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .context("artifact metadata must be an object")?;
            Ok(serde_json::to_value(
                state.artifacts.commit_stream(&upload_id, metadata)?,
            )?)
        }
        "artifact/stream/abort" => {
            let upload_id = required_string(&request.params, "upload_id")?;
            Ok(json!({
                "aborted": state.artifacts.abort_stream(&upload_id)?
            }))
        }
        "plugins/list" => {
            let host = PluginHost::open(&state.engine.config.plugin_dir)?;
            Ok(serde_json::to_value(host.list())?)
        }
        "plugins/capability/validate" => {
            let plugin_id = required_string(&request.params, "plugin_id")?;
            let capability = required_string(&request.params, "capability")?;
            let host = PluginHost::open(&state.engine.config.plugin_dir)?;
            Ok(serde_json::to_value(
                host.validate_capability(&plugin_id, &capability)?,
            )?)
        }
        "model/list" => Ok(serde_json::to_value(state.engine.models())?),
        "model/providers" => Ok(json!({"providers": state.engine.providers()})),
        _ => anyhow::bail!("unknown RPC method {}", request.method),
    }
}

fn enforce_mcp_policy(params: &Value, subject: &str, operation: &str) -> Result<()> {
    let policy_path = params
        .get("policy_path")
        .and_then(Value::as_str)
        .unwrap_or("config/policies/development.yaml");
    let policy = AgentPolicy::from_yaml(policy_path)?;
    match policy.evaluate(CapabilityRequest::Mcp(subject)) {
        openforge_policy::Decision::Allow => Ok(()),
        openforge_policy::Decision::Ask => {
            anyhow::bail!("{operation} requires approval under policy {policy_path}")
        }
        openforge_policy::Decision::Deny => {
            anyhow::bail!("{operation} is denied by policy {policy_path}")
        }
    }
}

async fn initialized_mcp_client(state: &AppState, server_name: &str) -> Result<McpStdioClient> {
    let server = state
        .engine
        .config
        .mcp_servers
        .iter()
        .find(|candidate| candidate.name == server_name)
        .with_context(|| format!("unknown MCP server {server_name}"))?;
    let mut config = McpProcessConfig::new(server.program.clone(), server.args.clone());
    config.cwd = server.cwd.as_ref().map(PathBuf::from);
    config.environment = server.environment.clone();
    config.request_timeout = Duration::from_secs(server.timeout_seconds.max(1));
    config.max_response_bytes = server.max_response_bytes.max(1024);

    let mut client = McpStdioClient::spawn_with_config(config).await?;
    client
        .initialize("openforge", env!("CARGO_PKG_VERSION"))
        .await?;
    Ok(client)
}

fn budget_limits_for_run(state: &AppState, run_id: Uuid) -> Result<BudgetLimits> {
    if let Some(limits) = state.engine.store.budget_limits(run_id)? {
        return Ok(BudgetLimits {
            per_call: limits.per_call,
            per_task: limits.per_task,
            per_run: limits.per_run,
            daily: limits.daily,
        });
    }

    let run = state
        .engine
        .store
        .get_run(run_id)?
        .context("run not found")?;
    let limits = BudgetLimits {
        per_call: run.budget.hard_limit.min(1.0),
        per_task: run.budget.hard_limit.min(5.0),
        per_run: run.budget.hard_limit,
        daily: run.budget.hard_limit.max(100.0),
    };
    state.engine.store.set_budget_limits(
        run_id,
        &openforge_store::BudgetLimitsRecord {
            per_call: limits.per_call,
            per_task: limits.per_task,
            per_run: limits.per_run,
            daily: limits.daily,
        },
    )?;
    Ok(limits)
}

fn load_global_events(state: &AppState, maximum: usize) -> Result<Vec<EventEnvelope>> {
    let mut events = Vec::new();
    let mut after = 0i64;

    while events.len() < maximum {
        let remaining = maximum - events.len();
        let page = state
            .engine
            .store
            .list_all_events(after, remaining.min(5000))?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|event| event.sequence).unwrap_or(after);
        let page_len = page.len();
        events.extend(page);
        if page_len < 5000 {
            break;
        }
    }

    if events.len() == maximum && !state.engine.store.list_all_events(after, 1)?.is_empty() {
        anyhow::bail!(
            "audit ledger exceeds verification limit; refusing to report a partial chain as valid"
        );
    }
    Ok(events)
}

fn evaluate_policy(policy: &AgentPolicy, capability: &str, params: &Value) -> Result<String> {
    let subject = params.get("subject").and_then(Value::as_str).unwrap_or("");
    let decision = match capability {
        "read_path" => policy.evaluate(CapabilityRequest::ReadPath(subject)),
        "write_path" => policy.evaluate(CapabilityRequest::WritePath(subject)),
        "network" => policy.evaluate(CapabilityRequest::Network(subject)),
        "mcp" => policy.evaluate(CapabilityRequest::Mcp(subject)),
        "acp" => policy.evaluate(CapabilityRequest::Acp(subject)),
        "database_read" => policy.evaluate(CapabilityRequest::DatabaseRead(subject)),
        "database_write" => policy.evaluate(CapabilityRequest::DatabaseWrite(subject)),
        "secret" => policy.evaluate(CapabilityRequest::Secret(subject)),
        "cloud_read" => policy.evaluate(CapabilityRequest::CloudRead(subject)),
        "cloud_write" => policy.evaluate(CapabilityRequest::CloudWrite(subject)),
        "deployment" => policy.evaluate(CapabilityRequest::Deployment(subject)),
        "browser" => policy.evaluate(CapabilityRequest::Browser(subject)),
        "delegate" => policy.evaluate(CapabilityRequest::Delegate(subject)),
        "process" => {
            let argv = string_array(params, "argv")?;
            policy.evaluate(CapabilityRequest::Process(&argv))
        }
        "git" => {
            let argv = string_array(params, "argv")?;
            policy.evaluate(CapabilityRequest::Git(&argv))
        }
        other => anyhow::bail!("unknown capability {other}"),
    };
    Ok(match decision {
        openforge_policy::Decision::Allow => "allow",
        openforge_policy::Decision::Ask => "ask",
        openforge_policy::Decision::Deny => "deny",
    }
    .into())
}

fn optional_string_array(params: &Value, field: &str) -> Result<Vec<String>> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(_) => string_array(params, field),
    }
}

fn string_array(params: &Value, field: &str) -> Result<Vec<String>> {
    params
        .get(field)
        .and_then(Value::as_array)
        .with_context(|| format!("{field} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("{field} must contain only strings"))
        })
        .collect()
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<()> {
    let Some(expected) = state.api_token.as_deref() else {
        return Ok(());
    };

    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .context("missing Authorization header")?;
    let supplied = authorization
        .strip_prefix("Bearer ")
        .context("Authorization must use Bearer scheme")?;

    if !constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        anyhow::bail!("invalid API token");
    }
    Ok(())
}

fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    let maximum = expected.len().max(supplied.len());
    for index in 0..maximum {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = supplied.get(index).copied().unwrap_or(0);
        difference |= (left ^ right) as usize;
    }
    difference == 0
}

fn allowed_origins() -> Result<Vec<HeaderValue>> {
    let raw = std::env::var("OPENFORGE_ALLOWED_ORIGINS").unwrap_or_else(|_| {
        [
            "http://127.0.0.1:5173",
            "http://localhost:5173",
            "http://127.0.0.1:1420",
            "http://localhost:1420",
            "tauri://localhost",
            "http://tauri.localhost",
        ]
        .join(",")
    });

    raw.split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| HeaderValue::from_str(origin).map_err(Into::into))
        .collect()
}

fn required_string(params: &Value, field: &str) -> Result<String> {
    let value = params
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("{field} is required"))?
        .trim();
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
