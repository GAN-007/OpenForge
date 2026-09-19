use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDescriptor {
    pub id: Uuid,
    pub program: String,
    pub cwd: String,
    pub created_at: DateTime<Utc>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalEvent {
    pub terminal_id: Uuid,
    pub stream: String,
    pub data: String,
    pub timestamp: DateTime<Utc>,
}

struct TerminalSession {
    descriptor: StdMutex<TerminalDescriptor>,
    master: StdMutex<Box<dyn MasterPty + Send>>,
    writer: StdMutex<Box<dyn Write + Send>>,
    child: StdMutex<Box<dyn portable_pty::Child + Send + Sync>>,
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
        self.spawn_sized(program, args, cwd, environment, 30, 120).await
    }

    pub async fn spawn_sized(
        &self,
        program: &str,
        args: &[String],
        cwd: impl AsRef<Path>,
        environment: &BTreeMap<String, String>,
        rows: u16,
        cols: u16,
    ) -> Result<TerminalDescriptor> {
        if program.trim().is_empty() {
            bail!("terminal program cannot be empty");
        }
        if rows == 0 || cols == 0 {
            bail!("terminal rows and cols must be greater than zero");
        }
        let cwd = cwd
            .as_ref()
            .canonicalize()
            .context("terminal cwd does not exist")?;
        for key in environment.keys() {
            validate_env_key(key)?;
        }

        let program = program.to_string();
        let descriptor_program = program.clone();
        let arguments = args.to_vec();
        let environment = environment.clone();
        let cwd_for_spawn = cwd.clone();
        let id = Uuid::now_v7();
        let created_at = Utc::now();

        let (master, writer, child, reader) = tokio::task::spawn_blocking(move || {
            let pty_system = native_pty_system();
            let pair = pty_system.openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;

            let mut command = CommandBuilder::new(&program);
            command.args(arguments);
            command.cwd(cwd_for_spawn);
            command.env_clear();

            for key in ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TMPDIR"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");
            for (key, value) in environment {
                command.env(key, value);
            }

            let child = pair.slave.spawn_command(command)?;
            drop(pair.slave);
            let reader = pair.master.try_clone_reader()?;
            let writer = pair.master.take_writer()?;
            Ok::<_, anyhow::Error>((pair.master, writer, child, reader))
        })
        .await
        .context("PTY spawn task failed")??;

        let descriptor = TerminalDescriptor {
            id,
            program: descriptor_program,
            cwd: cwd.display().to_string(),
            created_at,
            rows,
            cols,
        };
        let (output, _) = broadcast::channel(4096);
        let session = Arc::new(TerminalSession {
            descriptor: StdMutex::new(descriptor.clone()),
            master: StdMutex::new(master),
            writer: StdMutex::new(writer),
            child: StdMutex::new(child),
            output,
        });
        spawn_reader(id, reader, session.output.clone());

        self.sessions.write().await.insert(id, session);
        Ok(descriptor)
    }

    pub async fn write(&self, id: Uuid, data: &[u8]) -> Result<()> {
        if data.len() > 1024 * 1024 {
            bail!("terminal input frame exceeds 1 MiB");
        }
        let session = self.session(id).await?;
        let bytes = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut writer = session
                .writer
                .lock()
                .map_err(|_| anyhow::anyhow!("terminal writer mutex poisoned"))?;
            writer.write_all(&bytes)?;
            writer.flush()?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("terminal write task failed")??;
        Ok(())
    }

    pub async fn resize(&self, id: Uuid, rows: u16, cols: u16) -> Result<()> {
        if rows == 0 || cols == 0 {
            bail!("terminal rows and cols must be greater than zero");
        }
        let session = self.session(id).await?;
        tokio::task::spawn_blocking(move || {
            session
                .master
                .lock()
                .map_err(|_| anyhow::anyhow!("terminal master mutex poisoned"))?
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
            let mut descriptor = session
                .descriptor
                .lock()
                .map_err(|_| anyhow::anyhow!("terminal descriptor mutex poisoned"))?;
            descriptor.rows = rows;
            descriptor.cols = cols;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("terminal resize task failed")??;
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
        tokio::task::spawn_blocking(move || {
            let mut child = session
                .child
                .lock()
                .map_err(|_| anyhow::anyhow!("terminal child mutex poisoned"))?;
            let _ = child.kill();
            let _ = child.wait();
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("terminal close task failed")??;
        Ok(true)
    }

    pub async fn list(&self) -> Vec<TerminalDescriptor> {
        let sessions = self.sessions.read().await;
        let mut descriptors = sessions
            .values()
            .filter_map(|session| session.descriptor.lock().ok().map(|value| value.clone()))
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        descriptors
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

fn spawn_reader(
    terminal_id: Uuid,
    mut reader: Box<dyn Read + Send>,
    sender: broadcast::Sender<TerminalEvent>,
) {
    std::thread::Builder::new()
        .name(format!("openforge-terminal-{terminal_id}"))
        .spawn(move || {
            let mut buffer = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let _ = sender.send(TerminalEvent {
                            terminal_id,
                            stream: "pty".to_string(),
                            data: String::from_utf8_lossy(&buffer[..count]).into_owned(),
                            timestamp: Utc::now(),
                        });
                    }
                    Err(_) => break,
                }
            }
        })
        .ok();
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
