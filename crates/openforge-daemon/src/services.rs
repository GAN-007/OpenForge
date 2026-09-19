use anyhow::{Context, Result, bail};
use openforge_browser::BrowserClient;
use openforge_collab::CollaborationStore;
use openforge_core::{DebugAdapterConfig, LanguageServerConfig, OpenForgeConfig};
use openforge_dap::{DapClient, DapProcessConfig};
use openforge_debugger::DebugSession;
use openforge_lsp::{LspClient, LspProcessConfig};
use openforge_memory::MemoryStore;
use openforge_plugins::PluginRuntimeManager;
use openforge_team::TeamStore;
use openforge_terminal::TerminalManager;
use openforge_workers::WorkerStore;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;

pub struct ServiceHub {
    lsp_configs: HashMap<String, LanguageServerConfig>,
    dap_configs: HashMap<String, DebugAdapterConfig>,
    lsp_sessions: RwLock<HashMap<Uuid, Arc<Mutex<LspClient>>>>,
    dap_sessions: RwLock<HashMap<Uuid, Arc<Mutex<DapClient>>>>,
    browser_config: openforge_core::BrowserWorkerConfig,
    browser: Mutex<Option<BrowserClient>>,
    pub terminals: TerminalManager,
    pub workers: WorkerStore,
    pub memory: MemoryStore,
    pub team: TeamStore,
    pub collaboration: CollaborationStore,
    pub plugins: PluginRuntimeManager,
    pub debuggers: RwLock<HashMap<Uuid, DebugSession>>,
}

impl ServiceHub {
    pub fn new(config: &OpenForgeConfig) -> Result<Self> {
        let mut lsp_configs = HashMap::new();
        for server in &config.lsp_servers {
            if server.name.trim().is_empty() {
                bail!("language server name cannot be empty");
            }
            if lsp_configs
                .insert(server.name.clone(), server.clone())
                .is_some()
            {
                bail!("duplicate language server config {}", server.name);
            }
        }

        let mut dap_configs = HashMap::new();
        for adapter in &config.dap_adapters {
            if adapter.name.trim().is_empty() {
                bail!("debug adapter name cannot be empty");
            }
            if dap_configs
                .insert(adapter.name.clone(), adapter.clone())
                .is_some()
            {
                bail!("duplicate debug adapter config {}", adapter.name);
            }
        }

        Ok(Self {
            lsp_configs,
            dap_configs,
            lsp_sessions: RwLock::new(HashMap::new()),
            dap_sessions: RwLock::new(HashMap::new()),
            browser_config: config.browser.clone(),
            browser: Mutex::new(None),
            terminals: TerminalManager::default(),
            workers: WorkerStore::open(&config.worker_db)?,
            memory: MemoryStore::open(&config.memory_db)?,
            team: TeamStore::open(&config.team_db)?,
            collaboration: CollaborationStore::open(&config.collab_db)?,
            plugins: PluginRuntimeManager::default(),
            debuggers: RwLock::new(HashMap::new()),
        })
    }

    pub fn lsp_configs(&self) -> Vec<&LanguageServerConfig> {
        let mut values = self.lsp_configs.values().collect::<Vec<_>>();
        values.sort_by(|left, right| left.name.cmp(&right.name));
        values
    }

    pub fn dap_configs(&self) -> Vec<&DebugAdapterConfig> {
        let mut values = self.dap_configs.values().collect::<Vec<_>>();
        values.sort_by(|left, right| left.name.cmp(&right.name));
        values
    }

    pub async fn start_lsp(&self, name: &str, repo: &Path) -> Result<(Uuid, Value)> {
        let configured = self
            .lsp_configs
            .get(name)
            .with_context(|| format!("unknown language server {name}"))?
            .clone();
        let repo = repo
            .canonicalize()
            .context("LSP repository root unavailable")?;
        let cwd = resolve_cwd(&repo, configured.cwd.as_deref())?;
        let mut client = LspClient::spawn(&LspProcessConfig {
            name: configured.name.clone(),
            program: configured.program,
            args: configured.args,
            cwd: Some(cwd),
            environment: configured.environment,
            timeout_seconds: configured.timeout_seconds,
            max_message_bytes: configured.max_message_bytes,
            initialization_options: configured.initialization_options.clone(),
        })
        .await?;

        let root_uri = Url::from_directory_path(&repo)
            .map_err(|_| anyhow::anyhow!("repository root cannot be represented as file URI"))?;
        let capabilities = client
            .initialize(
                root_uri.as_str(),
                "openforge",
                configured.initialization_options,
            )
            .await?;

        let id = Uuid::now_v7();
        self.lsp_sessions
            .write()
            .await
            .insert(id, Arc::new(Mutex::new(client)));
        Ok((id, capabilities))
    }

    pub async fn lsp_request(
        &self,
        session_id: Uuid,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let client = self.lsp_session(session_id).await?;
        client.lock().await.request(method, params).await
    }

    pub async fn lsp_notify(&self, session_id: Uuid, method: &str, params: Value) -> Result<()> {
        let client = self.lsp_session(session_id).await?;
        client.lock().await.notify(method, params).await
    }

    pub async fn lsp_notifications(&self, session_id: Uuid) -> Result<Vec<Value>> {
        let client = self.lsp_session(session_id).await?;
        Ok(client.lock().await.drain_notifications())
    }

    pub async fn stop_lsp(&self, session_id: Uuid) -> Result<bool> {
        let client = self.lsp_sessions.write().await.remove(&session_id);
        if let Some(client) = client {
            let client = Arc::try_unwrap(client)
                .map_err(|_| anyhow::anyhow!("LSP session is still in use"))?
                .into_inner();
            client.shutdown().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn start_dap(&self, name: &str, repo: &Path) -> Result<(Uuid, Value)> {
        let configured = self
            .dap_configs
            .get(name)
            .with_context(|| format!("unknown debug adapter {name}"))?
            .clone();
        let repo = repo
            .canonicalize()
            .context("DAP repository root unavailable")?;
        let cwd = resolve_cwd(&repo, configured.cwd.as_deref())?;
        let mut client = DapClient::spawn(&DapProcessConfig {
            name: configured.name,
            program: configured.program,
            args: configured.args,
            cwd: Some(cwd),
            environment: configured.environment,
            timeout_seconds: configured.timeout_seconds,
            max_message_bytes: configured.max_message_bytes,
        })
        .await?;
        let capabilities = client.initialize(&configured.adapter_id).await?;
        let id = Uuid::now_v7();
        self.dap_sessions
            .write()
            .await
            .insert(id, Arc::new(Mutex::new(client)));
        Ok((id, capabilities))
    }

    pub async fn dap_request(
        &self,
        session_id: Uuid,
        command: &str,
        arguments: Value,
    ) -> Result<Value> {
        let client = self.dap_session(session_id).await?;
        client.lock().await.request(command, arguments).await
    }

    pub async fn dap_events(&self, session_id: Uuid) -> Result<Vec<openforge_dap::DapEvent>> {
        let client = self.dap_session(session_id).await?;
        Ok(client.lock().await.drain_events())
    }

    pub async fn stop_dap(&self, session_id: Uuid, terminate_debuggee: bool) -> Result<bool> {
        let client = self.dap_sessions.write().await.remove(&session_id);
        if let Some(client) = client {
            let client = Arc::try_unwrap(client)
                .map_err(|_| anyhow::anyhow!("DAP session is still in use"))?
                .into_inner();
            client.disconnect(terminate_debuggee).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn browser_request(&self, method: &str, params: Value) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        if guard.is_none() {
            *guard = Some(
                BrowserClient::spawn(
                    &self.browser_config.program,
                    &self.browser_config.args,
                    Duration::from_secs(self.browser_config.timeout_seconds.max(1)),
                )
                .await
                .context("start persistent browser QA worker")?,
            );
        }
        guard
            .as_mut()
            .expect("browser worker initialized")
            .raw_request(method, params)
            .await
    }

    pub async fn close_browser(&self) -> Result<bool> {
        let client = self.browser.lock().await.take();
        if let Some(client) = client {
            client.close().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn create_debug_session(
        &self,
        run_id: Uuid,
        task_id: Option<Uuid>,
        issue: String,
    ) -> Result<DebugSession> {
        let session = DebugSession::new(run_id, task_id, issue)?;
        self.debuggers
            .write()
            .await
            .insert(session.id, session.clone());
        Ok(session)
    }

    pub async fn get_debug_session(&self, id: Uuid) -> Result<DebugSession> {
        self.debuggers
            .read()
            .await
            .get(&id)
            .cloned()
            .with_context(|| format!("unknown debug session {id}"))
    }

    pub async fn update_debug_session(&self, session: DebugSession) -> Result<DebugSession> {
        if !self.debuggers.read().await.contains_key(&session.id) {
            bail!("unknown debug session {}", session.id);
        }
        self.debuggers
            .write()
            .await
            .insert(session.id, session.clone());
        Ok(session)
    }

    async fn lsp_session(&self, id: Uuid) -> Result<Arc<Mutex<LspClient>>> {
        self.lsp_sessions
            .read()
            .await
            .get(&id)
            .cloned()
            .with_context(|| format!("unknown LSP session {id}"))
    }

    async fn dap_session(&self, id: Uuid) -> Result<Arc<Mutex<DapClient>>> {
        self.dap_sessions
            .read()
            .await
            .get(&id)
            .cloned()
            .with_context(|| format!("unknown DAP session {id}"))
    }
}

fn resolve_cwd(repo: &Path, configured: Option<&str>) -> Result<PathBuf> {
    let candidate = match configured {
        Some(value) if !value.trim().is_empty() => repo.join(value),
        _ => repo.to_path_buf(),
    };
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("configured cwd {} unavailable", candidate.display()))?;
    if !canonical.starts_with(repo) {
        bail!("configured service cwd escapes repository");
    }
    Ok(canonical)
}
