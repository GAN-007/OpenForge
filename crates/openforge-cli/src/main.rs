use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use openforge_core::{Engine, OpenForgeConfig, RunnerBackend};
use openforge_policy::AgentPolicy;
use openforge_protocol::AutonomyLevel;
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "openforge",
    version,
    about = "Open-source AI engineering operating system"
)]
struct Cli {
    #[arg(long, default_value = "openforge.yaml", global = true)]
    config: PathBuf,

    #[arg(
        long,
        env = "OPENFORGE_DAEMON_URL",
        default_value = "http://127.0.0.1:8765",
        global = true
    )]
    daemon_url: String,

    #[arg(long, env = "OPENFORGE_API_TOKEN", global = true, hide_env_values = true)]
    api_token: Option<String>,

    #[arg(
        long,
        global = true,
        help = "Run the legacy in-process engine instead of the shared daemon"
    )]
    standalone: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
    Chat {
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        budget: f64,
        #[arg(long, value_enum, default_value_t = Mode::Execute)]
        mode: Mode,
        #[arg(long, default_value = "config/policies/development.yaml")]
        policy: PathBuf,
        #[arg(long, value_enum)]
        runner: Option<RunnerChoice>,
        #[arg(
            long,
            help = "Initialize Git metadata without prompting when the selected folder is not already a repository"
        )]
        yes_init_git: bool,
    },
    Plan {
        objective: String,
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        budget: f64,
        #[arg(long, value_enum, default_value_t = Mode::Suggest)]
        mode: Mode,
    },
    Execute {
        run_id: Uuid,
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value = "config/policies/development.yaml")]
        policy: PathBuf,
        #[arg(long)]
        docker: bool,
        #[arg(long, value_enum)]
        runner: Option<RunnerChoice>,
    },
    Run {
        objective: String,
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value_t = 10.0)]
        budget: f64,
        #[arg(long, value_enum, default_value_t = Mode::Execute)]
        mode: Mode,
        #[arg(long, default_value = "config/policies/development.yaml")]
        policy: PathBuf,
        #[arg(long)]
        docker: bool,
        #[arg(long, value_enum)]
        runner: Option<RunnerChoice>,
    },
    Status {
        run_id: Uuid,
    },
    Events {
        run_id: Uuid,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
    Providers,
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
}

#[derive(Subcommand)]
enum ModelCommand {
    Show,
    Configure {
        #[arg(long, default_value = "shared")]
        provider_name: String,
        #[arg(long, value_enum, default_value_t = ProviderKind::OpenaiCompatible)]
        kind: ProviderKind,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        model: String,
        #[arg(long)]
        family: Option<String>,
        #[arg(long, default_value_t = 32_768)]
        context_tokens: u32,
        #[arg(long, default_value_t = true)]
        tools: bool,
        #[arg(long, default_value_t = false)]
        vision: bool,
        #[arg(long, default_value_t = true)]
        structured_output: bool,
        #[arg(long, default_value_t = 0.0)]
        input_usd_per_million: f64,
        #[arg(long, default_value_t = 0.0)]
        output_usd_per_million: f64,
        #[arg(long)]
        region: Option<String>,
        #[arg(
            long,
            help = "API key. Prefer omitting this flag so OpenForge prompts without shell-history exposure."
        )]
        api_key: Option<String>,
        #[arg(long, help = "Keep the daemon's currently stored API key when reconfiguring")]
        retain_existing_api_key: bool,
    },
    Clear,
}

#[derive(Subcommand)]
enum MemoryCommand {
    Put {
        #[arg(long)]
        scope: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        value_json: String,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        repository_id: Option<String>,
    },
    Search {
        query: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    Forget {
        key: String,
        #[arg(long)]
        scope: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RunnerChoice {
    Local,
    Docker,
    Kubernetes,
}

impl RunnerChoice {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Kubernetes => "kubernetes",
        }
    }
}

impl From<RunnerChoice> for RunnerBackend {
    fn from(value: RunnerChoice) -> Self {
        match value {
            RunnerChoice::Local => Self::Local,
            RunnerChoice::Docker => Self::Docker,
            RunnerChoice::Kubernetes => Self::Kubernetes,
        }
    }
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum Mode {
    Observe,
    Suggest,
    Edit,
    #[default]
    Execute,
    Autonomous,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Suggest => "suggest",
            Self::Edit => "edit",
            Self::Execute => "execute",
            Self::Autonomous => "autonomous",
        }
    }

    fn executes(self) -> bool {
        matches!(self, Self::Edit | Self::Execute | Self::Autonomous)
    }
}

impl From<Mode> for AutonomyLevel {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Observe => Self::Observe,
            Mode::Suggest => Self::Suggest,
            Mode::Edit => Self::Edit,
            Mode::Execute => Self::Execute,
            Mode::Autonomous => Self::Autonomous,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ProviderKind {
    OpenaiCompatible,
    Anthropic,
    Gemini,
    BedrockAwsCli,
}

impl ProviderKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai-compatible",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::BedrockAwsCli => "bedrock-aws-cli",
        }
    }
}

struct DaemonClient {
    base_url: String,
    api_token: Option<String>,
    client: reqwest::Client,
    next_id: AtomicU64,
}

impl DaemonClient {
    fn new(base_url: String, api_token: Option<String>) -> Result<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()?,
            next_id: AtomicU64::new(1),
        })
    }

    async fn health(&self) -> Result<Value> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .context("connect to OpenForge daemon")?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("daemon health check returned {status}: {body}");
        }
        serde_json::from_str(&body).context("parse daemon health response")
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = self.client.post(format!("{}/v1/rpc", self.base_url)).json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        if let Some(token) = &self.api_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("call daemon RPC {method}"))?;
        let status = response.status();
        let body = response.text().await?;
        let payload: Value = serde_json::from_str(&body)
            .with_context(|| format!("daemon returned invalid JSON for {method}"))?;

        if !status.is_success() || payload.get("error").is_some_and(|error| !error.is_null()) {
            let message = payload
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or(&body);
            bail!("{method}: {message}");
        }

        payload
            .get("result")
            .cloned()
            .context("daemon RPC response missing result")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli {
        config,
        daemon_url,
        api_token,
        standalone,
        command,
    } = Cli::parse();

    if let Command::Init { repo } = command {
        return init(repo).await;
    }

    if standalone {
        return run_standalone(config, command).await;
    }

    let daemon = DaemonClient::new(daemon_url, api_token)?;
    daemon
        .health()
        .await
        .context("OpenForge daemon is not running; start it with setup.sh or openforge-daemon")?;
    run_daemon(&daemon, command).await
}

async fn run_daemon(daemon: &DaemonClient, command: Command) -> Result<()> {
    match command {
        Command::Chat {
            repo,
            budget,
            mode,
            policy,
            runner,
            yes_init_git,
        } => chat(daemon, repo, budget, mode, policy, runner, yes_init_git).await?,
        Command::Plan {
            objective,
            repo,
            budget,
            mode,
        } => {
            let repo = canonical_repo(&repo)?;
            let run = daemon
                .rpc(
                    "run/create",
                    json!({
                        "repo": repo,
                        "objective": objective,
                        "autonomy": mode.as_str(),
                        "budget_usd": budget
                    }),
                )
                .await?;
            let run_id = result_uuid(&run, "id")?;
            let tasks = daemon
                .rpc("run/plan", json!({"repo": repo, "run_id": run_id}))
                .await?;
            print_json(&json!({"run": run, "tasks": tasks}))?;
        }
        Command::Execute {
            run_id,
            repo,
            policy,
            docker,
            runner,
        } => {
            if docker && runner.is_some() {
                bail!("--docker and --runner cannot be used together");
            }
            let repo = canonical_repo(&repo)?;
            let mut params = json!({
                "repo": repo,
                "run_id": run_id,
                "policy_path": policy,
                "docker": docker
            });
            if let Some(runner) = runner {
                params["runner"] = Value::String(runner.as_str().into());
            }
            print_json(&daemon.rpc("run/execute", params).await?)?;
        }
        Command::Run {
            objective,
            repo,
            budget,
            mode,
            policy,
            docker,
            runner,
        } => {
            if docker && runner.is_some() {
                bail!("--docker and --runner cannot be used together");
            }
            let repo = canonical_repo(&repo)?;
            let run = daemon
                .rpc(
                    "run/create",
                    json!({
                        "repo": repo,
                        "objective": objective,
                        "autonomy": mode.as_str(),
                        "budget_usd": budget
                    }),
                )
                .await?;
            let run_id = result_uuid(&run, "id")?;
            let tasks = daemon
                .rpc("run/plan", json!({"repo": repo, "run_id": run_id}))
                .await?;
            if mode.executes() {
                let mut params = json!({
                    "repo": repo,
                    "run_id": run_id,
                    "policy_path": policy,
                    "docker": docker
                });
                let effective_runner = runner.or_else(|| {
                    matches!(mode, Mode::Autonomous).then_some(RunnerChoice::Docker)
                });
                if let Some(runner) = effective_runner {
                    params["runner"] = Value::String(runner.as_str().into());
                    params["docker"] = Value::Bool(false);
                }
                let execution = daemon.rpc("run/execute", params).await?;
                print_json(&json!({"run": run, "tasks": tasks, "execution": execution}))?;
            } else {
                print_json(&json!({"run": run, "tasks": tasks}))?;
            }
        }
        Command::Status { run_id } => {
            let run = daemon.rpc("run/get", json!({"run_id": run_id})).await?;
            let tasks = daemon.rpc("task/list", json!({"run_id": run_id})).await?;
            let budget = daemon.rpc("budget/get", json!({"run_id": run_id})).await?;
            print_json(&json!({"run": run, "tasks": tasks, "budget": budget}))?;
        }
        Command::Events { run_id, after } => {
            print_json(
                &daemon
                    .rpc(
                        "event/list",
                        json!({"run_id": run_id, "after_sequence": after, "limit": 1000}),
                    )
                    .await?,
            )?;
        }
        Command::Providers => print_json(&daemon.rpc("model/providers", json!({})).await?)?,
        Command::Model { command } => model_command(daemon, command).await?,
        Command::Memory { command } => memory_daemon(daemon, command).await?,
        Command::Init { .. } => unreachable!(),
    }
    Ok(())
}

async fn chat(
    daemon: &DaemonClient,
    repo: PathBuf,
    budget: f64,
    mode: Mode,
    policy: PathBuf,
    runner: Option<RunnerChoice>,
    yes_init_git: bool,
) -> Result<()> {
    if !budget.is_finite() || budget <= 0.0 {
        bail!("budget must be positive");
    }
    let repo = prepare_workspace(repo, yes_init_git)?;
    let repo_string = repo.to_string_lossy().into_owned();
    let provider = daemon.rpc("model/providers", json!({})).await?;

    println!("OpenForge interactive terminal");
    println!("workspace: {}", repo.display());
    println!(
        "model runtime: {}",
        serde_json::to_string(provider.get("models").unwrap_or(&Value::Null))?
    );
    println!(
        "mode: {} · per-turn budget: ${:.2} · type /help for commands",
        mode.as_str(),
        budget
    );

    let mut transcript: Vec<(String, String)> = Vec::new();
    loop {
        print!("openforge> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            println!();
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/exit" | "/quit" => break,
            "/help" => {
                println!("/help        show commands");
                println!("/provider    show the daemon's shared provider/model");
                println!("/context     show recent conversation context");
                println!("/clear       clear local conversation context");
                println!("/exit        leave the session");
                continue;
            }
            "/provider" => {
                print_json(&daemon.rpc("model/settings/get", json!({})).await?)?;
                continue;
            }
            "/context" => {
                for (role, text) in &transcript {
                    println!("{role}: {text}");
                }
                continue;
            }
            "/clear" => {
                transcript.clear();
                println!("conversation context cleared");
                continue;
            }
            _ => {}
        }

        let objective = conversation_objective(&transcript, input);
        let run = daemon
            .rpc(
                "run/create",
                json!({
                    "repo": repo_string,
                    "objective": objective,
                    "autonomy": mode.as_str(),
                    "budget_usd": budget
                }),
            )
            .await?;
        let run_id = result_uuid(&run, "id")?;
        let tasks = daemon
            .rpc(
                "run/plan",
                json!({"repo": repo_string, "run_id": run_id}),
            )
            .await?;
        let task_count = tasks.as_array().map_or(0, Vec::len);
        println!("planned {task_count} task(s) · run {run_id}");

        let assistant_summary = if mode.executes() {
            let effective_runner = runner.or_else(|| {
                matches!(mode, Mode::Autonomous).then_some(RunnerChoice::Docker)
            });
            let mut params = json!({
                "repo": repo_string,
                "run_id": run_id,
                "policy_path": policy,
                "docker": false
            });
            if let Some(selected) = effective_runner {
                params["runner"] = Value::String(selected.as_str().into());
            }
            let execution = daemon.rpc("run/execute", params).await?;
            let branch = execution
                .get("integration_branch")
                .and_then(Value::as_str)
                .unwrap_or("no integration branch returned");
            let status = daemon.rpc("run/get", json!({"run_id": run_id})).await?;
            println!("completed · {branch}");
            format!(
                "run {run_id} completed on {branch}; status={}",
                status
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )
        } else {
            println!("plan ready; mode {} does not execute changes", mode.as_str());
            format!("run {run_id} planned {task_count} task(s) without execution")
        };

        transcript.push(("user".into(), input.to_string()));
        transcript.push(("openforge".into(), assistant_summary));
        if transcript.len() > 24 {
            transcript.drain(0..transcript.len() - 24);
        }
    }
    Ok(())
}

async fn model_command(daemon: &DaemonClient, command: ModelCommand) -> Result<()> {
    match command {
        ModelCommand::Show => print_json(&daemon.rpc("model/settings/get", json!({})).await?)?,
        ModelCommand::Configure {
            provider_name,
            kind,
            base_url,
            model,
            family,
            context_tokens,
            tools,
            vision,
            structured_output,
            input_usd_per_million,
            output_usd_per_million,
            region,
            api_key,
            retain_existing_api_key,
        } => {
            let needs_endpoint = matches!(kind, ProviderKind::OpenaiCompatible);
            let base_url = match (needs_endpoint, base_url) {
                (true, None) => {
                    print!("Base URL (gateway /v1 URL): ");
                    io::stdout().flush()?;
                    let mut value = String::new();
                    io::stdin().read_line(&mut value)?;
                    Some(value.trim().to_string())
                }
                (_, value) => value,
            };

            let api_key = if api_key.is_some()
                || retain_existing_api_key
                || matches!(kind, ProviderKind::BedrockAwsCli)
            {
                api_key
            } else {
                let value = read_secret("API key (Enter to leave empty): ")?;
                (!value.is_empty()).then_some(value)
            };

            print_json(
                &daemon
                    .rpc(
                        "model/settings/set",
                        json!({
                            "provider_name": provider_name,
                            "kind": kind.as_str(),
                            "base_url": base_url.unwrap_or_default(),
                            "model": model,
                            "family": family,
                            "context_tokens": context_tokens,
                            "tools": tools,
                            "vision": vision,
                            "structured_output": structured_output,
                            "input_usd_per_million": input_usd_per_million,
                            "output_usd_per_million": output_usd_per_million,
                            "region": region,
                            "api_key": api_key,
                            "retain_existing_api_key": retain_existing_api_key
                        }),
                    )
                    .await?,
            )?;
        }
        ModelCommand::Clear => {
            print_json(&daemon.rpc("model/settings/clear", json!({})).await?)?;
        }
    }
    Ok(())
}

async fn memory_daemon(daemon: &DaemonClient, command: MemoryCommand) -> Result<()> {
    match command {
        MemoryCommand::Put {
            scope,
            key,
            value_json,
            project_id,
            repository_id,
        } => {
            let value: Value =
                serde_json::from_str(&value_json).context("value_json must be valid JSON")?;
            print_json(
                &daemon
                    .rpc(
                        "memory/put",
                        json!({
                            "scope": scope,
                            "key": key,
                            "value": value,
                            "project_id": project_id,
                            "repository_id": repository_id
                        }),
                    )
                    .await?,
            )?;
        }
        MemoryCommand::Search { query, scope, limit } => {
            print_json(
                &daemon
                    .rpc(
                        "memory/search",
                        json!({"query": query, "scope": scope, "limit": limit}),
                    )
                    .await?,
            )?;
        }
        MemoryCommand::Forget { key, scope } => {
            print_json(
                &daemon
                    .rpc("memory/delete", json!({"key": key, "scope": scope}))
                    .await?,
            )?;
        }
    }
    Ok(())
}

async fn run_standalone(config_path: PathBuf, command: Command) -> Result<()> {
    if matches!(command, Command::Chat { .. } | Command::Model { .. }) {
        bail!("chat and model settings require the shared OpenForge daemon");
    }

    let config = OpenForgeConfig::load(&config_path)
        .with_context(|| format!("load {}", config_path.display()))?;
    let engine = Engine::new(config)?;

    match command {
        Command::Plan { objective, repo, budget, mode } => {
            let run = engine.create_run(&repo, objective, mode.into(), budget).await?;
            let tasks = engine.plan_run(&repo, &run).await?;
            print_json(&json!({"run": run, "tasks": tasks}))?;
        }
        Command::Execute { run_id, repo, policy, docker, runner } => {
            let policy = AgentPolicy::from_yaml(policy)?;
            if docker && runner.is_some() {
                bail!("--docker and --runner cannot be used together");
            }
            let integration_branch = if let Some(runner) = runner {
                engine.execute_run_with_backend(&repo, run_id, policy, runner.into()).await?
            } else {
                engine.execute_run(&repo, run_id, policy, docker).await?
            };
            print_json(&json!({
                "run": engine.store.get_run(run_id)?,
                "integration_branch": integration_branch,
                "cost_usd": engine.store.run_cost(run_id)?
            }))?;
        }
        Command::Run { objective, repo, budget, mode, policy, docker, runner } => {
            let run = engine.create_run(&repo, objective, mode.into(), budget).await?;
            let tasks = engine.plan_run(&repo, &run).await?;
            eprintln!("planned {} tasks for run {}", tasks.len(), run.id);
            let policy = AgentPolicy::from_yaml(policy)?;
            if docker && runner.is_some() {
                bail!("--docker and --runner cannot be used together");
            }
            let integration_branch = if let Some(runner) = runner {
                engine.execute_run_with_backend(&repo, run.id, policy, runner.into()).await?
            } else {
                engine.execute_run(&repo, run.id, policy, docker).await?
            };
            print_json(&json!({
                "run_id": run.id,
                "status": "completed",
                "integration_branch": integration_branch,
                "cost_usd": engine.store.run_cost(run.id)?
            }))?;
        }
        Command::Status { run_id } => print_json(&json!({
            "run": engine.store.get_run(run_id)?,
            "tasks": engine.store.list_tasks(run_id)?,
            "cost_usd": engine.store.run_cost(run_id)?
        }))?,
        Command::Events { run_id, after } => {
            print_json(&serde_json::to_value(engine.store.list_events(run_id, after, 1000)?)?)?;
        }
        Command::Providers => print_json(&json!({
            "providers": engine.providers(),
            "models": engine.provider_catalog()
        }))?,
        Command::Memory { command } => match command {
            MemoryCommand::Put { scope, key, value_json, project_id, repository_id } => {
                let value: Value =
                    serde_json::from_str(&value_json).context("value_json must be valid JSON")?;
                engine.store.memory_put(
                    &scope,
                    project_id,
                    repository_id.as_deref(),
                    &key,
                    &value,
                )?;
                print_json(&json!({"stored": true, "scope": scope, "key": key}))?;
            }
            MemoryCommand::Search { query, scope, limit } => {
                print_json(&serde_json::to_value(engine.store.memory_search(
                    scope.as_deref(),
                    &query,
                    limit,
                )?)?)?;
            }
            MemoryCommand::Forget { key, scope } => {
                let deleted = engine.store.memory_delete(scope.as_deref(), &key)?;
                print_json(&json!({"deleted": deleted}))?;
            }
        },
        Command::Init { .. } | Command::Chat { .. } | Command::Model { .. } => unreachable!(),
    }
    Ok(())
}

fn canonical_repo(repo: &Path) -> Result<String> {
    Ok(repo
        .canonicalize()
        .with_context(|| format!("repository path {} does not exist", repo.display()))?
        .to_string_lossy()
        .into_owned())
}

fn prepare_workspace(repo: PathBuf, yes_init_git: bool) -> Result<PathBuf> {
    let repo = repo
        .canonicalize()
        .with_context(|| format!("workspace {} does not exist", repo.display()))?;
    if git_is_repository(&repo) {
        return Ok(repo);
    }

    let initialize = yes_init_git || confirm(
        "This folder is not a Git repository. OpenForge uses Git worktrees for safe edits. Initialize local Git metadata and create a baseline commit? [y/N] ",
    )?;
    if !initialize {
        bail!("OpenForge execution requires a Git workspace");
    }

    run_process(ProcessCommand::new("git").arg("-C").arg(&repo).arg("init"), "git init")?;
    run_process(
        ProcessCommand::new("git").arg("-C").arg(&repo).args(["add", "-A"]),
        "git add",
    )?;

    let staged = ProcessCommand::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .context("inspect Git baseline")?;
    if !staged.success() {
        run_process(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&repo)
                .args([
                    "-c",
                    "user.name=OpenForge",
                    "-c",
                    "user.email=openforge@localhost",
                    "commit",
                    "-m",
                    "chore: establish OpenForge workspace baseline",
                ]),
            "create Git baseline",
        )?;
    } else {
        let has_head = ProcessCommand::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "--verify", "HEAD"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !has_head {
            let marker = repo.join(".openforge-baseline");
            std::fs::write(&marker, b"OpenForge baseline\n")?;
            run_process(
                ProcessCommand::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["add", ".openforge-baseline"]),
                "stage baseline marker",
            )?;
            run_process(
                ProcessCommand::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args([
                        "-c",
                        "user.name=OpenForge",
                        "-c",
                        "user.email=openforge@localhost",
                        "commit",
                        "-m",
                        "chore: establish OpenForge workspace baseline",
                    ]),
                "create empty-folder Git baseline",
            )?;
            let _ = std::fs::remove_file(marker);
        }
    }
    Ok(repo)
}

fn git_is_repository(repo: &Path) -> bool {
    ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|output| output.status.success() && output.stdout.starts_with(b"true"))
        .unwrap_or(false)
}

fn run_process(command: &mut ProcessCommand, label: &str) -> Result<()> {
    let status = command.status().with_context(|| format!("run {label}"))?;
    if !status.success() {
        bail!("{label} failed with {status}");
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn conversation_objective(history: &[(String, String)], current: &str) -> String {
    if history.is_empty() {
        return current.to_string();
    }
    let mut context = String::from(
        "You are continuing an interactive OpenForge terminal session. Preserve the user's prior intent, inspect the real workspace, and make only changes required by the current instruction.\n\nRECENT SESSION CONTEXT\n",
    );
    for (role, text) in history.iter().rev().take(12).rev() {
        context.push_str(role);
        context.push_str(": ");
        context.push_str(text);
        context.push('\n');
    }
    context.push_str("\nCURRENT INSTRUCTION\n");
    context.push_str(current);
    if context.len() > 24_000 {
        let mut start = context.len() - 24_000;
        while start < context.len() && !context.is_char_boundary(start) {
            start += 1;
        }
        context = context[start..].to_string();
    }
    context
}

fn result_uuid(value: &Value, field: &str) -> Result<Uuid> {
    let raw = value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("daemon result missing {field}"))?;
    Uuid::parse_str(raw).with_context(|| format!("daemon returned invalid {field}"))
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn read_secret(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;

    #[cfg(unix)]
    let echo_disabled = ProcessCommand::new("stty")
        .arg("-echo")
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    #[cfg(not(unix))]
    let echo_disabled = false;

    let mut value = String::new();
    let read = io::stdin().read_line(&mut value);

    #[cfg(unix)]
    if echo_disabled {
        let _ = ProcessCommand::new("stty").arg("echo").status();
        println!();
    }

    read?;
    Ok(value.trim().to_string())
}

async fn init(repo: PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(repo.join(".openforge")).await?;
    tokio::fs::create_dir_all(repo.join("config/policies")).await?;

    if !repo.join("openforge.yaml").exists() {
        tokio::fs::write(repo.join("openforge.yaml"), DEFAULT_CONFIG).await?;
    }
    if !repo.join("config/policies/development.yaml").exists() {
        tokio::fs::write(repo.join("config/policies/development.yaml"), DEFAULT_POLICY).await?;
    }

    println!("initialized OpenForge configuration in {}", repo.display());
    Ok(())
}

const DEFAULT_CONFIG: &str = r#"state_db: .openforge/state.db
artifact_dir: .openforge/artifacts
worktree_dir: .openforge/worktrees
plugin_dir: plugins
max_parallel_agents: 4

runner:
  backend: local
  image: ghcr.io/gan-007/openforge-runner:latest

providers:
  - name: local
    kind: openai-compatible
    base_url: http://127.0.0.1:11434/v1
    models:
      - id: qwen3-coder
        family: qwen
        context_tokens: 32768
        tools: false
        vision: false
        structured_output: true
        input_usd_per_million: 0
        output_usd_per_million: 0
        latency_score: 0.3
        quality_score: 0.75
        privacy_score: 1.0
        max_data_classification: RESTRICTED

mcp_servers: []

browser:
  program: node
  args:
    - packages/browser-worker/dist/index.js
  timeout_seconds: 60

environment: {}
"#;

const DEFAULT_POLICY: &str = r#"autonomy: execute

filesystem:
  read:
    default: allow
    deny:
      - "**/.env"
      - "**/.env.*"
      - "**/.git/**"
      - "~/.ssh/**"
      - "~/.aws/**"
      - "~/.config/gcloud/**"
      - "~/.azure/**"
  write:
    default: ask
    allow:
      - "src/**"
      - "tests/**"
      - "crates/**"
      - "packages/**"
      - "python/**"
      - "jetbrains/**"
      - "docs/**"
      - "schemas/**"
      - "plugins/**"
      - "migrations/**"
      - "*.md"
      - "*.toml"
      - "*.json"
      - "*.yaml"
      - "*.yml"
    deny:
      - "**/.env"
      - "**/.env.*"
      - "**/.git/**"

process:
  default: ask
  allow:
    - "git status**"
    - "git diff**"
    - "git log**"
    - "cargo check**"
    - "cargo test**"
    - "cargo fmt**"
    - "cargo clippy**"
    - "pnpm test**"
    - "pnpm typecheck**"
    - "pnpm build**"
    - "npm test**"
    - "pytest**"
    - "ruff check**"
    - "mypy**"
  deny:
    - "sudo **"
    - "su **"
    - "ssh **"
    - "scp **"
    - "git push**"
    - "git remote set-url**"
    - "terraform apply**"
    - "tofu apply**"
    - "kubectl delete**"
    - "kubectl apply**"
    - "helm upgrade**"
    - "rm -rf /**"

network:
  default: ask
  allow:
    - "https://github.com/**"
    - "https://docs.github.com/**"
    - "https://crates.io/**"
    - "https://registry.npmjs.org/**"
    - "https://pypi.org/**"
    - "http://127.0.0.1/**"
    - "http://localhost/**"

mcp:
  default: ask
acp:
  default: ask

database_read:
  default: ask
  allow:
    - "local/**"
    - "dev/**"
database_write:
  default: ask
  deny:
    - "prod/**"
    - "production/**"

secrets:
  default: deny
  allow:
    - "secret://OPENAI_API_KEY"
    - "secret://ANTHROPIC_API_KEY"
    - "secret://GEMINI_API_KEY"

cloud_read:
  default: ask
cloud_write:
  default: deny

deployment:
  default: deny

browser:
  default: ask
  allow:
    - "http://127.0.0.1/**"
    - "http://localhost/**"
    - "navigate"
    - "click"
    - "fill"
    - "text"
    - "screenshot"

git:
  default: ask
  allow:
    - "status**"
    - "diff**"
    - "log**"
    - "show**"
  deny:
    - "push**"
    - "remote**"
    - "config --global**"

delegation:
  default: ask
  allow:
    - "architect"
    - "researcher"
    - "backend-engineer"
    - "frontend-engineer"
    - "database-engineer"
    - "devops-engineer"
    - "debugger"
    - "tester"
    - "reviewer"
    - "security-reviewer"
    - "documentation-engineer"
"#;
