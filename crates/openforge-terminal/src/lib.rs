use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, Mutex, RwLock},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDescriptor {
    pub id: Uuid,
    pub program: String,
    pub cwd: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalEvent {
    pub terminal_id: Uuid,
    pub stream: String,
    pub data: String,
    pub timestamp: DateTime<Utc>,
}

struct TerminalSession {
    descriptor: TerminalDescriptor,
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    output: broadcast::Sender<TerminalEvent>,
}

#[derive(Clone, Default)]
pub struct TerminalManager {
    sessions: Arc<RwLock<HashMap<Uuid, Arc<TerminalSession>>>>,
}

impl TerminalManager {
    pub async fn spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: impl AsRef<Path>,
        environment: &BTreeMap<String, String>,
    ) -> Result<TerminalDescriptor> {
        if program.trim().is_empty() {
            bail!("terminal program cannot be empty");
        }
        let cwd = cwd
            .as_ref()
            .canonicalize()
            .context("terminal cwd does not exist")?;

        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env_clear();

        for key in ["PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in environment {
            validate_env_key(key)?;
            command.env(key, value);
        }
        command.env("TERM", "xterm-256color");

        let mut child = command
            .spawn()
            .with_context(|| format!("spawn terminal program {program}"))?;
        let stdin = child.stdin.take().context("terminal stdin unavailable")?;
        let stdout = child.stdout.take().context("terminal stdout unavailable")?;
        let stderr = child.stderr.take().context("terminal stderr unavailable")?;
        let (output, _) = broadcast::channel(2048);

        let descriptor = TerminalDescriptor {
            id: Uuid::now_v7(),
            program: program.to_string(),
            cwd: cwd.display().to_string(),
            created_at: Utc::now(),
        };
        let session = Arc::new(TerminalSession {
            descriptor: descriptor.clone(),
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            output,
        });

        spawn_reader(
            descriptor.id,
            "stdout",
            stdout,
            session.output.clone(),
        );
        spawn_reader(
            descriptor.id,
            "stderr",
            stderr,
            session.output.clone(),
        );

        self.sessions
            .write()
            .await
            .insert(descriptor.id, session);
        Ok(descriptor)
    }

    pub async fn write(&self, id: Uuid, data: &[u8]) -> Result<()> {
        let session = self.session(id).await?;
        let mut stdin = session.stdin.lock().await;
        stdin.write_all(data).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn subscribe(&self, id: Uuid) -> Result<broadcast::Receiver<TerminalEvent>> {
        Ok(self.session(id).await?.output.subscribe())
    }

    pub async fn close(&self, id: Uuid) -> Result<bool> {
        let session = self.sessions.write().await.remove(&id);
        let Some(session) = session else {
            return Ok(false);
        };
        let mut child = session.child.lock().await;
        if child.id().is_some() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(true)
    }

    pub async fn list(&self) -> Vec<TerminalDescriptor> {
        self.sessions
            .read()
            .await
            .values()
            .map(|session| session.descriptor.clone())
            .collect()
    }

    async fn session(&self, id: Uuid) -> Result<Arc<TerminalSession>> {
        self.sessions
            .read()
            .await
            .get(&id)
            .cloned()
            .with_context(|| format!("unknown terminal session {id}"))
    }
}

fn spawn_reader<R>(
    terminal_id: Uuid,
    stream: &'static str,
    mut reader: R,
    sender: broadcast::Sender<TerminalEvent>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(count) => {
                    let _ = sender.send(TerminalEvent {
                        terminal_id,
                        stream: stream.to_string(),
                        data: String::from_utf8_lossy(&buffer[..count]).into_owned(),
                        timestamp: Utc::now(),
                    });
                }
                Err(_) => break,
            }
        }
    });
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid terminal environment variable name {key:?}");
    }
    Ok(())
}
