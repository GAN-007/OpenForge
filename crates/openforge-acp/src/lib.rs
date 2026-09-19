use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Duration, timeout},
};

#[derive(Debug, Clone)]
pub struct AcpProcessConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
}

impl AcpProcessConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            environment: BTreeMap::new(),
            request_timeout: Duration::from_secs(120),
            max_response_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

pub struct AcpAgentClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    request_timeout: Duration,
    max_response_bytes: usize,
}

impl AcpAgentClient {
    pub async fn spawn(program: &str, args: &[String]) -> Result<Self> {
        Self::spawn_with_config(AcpProcessConfig::new(program, args.to_vec())).await
    }

    pub async fn spawn_with_config(config: AcpProcessConfig) -> Result<Self> {
        if config.program.trim().is_empty() {
            bail!("ACP program cannot be empty");
        }
        if config.max_response_bytes == 0 {
            bail!("ACP max_response_bytes must be greater than zero");
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
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        for (key, value) in &config.environment {
            validate_env_key(key)?;
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("spawn ACP agent {}", config.program))?;
        let stdin = child.stdin.take().context("ACP stdin unavailable")?;
        let stdout = child.stdout.take().context("ACP stdout unavailable")?;

        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: AtomicU64::new(1),
            request_timeout: config.request_timeout,
            max_response_bytes: config.max_response_bytes,
        })
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        if method.trim().is_empty() {
            bail!("ACP notification method cannot be empty");
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
            bail!("ACP request method cannot be empty");
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
                let line = self.lines.next_line().await?.context("ACP agent exited")?;
                if line.trim().is_empty() {
                    continue;
                }
                if line.len() > self.max_response_bytes {
                    bail!("ACP response exceeded {} bytes", self.max_response_bytes);
                }

                let value: Value = serde_json::from_str(&line).context("invalid ACP JSON")?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }

                if let Some(error) = value.get("error") {
                    let parsed: AcpError =
                        serde_json::from_value(error.clone()).unwrap_or(AcpError {
                            code: -32000,
                            message: error.to_string(),
                            data: None,
                        });
                    bail!("ACP error {}: {}", parsed.code, parsed.message);
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        })
        .await
        .context("ACP request timed out")?
    }

    pub async fn close(mut self) -> Result<()> {
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
        }
        Ok(())
    }

    async fn write(&mut self, value: &Value) -> Result<()> {
        let payload = serde_json::to_string(value)?;
        if payload.len() > self.max_response_bytes {
            bail!("ACP outbound message exceeds configured limit");
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
        bail!("invalid ACP environment variable name {key:?}");
    }
    Ok(())
}
