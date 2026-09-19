mod extended;
mod services;

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use clap::Parser;
use openforge_artifacts::ArtifactStore;
use openforge_context::RepositoryIndex;
use openforge_core::{CompletionInput, Engine, OpenForgeConfig};
use openforge_events::verify_event_chain;
use openforge_policy::{AgentPolicy, CapabilityRequest};
use openforge_protocol::{
    AutonomyLevel, CapabilitySet, EventEnvelope, RpcError, RpcRequest, RpcResponse,
    PROTOCOL_VERSION,
};
use openforge_search::SearchIndex;
use openforge_symbols::SymbolGraph;
use openforge_team::{Permission, Principal};
use openforge_telemetry::TelemetryRegistry;
use services::ServiceHub;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::Instant,
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
pub(crate) struct AppState {
    pub(crate) engine: Arc<Engine>,
    pub(crate) artifacts: ArtifactStore,
    pub(crate) telemetry: TelemetryRegistry,
    pub(crate) api_token: Option<Arc<str>>,
    pub(crate) services: Arc<ServiceHub>,
    pub(crate) started_at: Instant,
}

#[derive(Clone)]
pub(crate) struct AuthContext {
    pub(crate) subject: String,
    pub(crate) principal: Option<Principal>,
    pub(crate) local_owner: bool,
}

impl AuthContext {
    pub(crate) fn require(&self, permission: Permission) -> Result<()> {
        if self.local_owner {
            return Ok(());
        }
        self.principal
            .as_ref()
            .context("authenticated principal unavailable")?
            .require(permission)
    }

    pub(crate) fn workspace_id(&self) -> Option<Uuid> {
        self.principal.as_ref().map(|principal| principal.workspace_id)
    }
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
    let artifacts = ArtifactStore::open(&config.artifact_dir)?;
    let services = Arc::new(ServiceHub::new(&config)?);
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
        services,
        started_at: Instant::now(),
    };

    let origins = allowed_origins()?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::POST, Method::GET])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/rpc", post(rpc))
        .route("/v1/terminal/{id}/ws", get(terminal_ws))
        .route("/v1/runs/{run_id}/events/ws", get(run_events_ws))
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

    let auth = match authorize(&state, &headers) {
        Ok(auth) => auth,
        Err(error) => {
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
    };
    if let Err(error) = authorize_method(&auth, &request.method) {
        state.telemetry.increment("rpc.forbidden", 1);
        return (
            StatusCode::FORBIDDEN,
            Json(RpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(RpcError {
                    code: -32003,
                    message: error.to_string(),
                    data: None,
                }),
            }),
        );
    }

    let method = request.method.clone();
    let started = Instant::now();
    state.telemetry.increment("rpc.calls", 1);

    let result = handle(&state, &auth, request).await;
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

async fn handle(state: &AppState, auth: &AuthContext, request: RpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "initialize" => {
            let capabilities = [
                "runs",
                "tasks",
                "events",
                "event_integrity",
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
        "run/create" => {
            let repo = required_string(&request.params, "repo")?;
            let repo = ensure_repo_access(state, auth, &repo)?;
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
                .create_run(&repo, objective, autonomy, budget)
                .await?;
            Ok(serde_json::to_value(run)?)
        }
        "run/plan" => {
            let repo = required_string(&request.params, "repo")?;
            let repo = ensure_repo_access(state, auth, &repo)?;
            let run_id = required_uuid(&request.params, "run_id")?;
            let run = state
                .engine
                .store
                .get_run(run_id)?
                .context("run not found")?;
            let tasks = state
                .engine
                .plan_run(&repo, &run)
                .await?;
            Ok(serde_json::to_value(tasks)?)
        }
        "run/execute" => {
            let repo = required_string(&request.params, "repo")?;
            let repo = ensure_repo_access(state, auth, &repo)?;
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
            let policy = AgentPolicy::from_yaml(policy_path)?;
            let branch = state
                .engine
                .execute_run(&repo, run_id, policy, docker)
                .await?;
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
            let repository_id = request
                .params
                .get("repository_id")
                .and_then(Value::as_str);
            state.engine.store.memory_put(
                &scope,
                project_id,
                repository_id,
                &key,
                &value,
            )?;
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
            let repo = ensure_repo_access(state, auth, &repo)?;
            Ok(serde_json::to_value(RepositoryIndex::build(repo)?)?)
        }
        "search/query" => {
            let repo = required_string(&request.params, "repo")?;
            let repo = ensure_repo_access(state, auth, &repo)?;
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
            let repo = ensure_repo_access(state, auth, &repo)?;
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
            let repo = ensure_repo_access(state, auth, &repo)?;
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
            Ok(serde_json::to_value(state.artifacts.put_bytes(
                &bytes,
                media_type,
                source,
                metadata,
            )?)?)
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
        "model/providers" => Ok(json!({"providers": state.engine.providers()})),
        _ => {
            if let Some(value) =
                extended::handle_extended(state, auth, &request.method, &request.params).await?
            {
                Ok(value)
            } else {
                anyhow::bail!("unknown RPC method {}", request.method)
            }
        }
    }
}

async fn terminal_ws(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let auth = match authorize_websocket(&state, &headers) {
        Ok(auth) => auth,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                error.to_string(),
            )
                .into_response();
        }
    };
    if let Err(error) = auth.require(Permission::Execute) {
        return (StatusCode::FORBIDDEN, error.to_string()).into_response();
    }
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid terminal id: {error}"),
            )
                .into_response();
        }
    };

    ws.protocols(["openforge-v1"])
        .on_upgrade(move |socket| handle_terminal_socket(state, id, socket))
        .into_response()
}

async fn handle_terminal_socket(state: AppState, id: Uuid, socket: WebSocket) {
    let mut output = match state.services.terminals.subscribe(id).await {
        Ok(output) => output,
        Err(error) => {
            tracing::warn!(terminal_id=%id, error=%error, "terminal websocket subscription failed");
            return;
        }
    };
    let (mut sender, mut receiver) = socket.split();

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(error) = state.services.terminals.write(id, text.as_bytes()).await {
                            let _ = sender
                                .send(Message::Text(
                                    json!({"type":"error","message":error.to_string()})
                                        .to_string()
                                        .into(),
                                ))
                                .await;
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Err(error) = state.services.terminals.write(id, &bytes).await {
                            let _ = sender
                                .send(Message::Text(
                                    json!({"type":"error","message":error.to_string()})
                                        .to_string()
                                        .into(),
                                ))
                                .await;
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(bytes))) => {
                        if sender.send(Message::Pong(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Err(error) => {
                        tracing::debug!(terminal_id=%id,error=%error,"terminal websocket receive error");
                        break;
                    }
                }
            }
            event = output.recv() => {
                match event {
                    Ok(event) => {
                        let payload = match serde_json::to_string(&event) {
                            Ok(payload) => payload,
                            Err(error) => {
                                tracing::warn!(terminal_id=%id,error=%error,"serialize terminal event failed");
                                break;
                            }
                        };
                        if sender.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let payload = json!({
                            "type": "lagged",
                            "skipped": skipped
                        }).to_string();
                        if sender.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn run_events_ws(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let auth = match authorize_websocket(&state, &headers) {
        Ok(auth) => auth,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                error.to_string(),
            )
                .into_response();
        }
    };
    if let Err(error) = auth.require(Permission::Read) {
        return (StatusCode::FORBIDDEN, error.to_string()).into_response();
    }
    let run_id = match Uuid::parse_str(&run_id) {
        Ok(id) => id,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid run id: {error}"),
            )
                .into_response();
        }
    };

    ws.protocols(["openforge-v1"])
        .on_upgrade(move |socket| handle_run_events_socket(state, run_id, socket))
        .into_response()
}

async fn handle_run_events_socket(state: AppState, run_id: Uuid, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut after_sequence = 0i64;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(350));

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(bytes))) => {
                        if sender.send(Message::Pong(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = interval.tick() => {
                match state.engine.store.list_events(run_id, after_sequence, 500) {
                    Ok(events) => {
                        for event in events {
                            after_sequence = event.sequence;
                            match serde_json::to_string(&event) {
                                Ok(payload) => {
                                    if sender.send(Message::Text(payload.into())).await.is_err() {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(run_id=%run_id,error=%error,"serialize run event failed");
                                    return;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let payload = json!({"type":"error","message":error.to_string()}).to_string();
                        let _ = sender.send(Message::Text(payload.into())).await;
                        return;
                    }
                }
            }
        }
    }
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

    Ok(events)
}

fn evaluate_policy(
    policy: &AgentPolicy,
    capability: &str,
    params: &Value,
) -> Result<String> {
    let subject = params
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or("");
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

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<AuthContext> {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty());

    authorize_token(state, supplied)
}

fn authorize_websocket(state: &AppState, headers: &HeaderMap) -> Result<AuthContext> {
    if let Ok(auth) = authorize(state, headers) {
        return Ok(auth);
    }
    let supplied = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find_map(|protocol| protocol.strip_prefix("openforge-token."))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());

    authorize_token(state, supplied)
}

fn authorize_token(state: &AppState, supplied: Option<&str>) -> Result<AuthContext> {
    if let Some(expected) = state.api_token.as_deref() {
        if let Some(supplied) = supplied {
            if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
                return Ok(AuthContext {
                    subject: "local-owner".into(),
                    principal: None,
                    local_owner: true,
                });
            }
            if let Some(principal) = state.services.team.authenticate(supplied)? {
                return Ok(AuthContext {
                    subject: principal.identity.subject.clone(),
                    principal: Some(principal),
                    local_owner: false,
                });
            }
        }
        anyhow::bail!("valid bearer authorization is required");
    }

    if let Some(supplied) = supplied {
        if let Some(principal) = state.services.team.authenticate(supplied)? {
            return Ok(AuthContext {
                subject: principal.identity.subject.clone(),
                principal: Some(principal),
                local_owner: false,
            });
        }
        anyhow::bail!("invalid bearer token");
    }

    Ok(AuthContext {
        subject: "local-owner".into(),
        principal: None,
        local_owner: true,
    })
}

fn authorize_method(auth: &AuthContext, method: &str) -> Result<()> {
    if auth.local_owner {
        return Ok(());
    }
    let permission = if method.starts_with("team/") {
        Permission::ManageMembers
    } else if method.starts_with("plugin/") || method.starts_with("worker/register") {
        Permission::ManageWorkspace
    } else if method.starts_with("policy/") && method != "policy/evaluate" {
        Permission::ManagePolicy
    } else if method.starts_with("run/execute")
        || method.starts_with("run/create")
        || method.starts_with("run/plan")
        || method.starts_with("terminal/")
        || method.starts_with("worker/")
        || method.starts_with("thread/")
        || method.starts_with("approval/")
        || method.starts_with("debug/")
        || method.starts_with("database/")
        || method.starts_with("devops/")
        || method.starts_with("edit/")
    {
        Permission::Execute
    } else {
        Permission::Read
    };
    auth.require(permission)
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

pub(crate) fn ensure_repo_access(
    state: &AppState,
    auth: &AuthContext,
    path: &str,
) -> Result<PathBuf> {
    let canonical = PathBuf::from(path)
        .canonicalize()
        .with_context(|| format!("repository path {path} unavailable"))?;
    if auth.local_owner {
        return Ok(canonical);
    }
    let workspace_id = auth
        .workspace_id()
        .context("team principal has no workspace")?;
    state
        .services
        .team
        .require_repository_access(workspace_id, &canonical)
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
