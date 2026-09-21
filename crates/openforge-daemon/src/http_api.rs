//! REST and resumable SSE adapters over the canonical control plane.
use super::*;
use axum::{
    extract::{Path, Query},
    response::{
        Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::stream;
use serde::Deserialize;
use std::{collections::VecDeque, convert::Infallible};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/runs", get(list_runs).post(create_run))
        .route("/v1/runs/{id}", get(get_run))
        .route("/v1/runs/{id}/tasks", get(list_tasks))
        .route("/v1/runs/{id}/events", get(list_events))
        .route("/v1/runs/{id}/events/stream", get(stream_events))
        .route("/v1/runs/{id}/budget", get(get_budget))
        .route("/v1/models", get(models))
        .route("/v1/memory", post(put_memory).delete(delete_memory))
        .route("/v1/memory/search", get(search_memory))
        .route("/v1/policy/evaluate", post(policy))
}

fn failure(status: StatusCode, message: impl ToString) -> Response {
    (
        status,
        Json(json!({"error": {"message": message.to_string()}})),
    )
        .into_response()
}

fn check_access(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    authorize(state, headers).map_err(|error| (StatusCode::UNAUTHORIZED, error.to_string()))
}

fn check_run(state: &AppState, id: Uuid) -> Result<(), (StatusCode, String)> {
    match state.engine.store.get_run(id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err((StatusCode::NOT_FOUND, "run not found".into())),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error.to_string())),
    }
}

async fn call(state: AppState, headers: HeaderMap, method: &str, params: Value) -> Response {
    if let Err(response) = check_access(&state, &headers) {
        return failure(response.0, response.1);
    }
    match handle(
        &state,
        RpcRequest {
            jsonrpc: "2.0".into(),
            id: json!(null),
            method: method.into(),
            params,
        },
    )
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Default, Deserialize)]
struct Page {
    limit: Option<usize>,
    offset: Option<usize>,
}
async fn list_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> Response {
    call(
        state,
        headers,
        "run/list",
        json!({"limit": page.limit.unwrap_or(100), "offset": page.offset.unwrap_or(0)}),
    )
    .await
}
async fn create_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<Value>,
) -> Response {
    call(state, headers, "run/create", params).await
}
async fn run_call(
    state: AppState,
    headers: HeaderMap,
    id: Uuid,
    method: &str,
    params: Value,
) -> Response {
    if let Err(response) = check_access(&state, &headers).and_then(|()| check_run(&state, id)) {
        return failure(response.0, response.1);
    }
    call(state, headers, method, params).await
}
async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    run_call(state, headers, id, "run/get", json!({"run_id": id})).await
}
async fn list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    run_call(state, headers, id, "task/list", json!({"run_id": id})).await
}
async fn get_budget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    run_call(state, headers, id, "budget/snapshot", json!({"run_id": id})).await
}
#[derive(Default, Deserialize)]
struct EventQuery {
    after_sequence: Option<i64>,
    limit: Option<usize>,
}
async fn list_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<EventQuery>,
) -> Response {
    run_call(state, headers, id, "event/list", json!({"run_id": id, "after_sequence": query.after_sequence.unwrap_or(0), "limit": query.limit.unwrap_or(500)})).await
}
async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    call(state, headers, "model/list", json!({})).await
}
async fn put_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<Value>,
) -> Response {
    call(state, headers, "memory/put", params).await
}
async fn delete_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<Value>,
) -> Response {
    call(state, headers, "memory/delete", params).await
}
#[derive(Default, Deserialize)]
struct MemoryQuery {
    query: Option<String>,
    scope: Option<String>,
    limit: Option<usize>,
}
async fn search_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MemoryQuery>,
) -> Response {
    call(state, headers, "memory/search", json!({"query": query.query.unwrap_or_default(), "scope": query.scope, "limit": query.limit.unwrap_or(100)})).await
}
async fn policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<Value>,
) -> Response {
    call(state, headers, "policy/evaluate", params).await
}

async fn stream_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<EventQuery>,
) -> Response {
    if let Err(response) = check_access(&state, &headers).and_then(|()| check_run(&state, id)) {
        return failure(response.0, response.1);
    }
    let mut after = query.after_sequence.unwrap_or(0);
    if let Some(value) = headers.get("last-event-id") {
        match value.to_str().ok().and_then(|s| s.parse::<i64>().ok()) {
            Some(value) if value >= 0 => after = after.max(value),
            _ => {
                return failure(
                    StatusCode::BAD_REQUEST,
                    "Last-Event-ID must be a non-negative sequence",
                );
            }
        }
    }
    if after < 0 {
        return failure(
            StatusCode::BAD_REQUEST,
            "after_sequence must be non-negative",
        );
    }
    // Read the durable ledger in bounded pages. Unlike a broadcast-only stream,
    // reconnects replay missed events and a slow consumer cannot lose events.
    let stream = stream::unfold(
        (state.engine.clone(), after, VecDeque::new(), false),
        move |(engine, mut after, mut pending, failed)| async move {
            if failed {
                return None;
            }
            loop {
                if let Some(event) = pending.pop_front() {
                    let event: EventEnvelope = event;
                    after = event.sequence;
                    let frame = Event::default()
                        .event("audit")
                        .id(after.to_string())
                        .json_data(&event)
                        .expect("event is serializable");
                    return Some((Ok::<_, Infallible>(frame), (engine, after, pending, false)));
                }
                match engine.store.list_events(id, after, 256) {
                    Ok(events) if !events.is_empty() => pending.extend(events),
                    Ok(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                    Err(error) => {
                        tracing::error!(%error, run_id = %id, "event stream failed");
                        let frame = Event::default()
                            .event("error")
                            .data("audit stream unavailable; reconnect to resume");
                        return Some((Ok(frame), (engine, after, pending, true)));
                    }
                }
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    pub(crate) fn fixture() -> (tempfile::TempDir, AppState, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = OpenForgeConfig::load("../../openforge.yaml").unwrap();
        config.state_db = dir.path().join("state.db").to_string_lossy().into();
        config.artifact_dir = dir.path().join("artifacts").to_string_lossy().into();
        let artifacts = ArtifactStore::open(&config.artifact_dir).unwrap();
        let state = AppState {
            engine: Arc::new(Engine::new(config).unwrap()),
            artifacts,
            telemetry: TelemetryRegistry::default(),
            api_token: Some(Arc::from("test-token")),
            started_at: Instant::now(),
        };
        let id = Uuid::new_v4();
        let run = serde_json::from_value(json!({
            "id": id, "project_id": Uuid::new_v4(), "objective": "test", "base_sha": "abc",
            "status": "planning", "autonomy": "suggest", "budget": {"currency": "USD", "soft_limit": 5.0, "hard_limit": 10.0, "spent": 0.0},
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        })).unwrap();
        state.engine.store.create_run(&run).unwrap();
        (dir, state, id)
    }
    fn request(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap()
    }
    #[tokio::test]
    async fn execution_rejects_empty_plans_and_records_runtime_failures() {
        let (dir, state, id) = fixture();
        assert!(
            state
                .engine
                .execute_run_with_backend(
                    dir.path(),
                    id,
                    AgentPolicy::default(),
                    RunnerBackend::Local
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("planned")
        );
        let task = serde_json::from_value(json!({
            "id": Uuid::new_v4(), "run_id": id, "title":"test", "description":"test", "role":"backend",
            "dependencies":[], "required_reviews":[], "acceptance":[], "status":"pending", "attempts":0, "max_attempts":1,
            "budget":{"max_usd":1.0,"max_model_calls":1,"max_tool_calls":1,"max_wall_seconds":10},
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"
        })).unwrap();
        state.engine.store.upsert_task(&task).unwrap();
        state.engine.store.claim_run(id).unwrap();
        assert!(state.engine.store.claim_run(id).is_err());
        state
            .engine
            .store
            .update_run_status(id, openforge_protocol::RunStatus::Paused)
            .unwrap();
        assert!(
            state
                .engine
                .execute_run_with_backend(
                    &dir.path().join("missing-repo"),
                    id,
                    AgentPolicy::default(),
                    RunnerBackend::Local
                )
                .await
                .is_err()
        );
        assert_eq!(
            state.engine.store.get_run(id).unwrap().unwrap().status,
            openforge_protocol::RunStatus::Failed
        );
        assert!(
            state
                .engine
                .store
                .list_events(id, 0, 10)
                .unwrap()
                .iter()
                .any(|event| event.event_type == "run.failed")
        );
    }

    #[tokio::test]
    async fn over_budget_provider_response_is_still_accounted() {
        let (_dir, state, id) = fixture();
        let server = Router::new().route("/chat/completions", post(|| async {
            Json(json!({"id":"test", "choices":[{"message":{"content":"test"}}], "usage":{"prompt_tokens":1000000,"completion_tokens":1}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        let mut config = state.engine.config.clone();
        config.providers[0].base_url = format!("http://{address}");
        config.providers[0].models[0].input_usd_per_million = 1.0;
        config.providers[0].models[0].latency_score = 0.0;
        let engine = Engine::new(config).unwrap();
        let result = engine
            .completion(CompletionInput {
                run_id: id,
                file_path: "test.rs".into(),
                language: "rust".into(),
                prefix: "fn".into(),
                suffix: String::new(),
                max_output_tokens: 16,
                max_cost_usd: 0.1,
            })
            .await;
        server.abort();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("hard cost budget"), "{error}");
        assert_eq!(engine.store.run_cost(id).unwrap(), 1.0);
    }

    #[tokio::test]
    async fn gateway_setup_requires_daemon_authentication() {
        let (_dir, state, _) = fixture();
        let app = Router::new()
            .route("/v1/rpc", post(crate::rpc))
            .with_state(state);
        let request = Request::builder().method("POST").uri("/v1/rpc")
            .header("content-type", "application/json")
            .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"gateway/connect","params":{"api_key":"test-key"}}).to_string())).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), 10000).await.unwrap();
        assert!(
            !String::from_utf8(body.to_vec())
                .unwrap()
                .contains("test-key")
        );
    }

    #[tokio::test]
    async fn rest_requires_auth_and_lists_persisted_runs() {
        let (_dir, state, id) = fixture();
        let app = routes().with_state(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/runs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(request("/v1/runs?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value[0]["id"], id.to_string());
        let response = app
            .clone()
            .oneshot(request("/v1/runs?limit=1&offset=1"))
            .await
            .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value, json!([]));
        let response = app
            .oneshot(request(&format!("/v1/runs/{}", Uuid::new_v4())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    #[tokio::test]
    async fn stream_replays_after_cursor_and_delivers_new_events() {
        let (_dir, state, id) = fixture();
        let append = || {
            state
                .engine
                .store
                .append_event(
                    Some(id),
                    None,
                    openforge_protocol::Actor {
                        kind: "system".into(),
                        id: "test".into(),
                    },
                    "test.event",
                    json!({}),
                )
                .unwrap()
        };
        let first = append();
        let second = append();
        let app = routes().with_state(state.clone());
        let mut req = request(&format!("/v1/runs/{id}/events/stream"));
        req.headers_mut()
            .insert("last-event-id", first.sequence.to_string().parse().unwrap());
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        let text = String::from_utf8(frame.to_vec()).unwrap();
        assert!(text.contains(&second.event_id.to_string()));
        assert!(!text.contains(&first.event_id.to_string()));
        let third = append();
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(
            String::from_utf8(frame.to_vec())
                .unwrap()
                .contains(&third.event_id.to_string())
        );
        let response = app
            .clone()
            .oneshot(request(&format!(
                "/v1/runs/{id}/events/stream?after_sequence=-1"
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/runs/{id}/events/stream"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn rest_memory_round_trip_and_model_catalog() {
        let (_dir, state, _) = fixture();
        let app = routes().with_state(state);
        for value in ["old", "new"] {
            let req = Request::builder()
                .method("POST")
                .uri("/v1/memory")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"scope":"project", "key":"example", "value":value}).to_string(),
                ))
                .unwrap();
            assert_eq!(
                app.clone().oneshot(req).await.unwrap().status(),
                StatusCode::OK
            );
        }
        let response = app
            .clone()
            .oneshot(request("/v1/memory/search?query=example"))
            .await
            .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["value"], "new");
        let response = app.oneshot(request("/v1/models")).await.unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(value[0]["provider"], "local");
    }
}
