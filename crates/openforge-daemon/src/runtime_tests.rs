//! Exercise the real JSON-RPC boundary with temporary persistent state.
use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

async fn rpc_value(state: &AppState, method: &str, params: Value) -> Value {
    let app = Router::new()
        .route("/v1/rpc", post(rpc))
        .with_state(state.clone());
    let request = Request::builder()
        .method("POST")
        .uri("/v1/rpc")
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":7,"method":method,"params":params}).to_string(),
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap()
}

#[tokio::test]
async fn stream_round_trip_and_abort_through_rpc() {
    let (_dir, state, _) = http_api::tests::fixture();
    let begin = rpc_value(
        &state,
        "artifact/stream/begin",
        json!({"media_type":"text/plain"}),
    )
    .await;
    let id = &begin["result"]["upload_id"];
    for data in ["hello ", "world"] {
        let response = rpc_value(
            &state,
            "artifact/stream/chunk",
            json!({"upload_id":id,"base64":BASE64.encode(data)}),
        )
        .await;
        assert!(response.get("error").is_none(), "{response}");
    }
    let committed = rpc_value(
        &state,
        "artifact/stream/commit",
        json!({"upload_id":id,"metadata":{"test":true}}),
    )
    .await;
    assert_eq!(committed["result"]["bytes"], 11);
    let result = rpc_value(
        &state,
        "artifact/get",
        json!({"sha256":committed["result"]["digest"]}),
    )
    .await;
    assert_eq!(result["result"]["base64"], BASE64.encode("hello world"));
    let rejected = rpc_value(
        &state,
        "artifact/stream/chunk",
        json!({"upload_id":id,"base64":"YQ=="}),
    )
    .await;
    assert!(rejected.get("error").is_some());
    let begin = rpc_value(&state, "artifact/stream/begin", json!({})).await;
    let abort = rpc_value(
        &state,
        "artifact/stream/abort",
        json!({"upload_id":begin["result"]["upload_id"]}),
    )
    .await;
    assert_eq!(abort["result"]["aborted"], true);
    let invalid = rpc_value(
        &state,
        "artifact/stream/abort",
        json!({"upload_id":"../../outside"}),
    )
    .await;
    assert!(invalid.get("error").is_some());
}

#[tokio::test]
async fn budget_reservations_persist_and_cannot_settle_twice() {
    let (_dir, state, run) = http_api::tests::fixture();
    let reserved = rpc_value(
        &state,
        "budget/reserve",
        json!({"run_id":run,"estimated_usd":0.5}),
    )
    .await;
    assert!(reserved.get("error").is_none(), "{reserved}");
    let id = &reserved["result"]["reservation_id"];
    let settled = rpc_value(
        &state,
        "budget/settle",
        json!({"reservation_id":id,"actual_usd":0.4}),
    )
    .await;
    assert!(settled["result"]["settled_at"].is_string(), "{settled}");
    let duplicate = rpc_value(
        &state,
        "budget/settle",
        json!({"reservation_id":id,"actual_usd":0.4}),
    )
    .await;
    assert!(duplicate.get("error").is_some());
    let reopened = openforge_store::Store::open(&state.engine.config.state_db).unwrap();
    assert_eq!(reopened.budget_usage(run).unwrap(), (0.0, 0.4));
    let snapshot = rpc_value(&state, "budget/snapshot", json!({"run_id":run})).await;
    assert_eq!(snapshot["result"]["run_spent_usd"], 0.4, "{snapshot}");
    let invalid = rpc_value(
        &state,
        "budget/reserve",
        json!({"run_id":run,"estimated_usd":-1}),
    )
    .await;
    assert!(invalid.get("error").is_some());
}

#[tokio::test]
async fn secret_lease_enforces_policy_persists_metadata_and_revokes() {
    let (dir, state, _) = http_api::tests::fixture();
    // PATH is non-secret test data already present in every test process.
    let policy = dir.path().join("policy.yaml");
    std::fs::write(
        &policy,
        "autonomy: autonomous\nsecrets:\n  default: deny\n  allow: [\"secret://PATH\"]\n",
    )
    .unwrap();
    let lease = rpc_value(
        &state,
        "secret/lease",
        json!({"secret_name":"PATH","audience":"test","policy_path":policy,"ttl_seconds":500}),
    )
    .await;
    assert!(lease.get("error").is_none(), "{lease}");
    assert_eq!(lease["result"]["value"], std::env::var("PATH").unwrap());
    let id = &lease["result"]["lease"]["id"];
    let listed = rpc_value(&state, "secret/list", json!({})).await;
    assert_eq!(listed["result"].as_array().unwrap().len(), 1);
    assert!(listed["result"][0].get("value").is_none());
    let revoked = rpc_value(&state, "secret/revoke", json!({"lease_id":id})).await;
    assert_eq!(revoked["result"]["revoked"], true, "{revoked}");
    let denied = rpc_value(
        &state,
        "secret/lease",
        json!({"secret_name":"NOT_ALLOWED","audience":"test","policy_path":policy}),
    )
    .await;
    assert!(denied.get("error").is_some());
}

#[tokio::test]
async fn mcp_operations_are_denied_before_spawning_unknown_servers() {
    let (dir, state, _) = http_api::tests::fixture();
    let policy = dir.path().join("deny.yaml");
    std::fs::write(&policy, "mcp:\n  default: deny\n").unwrap();
    for method in [
        "mcp/list_tools",
        "mcp/call_tool",
        "mcp/list_resources",
        "mcp/read_resource",
        "mcp/list_prompts",
    ] {
        let result = rpc_value(&state, method, json!({"server_name":"not-configured","tool_name":"test","uri":"test://x","policy_path":policy})).await;
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap()
                .contains("denied"),
            "{result}"
        );
    }
}

#[tokio::test]
async fn acp_spawn_request_notify_close_persists_lifecycle() {
    let (dir, state, _) = http_api::tests::fixture();
    let policy = dir.path().join("policy.yaml");
    std::fs::write(
        &policy,
        "autonomy: execute\nacp:\n  default: deny\n  allow: [python3]\n",
    )
    .unwrap();
    let script = "import sys,json\nfor line in sys.stdin:\n r=json.loads(line)\n if 'id' in r: print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':r.get('params')}),flush=True)\n";
    let spawned = rpc_value(
        &state,
        "acp/spawn",
        json!({"program":"python3","args":["-u","-c",script],"policy_path":policy}),
    )
    .await;
    assert!(spawned.get("error").is_none(), "{spawned}");
    let id = &spawned["result"]["process_id"];
    let result = rpc_value(
        &state,
        "acp/request",
        json!({"process_id":id,"method":"echo","params":{"ok":true}}),
    )
    .await;
    assert_eq!(result["result"]["result"]["ok"], true, "{result}");
    let notify = rpc_value(
        &state,
        "acp/notify",
        json!({"process_id":id,"method":"notification"}),
    )
    .await;
    assert_eq!(notify["result"]["ok"], true);
    let listed = rpc_value(&state, "acp/list", json!({})).await;
    assert_eq!(listed["result"].as_array().unwrap().len(), 1);
    let closed = rpc_value(&state, "acp/close", json!({"process_id":id})).await;
    assert_eq!(closed["result"]["closed"], true);
    let records = state.engine.store.list_acp_processes().unwrap();
    assert_eq!(records[0]["active"], false);
    assert!(records[0]["closed_at"].is_string());
    assert!(state.engine.acp_clients.lock().await.is_empty());
}

#[tokio::test]
async fn mcp_discovery_calls_resources_and_prompts_reach_configured_process() {
    let (dir, mut state, _) = http_api::tests::fixture();
    let policy = dir.path().join("policy.yaml");
    std::fs::write(
        &policy,
        "autonomy: execute\nmcp:\n  default: deny\n  allow: [\"*\"]\n",
    )
    .unwrap();
    let script = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 m=r['method']
 responses={
 'initialize':{'protocolVersion':'2024-11-05','capabilities':{},'serverInfo':{'name':'test','version':'1'}},
 'tools/list':{'tools':[{'name':'echo','inputSchema':{'type':'object'}}]},
 'tools/call':{'content':[{'type':'text','text':'done'}],'isError':False},
 'resources/list':{'resources':[{'uri':'test://value','name':'value'}]},
 'resources/read':{'contents':[{'uri':'test://value','text':'stored'}]},
 'prompts/list':{'prompts':[{'name':'review'}]}}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':responses[m]}),flush=True)
"#;
    let mut config = state.engine.config.clone();
    config.mcp_servers = serde_json::from_value(
        json!([{"name":"fixture","program":"python3","args":["-u","-c",script]}]),
    )
    .unwrap();
    state.engine = Arc::new(Engine::new(config).unwrap());
    for (method, pointer, expected) in [
        ("mcp/list_tools", "/result/0/name", "echo"),
        ("mcp/call_tool", "/result/content/0/text", "done"),
        (
            "mcp/list_resources",
            "/result/resources/0/uri",
            "test://value",
        ),
        ("mcp/read_resource", "/result/contents/0/text", "stored"),
        ("mcp/list_prompts", "/result/prompts/0/name", "review"),
    ] {
        let result = rpc_value(&state, method, json!({"server_name":"fixture","tool_name":"echo","arguments":{},"uri":"test://value","policy_path":policy})).await;
        assert_eq!(result.pointer(pointer), Some(&json!(expected)), "{result}");
    }
}
