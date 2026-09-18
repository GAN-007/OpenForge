use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{timeout, Duration},
};

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone)]
pub struct McpProcessConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
}

impl McpProcessConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            environment: BTreeMap::new(),
            request_timeout: Duration::from_secs(60),
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "serverInfo")]
    pub server_info: Option<Value>,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCallResult {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
    #[serde(default)]
    pub structured_content: Option<Value>,
}

pub struct McpStdioClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    request_timeout: Duration,
    max_response_bytes: usize,
    initialized: bool,
}

impl McpStdioClient {
    pub async fn spawn(program: &str, args: &[String]) -> Result<Self> {
        Self::spawn_with_config(McpProcessConfig::new(program, args.to_vec())).await
    }

    pub async fn spawn_with_config(config: McpProcessConfig) -> Result<Self> {
        if config.program.trim().is_empty() {
            bail!("MCP program cannot be empty");
        }
        if config.max_response_bytes == 0 {
            bail!("MCP max_response_bytes must be greater than zero");
        }

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);

        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }

        if !config.environment.is_empty() {
            command.env_clear();
            if let Some(path) = std::env::var_os("PATH") {
                command.env("PATH", path);
            }
            for (key, value) in &config.environment {
                validate_env_key(key)?;
                command.env(key, value);
            }
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("spawn MCP server {}", config.program))?;
        let stdin = child.stdin.take().context("MCP stdin unavailable")?;
        let stdout = child.stdout.take().context("MCP stdout unavailable")?;

        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: AtomicU64::new(1),
            request_timeout: config.request_timeout,
            max_response_bytes: config.max_response_bytes,
            initialized: false,
        })
    }

    pub async fn initialize(
        &mut self,
        client_name: &str,
        client_version: &str,
    ) -> Result<McpServerInfo> {
        if self.initialized {
            bail!("MCP client is already initialized");
        }
        if client_name.trim().is_empty() || client_version.trim().is_empty() {
            bail!("MCP client name and version are required");
        }

        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": client_name,
                        "version": client_version
                    }
                }),
            )
            .await?;

        let server: McpServerInfo =
            serde_json::from_value(result).context("invalid MCP initialize result")?;
        if server.protocol_version.trim().is_empty() {
            bail!("MCP server returned an empty protocolVersion");
        }

        self.notify("notifications/initialized", json!({})).await?;
        self.initialized = true;
        Ok(server)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        self.require_initialized()?;
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .cloned()
            .context("MCP tools/list response missing tools")?;
        Ok(serde_json::from_value(tools).context("invalid MCP tool list")?)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResult> {
        self.require_initialized()?;
        if name.trim().is_empty() {
            bail!("MCP tool name cannot be empty");
        }

        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments
                }),
            )
            .await?;

        let call: McpToolCallResult =
            serde_json::from_value(result).context("invalid MCP tool result")?;
        if call.is_error {
            bail!("MCP tool {name} reported an error: {:?}", call.content);
        }
        Ok(call)
    }

    pub async fn list_resources(&mut self) -> Result<Value> {
        self.require_initialized()?;
        self.request("resources/list", json!({})).await
    }

    pub async fn read_resource(&mut self, uri: &str) -> Result<Value> {
        self.require_initialized()?;
        if uri.trim().is_empty() {
            bail!("MCP resource URI cannot be empty");
        }
        self.request("resources/read", json!({"uri": uri})).await
    }

    pub async fn list_prompts(&mut self) -> Result<Value> {
        self.require_initialized()?;
        self.request("prompts/list", json!({})).await
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        if method.trim().is_empty() {
            bail!("MCP notification method cannot be empty");
        }
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        if method.trim().is_empty() {
            bail!("MCP request method cannot be empty");
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;

        timeout(self.request_timeout, async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await?
                    .context("MCP server closed stdout")?;
                if line.trim().is_empty() {
                    continue;
                }
                if line.len() > self.max_response_bytes {
                    bail!(
                        "MCP response exceeded {} bytes",
                        self.max_response_bytes
                    );
                }

                let value: Value =
                    serde_json::from_str(&line).context("invalid MCP JSON")?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }

                if let Some(error) = value.get("error") {
                    bail!("MCP error: {error}");
                }
                return value
                    .get("result")
                    .cloned()
                    .context("MCP response missing result");
            }
        })
        .await
        .context("MCP request timed out")?
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if self.initialized {
            let _ = self.notify("notifications/cancelled", json!({})).await;
        }
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
        }
        Ok(())
    }

    fn require_initialized(&self) -> Result<()> {
        if !self.initialized {
            bail!("MCP client must be initialized first");
        }
        Ok(())
    }

    async fn write(&mut self, value: &Value) -> Result<()> {
        let payload = serde_json::to_string(value)?;
        if payload.len() > self.max_response_bytes {
            bail!("MCP outbound message exceeds configured limit");
        }
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid MCP environment variable name {key:?}");
    }
    Ok(())
}
