use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Duration, timeout},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapProcessConfig {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: usize,
}

fn default_timeout_seconds() -> u64 {
    60
}

fn default_max_message_bytes() -> usize {
    16 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapEvent {
    pub event: String,
    #[serde(default)]
    pub body: Value,
}

pub struct DapClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sequence: u64,
    request_timeout: Duration,
    max_message_bytes: usize,
    events: VecDeque<DapEvent>,
}

impl DapClient {
    pub async fn spawn(config: &DapProcessConfig) -> Result<Self> {
        if config.name.trim().is_empty() || config.program.trim().is_empty() {
            bail!("DAP name and program are required");
        }

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .env_clear();

        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        for (key, value) in &config.environment {
            validate_env_key(key)?;
            command.env(key, value);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("spawn debug adapter {}", config.name))?;
        let stdin = child.stdin.take().context("DAP stdin unavailable")?;
        let stdout = child.stdout.take().context("DAP stdout unavailable")?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            sequence: 1,
            request_timeout: Duration::from_secs(config.timeout_seconds.max(1)),
            max_message_bytes: config.max_message_bytes.max(1024),
            events: VecDeque::new(),
        })
    }

    pub async fn initialize(&mut self, adapter_id: &str) -> Result<Value> {
        self.request(
            "initialize",
            json!({
                "clientID": "openforge",
                "clientName": "OpenForge",
                "adapterID": adapter_id,
                "pathFormat": "path",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "supportsVariableType": true,
                "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": true,
                "supportsMemoryReferences": true,
                "supportsProgressReporting": true,
                "supportsInvalidatedEvent": true
            }),
        )
        .await
    }

    pub async fn launch(&mut self, arguments: Value) -> Result<Value> {
        self.request("launch", arguments).await
    }

    pub async fn attach(&mut self, arguments: Value) -> Result<Value> {
        self.request("attach", arguments).await
    }

    pub async fn set_breakpoints(&mut self, source_path: &str, lines: &[u32]) -> Result<Value> {
        self.request(
            "setBreakpoints",
            json!({
                "source": {"path": source_path},
                "breakpoints": lines.iter().map(|line| json!({"line": line})).collect::<Vec<_>>(),
                "sourceModified": false
            }),
        )
        .await
    }

    pub async fn configuration_done(&mut self) -> Result<Value> {
        self.request("configurationDone", json!({})).await
    }

    pub async fn threads(&mut self) -> Result<Value> {
        self.request("threads", json!({})).await
    }

    pub async fn stack_trace(
        &mut self,
        thread_id: i64,
        start_frame: u64,
        levels: u64,
    ) -> Result<Value> {
        self.request(
            "stackTrace",
            json!({
                "threadId": thread_id,
                "startFrame": start_frame,
                "levels": levels
            }),
        )
        .await
    }

    pub async fn scopes(&mut self, frame_id: i64) -> Result<Value> {
        self.request("scopes", json!({"frameId": frame_id})).await
    }

    pub async fn variables(
        &mut self,
        variables_reference: i64,
        start: Option<u64>,
        count: Option<u64>,
    ) -> Result<Value> {
        self.request(
            "variables",
            json!({
                "variablesReference": variables_reference,
                "start": start,
                "count": count
            }),
        )
        .await
    }

    pub async fn evaluate(
        &mut self,
        expression: &str,
        frame_id: Option<i64>,
        context: &str,
    ) -> Result<Value> {
        self.request(
            "evaluate",
            json!({
                "expression": expression,
                "frameId": frame_id,
                "context": context
            }),
        )
        .await
    }

    pub async fn continue_thread(&mut self, thread_id: i64) -> Result<Value> {
        self.request("continue", json!({"threadId": thread_id}))
            .await
    }

    pub async fn next(&mut self, thread_id: i64) -> Result<Value> {
        self.request("next", json!({"threadId": thread_id})).await
    }

    pub async fn step_in(&mut self, thread_id: i64) -> Result<Value> {
        self.request("stepIn", json!({"threadId": thread_id})).await
    }

    pub async fn step_out(&mut self, thread_id: i64) -> Result<Value> {
        self.request("stepOut", json!({"threadId": thread_id}))
            .await
    }

    pub async fn pause(&mut self, thread_id: i64) -> Result<Value> {
        self.request("pause", json!({"threadId": thread_id})).await
    }

    pub async fn request(&mut self, command: &str, arguments: Value) -> Result<Value> {
        if command.trim().is_empty() {
            bail!("DAP command cannot be empty");
        }

        let request_seq = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        self.write_message(&json!({
            "seq": request_seq,
            "type": "request",
            "command": command,
            "arguments": arguments
        }))
        .await?;

        timeout(self.request_timeout, async {
            loop {
                let message = read_message(&mut self.stdout, self.max_message_bytes).await?;
                match message.get("type").and_then(Value::as_str) {
                    Some("event") => {
                        let event = message
                            .get("event")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string();
                        self.events.push_back(DapEvent {
                            event,
                            body: message.get("body").cloned().unwrap_or(Value::Null),
                        });
                    }
                    Some("response")
                        if message.get("request_seq").and_then(Value::as_u64)
                            == Some(request_seq) =>
                    {
                        if !message
                            .get("success")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            bail!(
                                "DAP command {command} failed: {}",
                                message
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown error")
                            );
                        }
                        return Ok(message.get("body").cloned().unwrap_or(Value::Null));
                    }
                    _ => {}
                }
            }
        })
        .await
        .with_context(|| format!("DAP command {command} timed out"))?
    }

    pub fn drain_events(&mut self) -> Vec<DapEvent> {
        self.events.drain(..).collect()
    }

    pub async fn disconnect(mut self, terminate_debuggee: bool) -> Result<()> {
        let _ = self
            .request(
                "disconnect",
                json!({"terminateDebuggee": terminate_debuggee}),
            )
            .await;
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        Ok(())
    }

    async fn write_message(&mut self, value: &Value) -> Result<()> {
        let payload = serde_json::to_vec(value)?;
        if payload.len() > self.max_message_bytes {
            bail!("DAP outbound message exceeds configured limit");
        }
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(&payload).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

async fn read_message(
    reader: &mut BufReader<ChildStdout>,
    max_message_bytes: usize,
) -> Result<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            bail!("debug adapter closed stdout");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid DAP Content-Length")?,
            );
        }
    }

    let length = content_length.context("DAP message missing Content-Length")?;
    if length > max_message_bytes {
        bail!("DAP message exceeds configured limit");
    }

    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload).context("invalid DAP JSON payload")?)
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid DAP environment variable name {key:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_environment_names() {
        assert!(validate_env_key("PATH").is_ok());
        assert!(validate_env_key("BAD=VALUE").is_err());
    }
}
