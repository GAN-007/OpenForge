use axum::{
    Json, Router,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn listing_is_local_and_failed_execution_keeps_terminal_open() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(directory.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(directory.path().join("README.md"), "fixture").unwrap();
    std::fs::write(directory.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir(directory.path().join("target")).unwrap();
    std::fs::write(directory.path().join("target/ignored"), "fixture").unwrap();
    let policy = directory.path().join("policy.yaml");
    std::fs::write(&policy, "autonomy: execute\n").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let count = runs.clone();
    let app = Router::new().route("/health", get(|| async {Json(json!({"status":"ok","protocol":"openforge.protocol.v2"}))}))
        .route("/v1/rpc", post(move |Json(request): Json<Value>| {
            let count = count.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "gateway/status" => json!({"connected":true}),
                    "model/list" => json!([]),
                    "run/create" => {assert_eq!(request["params"]["budget_usd"], 2.0); count.fetch_add(1, Ordering::SeqCst); json!({"id":"00000000-0000-4000-8000-000000000001"})},
                    "run/plan" => json!([{"title":"test"}]),
                    "run/execute" if count.load(Ordering::SeqCst) == 1 => return Json(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"provider returned HTTP 400"}})),
                    "run/execute" => json!({"integration_branch":"of/integration/test"}),
                    "run/get" => json!({"status":"completed"}),
                    "event/list" => json!([{"event_type":"task.completed","payload":{"summary":"VERIFIED OUTPUT"},"sequence":1}]),
                    other => panic!("unexpected RPC {other}"),
                };
                Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_openforge"))
        .args(["--daemon-url", &format!("http://{address}"), "chat"])
        .arg(directory.path())
        .env("XDG_STATE_HOME", directory.path().join("private-state"))
        .arg("--policy")
        .arg(policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/\n/status\n/budget 2\n/plan\n/mode execute\n/new\n/fork\n/resume\n/compact\n/connect should-not-be-forwarded\n/review unsupported-argument\n/not-a-command\n/init\n/init\nlist all files and folders\nlist all files in this folder\nwhoami\npwd\nls\nFix a test\nSuccess\n/help\n/quit\n")
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    server.abort();
    assert!(result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.contains("README.md"));
    assert!(!stdout.contains("target/ignored"));
    assert!(stdout.contains("list workspace files locally"));
    assert!(stdout.contains("No model request was sent"));
    assert!(stdout.contains("Forked session:"));
    assert!(stdout.matches("openforge> ").count() >= 8);
    assert_eq!(stdout.matches("Workspace files and folders").count(), 3);
    let identity = std::process::Command::new("id")
        .arg("-un")
        .output()
        .unwrap();
    assert!(stdout.contains(String::from_utf8(identity.stdout).unwrap().trim()));
    assert!(stderr.contains("terminal remains open"));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(stdout.contains("VERIFIED OUTPUT"));
    let saved = std::fs::read_dir(directory.path().join("private-state/openforge/sessions"))
        .unwrap()
        .filter_map(|entry| {
            serde_json::from_slice::<Value>(&std::fs::read(entry.ok()?.path()).ok()?).ok()
        })
        .any(|value| value["transcript"].to_string().contains("VERIFIED OUTPUT"));
    assert!(saved, "verified task summaries must survive a session save");
    assert!(directory.path().join("AGENTS.md").exists());
}

#[test]
#[cfg(unix)]
fn real_terminal_palette_and_editing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new("python3")
        .arg(root.join("tests/test_terminal_pty.py"))
        .env("OPENFORGE_TEST_BINARY", env!("CARGO_BIN_EXE_openforge"))
        .status()
        .unwrap();
    assert!(status.success());
}
