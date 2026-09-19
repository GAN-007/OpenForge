use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn tools(&self) -> Vec<ServerTool>;
    async fn call(&self, name: &str, arguments: Value) -> Result<Value>;
}

pub async fn run_mcp_stdio(handler: Arc<dyn ToolHandler>) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_json_line(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": -32700, "message": error.to_string()}
                    }),
                )
                .await?;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "serverInfo": {
                        "name": "openforge",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"tools": {"listChanged": false}}
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": handler.tools().into_iter().map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "inputSchema": tool.input_schema
                        })
                    }).collect::<Vec<_>>()
                }
            }),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match handler.call(name, arguments).await {
                    Ok(result) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&result)?
                            }],
                            "structuredContent": result,
                            "isError": false
                        }
                    }),
                    Err(error) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": error.to_string()}],
                            "isError": true
                        }
                    }),
                }
            }
            "notifications/initialized" => continue,
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("unknown MCP method {method}")}
            }),
        };

        write_json_line(&mut stdout, &response).await?;
    }
    Ok(())
}

#[async_trait]
pub trait AcpHandler: Send + Sync {
    async fn create_session(&self, cwd: &str, metadata: Value) -> Result<Value>;
    async fn prompt(&self, session_id: &str, prompt: Value) -> Result<Value>;
    async fn cancel(&self, session_id: &str) -> Result<()>;
}

pub async fn run_acp_stdio(handler: Arc<dyn AcpHandler>) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).context("parse ACP JSON-RPC request")?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let result: Result<Value> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": 1,
                "agentInfo": {
                    "name": "OpenForge",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "agentCapabilities": {
                    "loadSession": false,
                    "promptCapabilities": {
                        "image": true,
                        "audio": false,
                        "embeddedContext": true
                    },
                    "mcpCapabilities": {
                        "http": true,
                        "sse": true
                    }
                }
            })),
            "session/new" => {
                let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or(".");
                handler
                    .create_session(cwd, params.get("meta").cloned().unwrap_or(Value::Null))
                    .await
            }
            "session/prompt" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .context("ACP session/prompt missing sessionId")?;
                handler
                    .prompt(
                        session_id,
                        params.get("prompt").cloned().unwrap_or(Value::Null),
                    )
                    .await
            }
            "session/cancel" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .context("ACP session/cancel missing sessionId")?;
                handler.cancel(session_id).await?;
                Ok(json!({"cancelled": true}))
            }
            _ => bail!("unknown ACP method {method}"),
        };

        let response = match result {
            Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
            Err(error) => json!({
                "jsonrpc":"2.0",
                "id":id,
                "error":{"code":-32000,"message":error.to_string()}
            }),
        };
        write_json_line(&mut stdout, &response).await?;
    }
    Ok(())
}

async fn write_json_line(stdout: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    let mut payload = serde_json::to_vec(value)?;
    payload.push(b'\n');
    stdout.write_all(&payload).await?;
    stdout.flush().await?;
    Ok(())
}
