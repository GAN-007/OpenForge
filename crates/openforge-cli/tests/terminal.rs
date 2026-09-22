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
                    "run/create" => {count.fetch_add(1, Ordering::SeqCst); json!({"id":"00000000-0000-4000-8000-000000000001"})},
                    "run/plan" => json!([{"title":"test"}]),
                    "run/execute" => return Json(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"provider returned HTTP 400"}})),
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
        .write_all(b"list all files and folders\nFix a test\n/help\n/quit\n")
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
    assert!(stdout.contains("/files       list workspace"));
    assert!(stdout.matches("openforge> ").count() >= 4);
    assert!(stderr.contains("terminal remains open"));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}
