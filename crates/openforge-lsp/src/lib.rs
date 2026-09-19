use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{timeout, Duration},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspProcessConfig {
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
    #[serde(default)]
    pub initialization_options: Value,
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_max_message_bytes() -> usize {
    16 * 1024 * 1024
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    request_timeout: Duration,
    max_message_bytes: usize,
    notifications: VecDeque<Value>,
    initialized: bool,
}

impl LspClient {
    pub async fn spawn(config: &LspProcessConfig) -> Result<Self> {
        if config.name.trim().is_empty() || config.program.trim().is_empty() {
            bail!("LSP name and program are required");
        }
        if config.max_message_bytes == 0 {
            bail!("LSP max_message_bytes must be greater than zero");
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
            .with_context(|| format!("spawn language server {}", config.name))?;
        let stdin = child.stdin.take().context("LSP stdin unavailable")?;
        let stdout = child.stdout.take().context("LSP stdout unavailable")?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            request_timeout: Duration::from_secs(config.timeout_seconds.max(1)),
            max_message_bytes: config.max_message_bytes,
            notifications: VecDeque::new(),
            initialized: false,
        })
    }

    pub async fn initialize(
        &mut self,
        root_uri: &str,
        client_name: &str,
        initialization_options: Value,
    ) -> Result<Value> {
        if self.initialized {
            bail!("language server is already initialized");
        }

        let result = self
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "clientInfo": {
                        "name": client_name,
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "rootUri": root_uri,
                    "capabilities": {
                        "workspace": {
                            "workspaceFolders": true,
                            "configuration": true
                        },
                        "textDocument": {
                            "synchronization": {
                                "dynamicRegistration": true,
                                "willSave": false,
                                "didSave": true,
                                "willSaveWaitUntil": false
                            },
                            "completion": {
                                "dynamicRegistration": true,
                                "completionItem": {
                                    "snippetSupport": true,
                                    "documentationFormat": ["markdown", "plaintext"]
                                }
                            },
                            "hover": {
                                "dynamicRegistration": true,
                                "contentFormat": ["markdown", "plaintext"]
                            },
                            "definition": {"dynamicRegistration": true},
                            "references": {"dynamicRegistration": true},
                            "rename": {"dynamicRegistration": true, "prepareSupport": true},
                            "codeAction": {
                                "dynamicRegistration": true,
                                "isPreferredSupport": true,
                                "dataSupport": true
                            },
                            "publishDiagnostics": {
                                "relatedInformation": true,
                                "versionSupport": true,
                                "codeDescriptionSupport": true,
                                "dataSupport": true
                            },
                            "semanticTokens": {
                                "dynamicRegistration": true,
                                "requests": {"range": true, "full": {"delta": true}},
                                "tokenTypes": [],
                                "tokenModifiers": [],
                                "formats": ["relative"]
                            }
                        }
                    },
                    "initializationOptions": initialization_options
                }),
            )
            .await?;

        self.notify("initialized", json!({})).await?;
        self.initialized = true;
        Ok(result)
    }

    pub async fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        version: i64,
        text: &str,
    ) -> Result<()> {
        self.require_initialized()?;
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text
                }
            }),
        )
        .await
    }

    pub async fn did_change(
        &mut self,
        uri: &str,
        version: i64,
        text: &str,
    ) -> Result<()> {
        self.require_initialized()?;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}]
            }),
        )
        .await
    }

    pub async fn did_close(&mut self, uri: &str) -> Result<()> {
        self.require_initialized()?;
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        )
        .await
    }

    pub async fn completion(&mut self, uri: &str, line: u32, character: u32) -> Result<Value> {
        self.text_document_position_request("textDocument/completion", uri, line, character)
            .await
    }

    pub async fn hover(&mut self, uri: &str, line: u32, character: u32) -> Result<Value> {
        self.text_document_position_request("textDocument/hover", uri, line, character)
            .await
    }

    pub async fn definition(&mut self, uri: &str, line: u32, character: u32) -> Result<Value> {
        self.text_document_position_request("textDocument/definition", uri, line, character)
            .await
    }

    pub async fn references(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Value> {
        self.require_initialized()?;
        self.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": include_declaration}
            }),
        )
        .await
    }

    pub async fn rename(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Value> {
        self.require_initialized()?;
        if new_name.trim().is_empty() {
            bail!("rename target cannot be empty");
        }
        self.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
                "newName": new_name
            }),
        )
        .await
    }

    pub async fn code_actions(
        &mut self,
        uri: &str,
        range: Value,
        diagnostics: Value,
    ) -> Result<Value> {
        self.require_initialized()?;
        self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": range,
                "context": {"diagnostics": diagnostics}
            }),
        )
        .await
    }

    pub async fn semantic_tokens_full(&mut self, uri: &str) -> Result<Value> {
        self.require_initialized()?;
        self.request(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": uri}}),
        )
        .await
    }

    pub fn drain_notifications(&mut self) -> Vec<Value> {
        self.notifications.drain(..).collect()
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        if method.trim().is_empty() {
            bail!("LSP method cannot be empty");
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;

        timeout(self.request_timeout, async {
            loop {
                let value = read_message(&mut self.stdout, self.max_message_bytes).await?;
                if value.get("id").and_then(Value::as_u64) == Some(id) {
                    if let Some(error) = value.get("error") {
                        bail!("LSP request {method} failed: {error}");
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                if value.get("method").is_some() {
                    self.notifications.push_back(value);
                }
            }
        })
        .await
        .with_context(|| format!("LSP request {method} timed out"))?
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if self.initialized {
            let _ = self.request("shutdown", Value::Null).await;
            let _ = self.notify("exit", Value::Null).await;
        }
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        Ok(())
    }

    async fn text_document_position_request(
        &mut self,
        method: &str,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Value> {
        self.require_initialized()?;
        self.request(
            method,
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character}
            }),
        )
        .await
    }

    fn require_initialized(&self) -> Result<()> {
        if !self.initialized {
            bail!("language server must be initialized first");
        }
        Ok(())
    }

    async fn write_message(&mut self, value: &Value) -> Result<()> {
        let payload = serde_json::to_vec(value)?;
        if payload.len() > self.max_message_bytes {
            bail!("LSP outbound message exceeds configured limit");
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
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            bail!("language server closed stdout");
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
                    .context("invalid LSP Content-Length")?,
            );
        }
    }

    let length = content_length.context("LSP message missing Content-Length")?;
    if length > max_message_bytes {
        bail!("LSP message exceeds configured limit");
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload).context("invalid LSP JSON payload")?)
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid LSP environment variable name {key:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_environment_names() {
        assert!(validate_env_key("RUST_LOG").is_ok());
        assert!(validate_env_key("BAD=VALUE").is_err());
    }
}
