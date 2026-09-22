mod session;
mod terminal;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
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

    #[arg(
        long,
        env = "OPENFORGE_API_TOKEN",
        global = true,
        hide_env_values = true
    )]
    api_token: Option<String>,

    #[arg(
        long,
        global = true,
        help = "Use an in-process engine instead of the shared OpenForge daemon"
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
        runner: Option<Runner>,
        #[arg(
            long,
            help = "Initialize Git metadata without prompting when the selected folder is not a repository"
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
        runner: Option<Runner>,
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
        runner: Option<Runner>,
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
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Gateway {
        #[command(subcommand)]
        command: GatewayCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    Servers,
    Tools {
        server: String,
    },
    Call {
        server: String,
        tool: String,
        #[arg(default_value = "{}")]
        arguments: String,
    },
}

#[derive(Subcommand)]
enum GatewayCommand {
    Status,
    Connect {
        #[arg(
            long,
            help = "Sevi API key. Omit to enter it using a non-echoing prompt."
        )]
        api_key: Option<String>,
    },
    Disconnect,
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
enum Runner {
    Local,
    Docker,
    Kubernetes,
}

impl Runner {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Kubernetes => "kubernetes",
        }
    }
}

impl From<Runner> for RunnerBackend {
    fn from(value: Runner) -> Self {
        match value {
            Runner::Local => Self::Local,
            Runner::Docker => Self::Docker,
            Runner::Kubernetes => Self::Kubernetes,
        }
    }
}

fn selected_runner(runner: Option<Runner>, docker: bool) -> RunnerBackend {
    runner.map(Into::into).unwrap_or(if docker {
        RunnerBackend::Docker
    } else {
        RunnerBackend::Local
    })
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
            bail!("daemon health check returned {status}");
        }
        let value: Value = serde_json::from_str(&body).context("parse daemon health response")?;
        if value["protocol"] != "openforge.protocol.v2" || value["status"] != "ok" {
            bail!("endpoint is not a compatible OpenForge daemon");
        }
        Ok(value)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = self
            .client
            .post(format!("{}/v1/rpc", self.base_url))
            .json(&json!({
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

        if payload["jsonrpc"] != "2.0" || payload["id"] != id {
            bail!("daemon returned a mismatched JSON-RPC response");
        }
        if !status.is_success() || payload.get("error").is_some_and(|value| !value.is_null()) {
            let message = payload
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("daemon request failed");
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

    if let Command::Init { repo } = &command {
        return init(repo.clone()).await;
    }

    if standalone {
        return run_standalone(config, command).await;
    }

    let daemon = DaemonClient::new(daemon_url, api_token)?;
    daemon
        .health()
        .await
        .context("OpenForge daemon is not running; start it with ./setup.sh or openforge-daemon")?;
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
            let repo = canonical_repo(&repo)?;
            let policy = canonical_policy(&policy)?;
            let selected =
                runner
                    .map(Runner::as_str)
                    .unwrap_or(if docker { "docker" } else { "local" });
            print_json(
                &daemon
                    .rpc(
                        "run/execute",
                        json!({
                            "repo": repo,
                            "run_id": run_id,
                            "policy_path": policy,
                            "runner_backend": selected
                        }),
                    )
                    .await?,
            )?;
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
                let policy = canonical_policy(&policy)?;
                let selected = runner.map(Runner::as_str).unwrap_or(
                    if matches!(mode, Mode::Autonomous) || docker {
                        "docker"
                    } else {
                        "local"
                    },
                );
                let execution = daemon
                    .rpc(
                        "run/execute",
                        json!({
                            "repo": repo,
                            "run_id": run_id,
                            "policy_path": policy,
                            "runner_backend": selected
                        }),
                    )
                    .await?;
                print_json(&json!({"run": run, "tasks": tasks, "execution": execution}))?;
            } else {
                print_json(&json!({"run": run, "tasks": tasks}))?;
            }
        }
        Command::Status { run_id } => {
            let run = daemon.rpc("run/get", json!({"run_id": run_id})).await?;
            let tasks = daemon.rpc("task/list", json!({"run_id": run_id})).await?;
            let budget = daemon
                .rpc("budget/snapshot", json!({"run_id": run_id}))
                .await?;
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
        Command::Providers => {
            let provider_result = daemon.rpc("model/providers", json!({})).await?;
            let providers = provider_array(&provider_result)?;
            let models = daemon.rpc("model/list", json!({})).await?;
            print_json(&json!({"providers": providers, "models": models}))?;
        }
        Command::Mcp { command } => {
            let (method, params) = match command {
                McpCommand::Servers => ("mcp/list_servers", json!({})),
                McpCommand::Tools { server } => ("mcp/list_tools", json!({"server_name": server})),
                McpCommand::Call {
                    server,
                    tool,
                    arguments,
                } => (
                    "mcp/call_tool",
                    json!({"server_name": server, "tool_name": tool, "arguments": serde_json::from_str::<Value>(&arguments).context("invalid tool arguments JSON")?}),
                ),
            };
            print_json(&daemon.rpc(method, params).await?)?;
        }
        Command::Gateway { command } => gateway_command(daemon, command).await?,
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
    runner: Option<Runner>,
    yes_init_git: bool,
) -> Result<()> {
    if !budget.is_finite() || budget <= 0.0 {
        bail!("budget must be positive");
    }
    let repo = prepare_workspace(repo, yes_init_git)?;
    let repo_string = repo.to_string_lossy().into_owned();
    let gateway = daemon.rpc("gateway/status", json!({})).await?;
    let models = daemon.rpc("model/list", json!({})).await?;

    println!("OpenForge interactive terminal");
    println!("workspace: {}", repo.display());
    println!(
        "gateway: {} · model runtime: {}",
        if gateway
            .get("connected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "Sevi connected"
        } else {
            "configured provider"
        },
        models
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model["model"].as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "mode: {} · per-turn budget: USD {:.2} · type /help for commands",
        mode.as_str(),
        budget
    );

    let mut mode = mode;
    let mut budget = budget;
    let mut policy = policy;
    let session_root = session::Session::root()?;
    let mut session = session::Session::new(&repo);
    let mut editor = terminal::Editor::default();
    println!(
        "session: {} · / opens commands · Tab completes · Up/Down selects or recalls history",
        session.id
    );
    loop {
        let Some(input) = editor.read()? else {
            break;
        };
        let input = input.trim_end();
        if input.trim().is_empty() {
            continue;
        }

        match session_command(
            daemon,
            input,
            &mut session,
            &mut mode,
            &mut budget,
            &mut policy,
            &session_root,
        )
        .await
        {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => {
                eprintln!("Command failed: {error}");
                continue;
            }
        }

        if is_file_listing(input) {
            match workspace_listing(&repo) {
                Ok(entries) => {
                    println!("Workspace files and folders (respects ignore rules; excludes .git):");
                    for entry in entries {
                        println!("{entry}");
                    }
                }
                Err(error) => eprintln!("Could not list workspace: {error}"),
            }
            continue;
        }
        match input.trim_end_matches(['.', '!', '?']) {
            "whoami" | "/whoami" => {
                match ProcessCommand::new("id").arg("-un").output() {
                    Ok(output) if output.status.success() => {
                        print!("{}", String::from_utf8_lossy(&output.stdout))
                    }
                    _ => eprintln!("Could not determine the current operating-system user"),
                }
                continue;
            }
            "pwd" | "/pwd" => {
                println!("{}", repo.display());
                continue;
            }
            "/exit" | "/quit" => break,
            "/help" => {
                println!("/help        show terminal commands");
                println!(
                    "/files       list workspace files/folders locally without a model request"
                );
                println!("whoami      show the local operating-system user (no model call)");
                println!("pwd         show the selected workspace (no model call)");
                println!("/gateway     show the shared Sevi gateway status");
                println!("/connect     enter your Sevi key without echoing it");
                println!("/disconnect  disconnect the shared Sevi provider");
                println!("/provider    show the daemon model catalog");
                println!("/context     show recent terminal conversation context");
                println!("/clear       clear terminal conversation context");
                println!("/exit        leave OpenForge");
                continue;
            }
            "/connect" => {
                if let Err(error) =
                    gateway_command(daemon, GatewayCommand::Connect { api_key: None }).await
                {
                    eprintln!("Connection failed: {error}");
                }
                continue;
            }
            "/disconnect" => {
                if let Err(error) = gateway_command(daemon, GatewayCommand::Disconnect).await {
                    eprintln!("Disconnect failed: {error}");
                }
                continue;
            }
            "/gateway" => {
                match daemon.rpc("gateway/status", json!({})).await {
                    Ok(value) => print_json(&value)?,
                    Err(error) => eprintln!("Request failed: {error}"),
                }
                continue;
            }
            "/provider" => {
                match daemon.rpc("model/list", json!({})).await {
                    Ok(value) => print_json(&value)?,
                    Err(error) => eprintln!("Request failed: {error}"),
                }
                continue;
            }
            "/context" => {
                for (role, text) in &session.transcript {
                    println!("{role}: {text}");
                }
                continue;
            }
            "/clear" => {
                session.transcript.clear();
                println!("conversation context cleared");
                continue;
            }
            _ => {}
        }

        let review_policy = if input == "/review" {
            match review_policy(&policy, &session_root, &session) {
                Ok(policy) => Some(policy),
                Err(error) => {
                    eprintln!("Review unavailable: {error}");
                    continue;
                }
            }
        } else {
            None
        };
        let turn_mode = if review_policy.is_some() {
            Mode::Execute
        } else {
            mode
        };
        let turn_policy = review_policy
            .as_ref()
            .map(|value| value.0.as_path())
            .unwrap_or(&policy);
        let outcome: Result<String> = async {
            let request = if input == "/review" {
                format!("Review the following tracked changes for bugs and regressions. Report findings with file references. Do not modify files or execute commands. This is read-only reporting: use empty acceptance checks.\n{}", workspace_diff(&repo, session.last_run)?)
            } else if let Some(inline_plan) = input.strip_prefix("/plan ") {
                inline_plan.to_owned()
            } else {
                input.to_owned()
            };
            let mut objective = conversation_objective(&session.transcript, &request);
            if let Some(goal) = session.goal.as_deref().filter(|_| !session.goal_paused) {
                objective.push_str("\n\nPERSISTENT GOAL\n");
                objective.push_str(goal);
            }
            if let Some(personality) = session.personality.as_deref() {
                if personality != "none" {
                    objective.push_str("\n\nRESPONSE STYLE\n");
                    objective.push_str(match personality {
                        "friendly" => "Be warm, clear, and collaborative while remaining technically precise.",
                        "pragmatic" => "Be concise, implementation-focused, and explicit about tradeoffs and verification.",
                        _ => "",
                    });
                }
            }
            append_mentions(&mut objective, &session)?;
            let instructions = repo.join("AGENTS.md");
            if instructions.exists() {
                let canonical = instructions.canonicalize()?;
                if !canonical.starts_with(&repo) { bail!("AGENTS.md must stay inside the workspace"); }
                if std::fs::metadata(&canonical)?.len() > 64 * 1024 { bail!("AGENTS.md exceeds 64 KiB"); }
                objective.push_str("\n\nPROJECT INSTRUCTIONS (AGENTS.md)\n");
                objective.push_str(&std::fs::read_to_string(canonical)?);
            }
            let run = daemon
                .rpc(
                    "run/create",
                    json!({
                        "repo": repo_string,
                        "objective": objective,
                        "autonomy": turn_mode.as_str(),
                        "budget_usd": budget
                    }),
                )
                .await?;
            let run_id = result_uuid(&run, "id")?;
            session.last_run = Some(run_id);
            session.save(&session_root)?;
            println!("run {run_id}");
            let tasks = daemon
                .rpc("run/plan", json!({"repo": repo_string, "run_id": run_id}))
                .await?;
            let task_count = tasks.as_array().map_or(0, Vec::len);
            println!("planned {task_count} task(s) · run {run_id}");

            let summary = if turn_mode.executes() {
                let selected =
                    runner
                        .map(Runner::as_str)
                        .unwrap_or(if matches!(turn_mode, Mode::Autonomous) {
                            "docker"
                        } else {
                            "local"
                        });
                let policy = canonical_policy(turn_policy)?;
                let execution = daemon
                    .execute_with_feedback(
                        run_id,
                        json!({
                            "repo": repo_string,
                            "run_id": run_id,
                            "policy_path": policy,
                            "runner_backend": selected
                        }),
                    )
                    .await?;
                let branch = execution
                    .get("integration_branch")
                    .and_then(Value::as_str)
                    .unwrap_or("integration branch unavailable");
                let current = daemon.rpc("run/get", json!({"run_id": run_id})).await?;
                println!("completed · {branch}");
                let events = daemon
                    .rpc("event/list", json!({"run_id": run_id, "limit": 1000}))
                    .await?;
                let summaries: Vec<_> = events
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|event| event["event_type"] == "task.completed")
                    .filter_map(|event| event["payload"]["summary"].as_str())
                    .collect();
                for summary in &summaries {
                    println!("{summary}");
                }
                if !summaries.is_empty() {
                    return Ok(summaries.join("\n"));
                }
                format!(
                    "run {run_id} completed on {branch}; status={}",
                    current
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            } else {
                println!(
                    "plan ready; mode {} does not execute changes",
                    mode.as_str()
                );
                format!("run {run_id} planned {task_count} task(s) without execution")
            };

            Ok(summary)
        }
        .await;
        let summary = match outcome {
            Ok(summary) => summary,
            Err(error) => {
                eprintln!(
                    "Request failed: {error}. The terminal remains open; use /files, /gateway or another objective."
                );
                continue;
            }
        };
        session.transcript.push(("user".into(), input.to_string()));
        session.transcript.push(("openforge".into(), summary));
        if session.transcript.len() > 24 {
            session.transcript.drain(0..session.transcript.len() - 24);
        }
        session.save(&session_root)?;
    }
    session.save(&session_root)?;
    Ok(())
}

struct ReviewPolicy(PathBuf);
impl Drop for ReviewPolicy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn review_policy(policy: &Path, root: &Path, session: &session::Session) -> Result<ReviewPolicy> {
    session.save(root)?;
    let mut policy = AgentPolicy::from_yaml(policy)?;
    policy.autonomy = AutonomyLevel::Suggest;
    policy.browser.default = openforge_policy::Decision::Deny;
    policy.browser.allow.clear();
    policy.network.default = openforge_policy::Decision::Deny;
    policy.network.allow.clear();
    let path = root.join(format!("review-policy-{}.json", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)?
        .write_all(&serde_json::to_vec(&policy)?)?;
    Ok(ReviewPolicy(path))
}

fn workspace_diff(repo: &Path, run: Option<Uuid>) -> Result<String> {
    let mut result = String::new();
    let mut refs = vec![vec!["HEAD".to_owned()]];
    if let Some(id) = run {
        let branch = format!("refs/heads/of/integration/{id}");
        if ProcessCommand::new("git")
            .current_dir(repo)
            .args(["rev-parse", "--verify", &branch])
            .output()?
            .status
            .success()
        {
            refs.push(vec!["HEAD".into(), branch]);
        }
    }
    for range in refs {
        let output = ProcessCommand::new("git")
            .current_dir(repo)
            .args(["diff", "--no-ext-diff", "--no-textconv"])
            .args(&range)
            .arg("--")
            .output()?;
        if !output.status.success() {
            bail!("git diff failed; the repository needs an initial commit");
        }
        if result.len() + output.stdout.len() > 128 * 1024 {
            bail!("diff exceeds 128 KiB; narrow the changes before reviewing");
        }
        result.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if result.is_empty() {
        result = "No tracked changes in the workspace or last run".into();
    }
    Ok(result)
}

async fn session_command(
    daemon: &DaemonClient,
    input: &str,
    session: &mut session::Session,
    mode: &mut Mode,
    budget: &mut f64,
    policy: &mut PathBuf,
    root: &Path,
) -> Result<bool> {
    let (command, argument) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
    let argument = argument.trim();
    let accepts_argument = matches!(
        command,
        "/permissions"
            | "/approvals"
            | "/mode"
            | "/budget"
            | "/resume"
            | "/mcp"
            | "/plan"
            | "/goal"
            | "/personality"
            | "/mention"
            | "/rename"
            | "/memories"
            | "/skills"
            | "/stop"
    );
    if !argument.is_empty() && command.starts_with('/') && !accepts_argument {
        bail!("{command} does not accept arguments");
    }
    match command {
        "/" | "/help" => terminal::help(),
        "/status" => {
            println!(
                "session: {}\ntitle: {}\nworkspace: {}\nmode: {}\nbudget: USD {:.2}\npolicy: {}\ngoal: {}\npersonality: {}\nmentioned files: {}",
                session.id,
                session.title.as_deref().unwrap_or("untitled"),
                session.workspace.display(),
                mode.as_str(),
                budget,
                policy.display(),
                session.goal.as_deref().unwrap_or("none"),
                session.personality.as_deref().unwrap_or("none"),
                session.mentions.len()
            );
            if let Some(id) = session.last_run {
                print_json(&daemon.rpc("run/get", json!({"run_id": id})).await?)?;
            }
        }
        "/model" => {
            print_json(&daemon.rpc("model/list", json!({})).await?)?;
            println!(
                "Models are configured by the shared daemon. Sevi auto-select routes upstream; this gateway does not expose model/reasoning selection."
            );
        }
        "/permissions" | "/approvals" => {
            if !argument.is_empty() {
                let path = canonical_policy(Path::new(argument))?;
                AgentPolicy::from_yaml(&path)?;
                *policy = PathBuf::from(path);
            }
            println!(
                "Active policy: {}. Ask decisions stop for approval; changing policy affects future turns.",
                policy.display()
            );
            print_json(&serde_json::to_value(AgentPolicy::from_yaml(&*policy)?)?)?;
        }
        "/plan" => {
            *mode = Mode::Suggest;
            if argument.is_empty() {
                println!(
                    "Planning mode: objectives create plans without execution. Use /mode execute to execute future objectives."
                );
            } else {
                return Ok(false);
            }
        }
        "/mode" => {
            if !argument.is_empty() {
                *mode = match argument {
                    "suggest" => Mode::Suggest,
                    "edit" => Mode::Edit,
                    "execute" => Mode::Execute,
                    _ => bail!("use /mode suggest|edit|execute"),
                };
            }
            println!("Mode: {}", mode.as_str());
        }
        "/budget" => {
            if !argument.is_empty() {
                let value: f64 = argument.parse().context("use /budget POSITIVE_USD")?;
                if !value.is_finite() || value <= 0.0 {
                    bail!("budget must be positive and finite");
                }
                *budget = value;
            }
            println!("Next-turn run budget: USD {budget:.2}");
        }
        "/goal" => {
            match argument {
                "" => println!(
                    "Goal: {}{}",
                    session.goal.as_deref().unwrap_or("none"),
                    if session.goal_paused { " (paused)" } else { "" }
                ),
                "clear" => {
                    session.goal = None;
                    session.goal_paused = false;
                    println!("Persistent goal cleared");
                }
                "pause" => {
                    session.goal.as_ref().context("No goal is set")?;
                    session.goal_paused = true;
                    println!("Persistent goal paused");
                }
                "resume" => {
                    session.goal.as_ref().context("No goal is set")?;
                    session.goal_paused = false;
                    println!("Persistent goal resumed");
                }
                value => {
                    let value = value.strip_prefix("edit ").unwrap_or(value).trim();
                    if value.is_empty() || value.chars().count() > 4000 {
                        bail!("goal must contain 1 to 4000 characters");
                    }
                    session.goal = Some(value.to_string());
                    session.goal_paused = false;
                    println!("Persistent goal set");
                }
            }
            session.save(root)?;
        }
        "/personality" => {
            if argument.is_empty() {
                println!(
                    "Personality: {}",
                    session.personality.as_deref().unwrap_or("none")
                );
            } else {
                match argument {
                    "friendly" | "pragmatic" | "none" => {
                        session.personality = Some(argument.to_string());
                        session.save(root)?;
                        println!("Personality: {argument}");
                    }
                    _ => bail!("use /personality friendly|pragmatic|none"),
                }
            }
        }
        "/mention" => {
            if argument == "clear" {
                session.mentions.clear();
                session.save(root)?;
                println!("Mentioned files cleared");
            } else if argument.is_empty() {
                for path in &session.mentions {
                    println!("{}", path.display());
                }
                if session.mentions.is_empty() {
                    println!("No files are attached to future objectives");
                }
            } else {
                let relative = validate_mentioned_file(&session.workspace, argument)?;
                if !session.mentions.contains(&relative) {
                    session.mentions.push(relative.clone());
                }
                session.save(root)?;
                println!("Attached {}", relative.display());
            }
        }
        "/rename" => {
            if argument.is_empty() || argument.chars().count() > 120 {
                bail!("use /rename TITLE (1 to 120 characters)");
            }
            session.title = Some(argument.to_string());
            session.save(root)?;
            println!("Session renamed: {argument}");
        }
        "/diff" => println!("{}", workspace_diff(&session.workspace, session.last_run)?),
        "/init" => {
            let path = session.workspace.join("AGENTS.md");
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).context("AGENTS.md already exists or cannot be created; existing instructions are preserved")?;
            file.write_all(b"# Project instructions\n\nRead the repository documentation before changing code. Preserve existing behavior unless the task explicitly changes it. Follow the surrounding code style. Run the relevant existing tests for each change, and report any checks that could not run. Never commit credentials or claim validation that was not performed.\n")?;
            println!(
                "Created {}. These instructions are included in future terminal objectives.",
                path.display()
            );
        }
        "/review" => return Ok(false),
        "/new" => {
            session.save(root)?;
            *session = session::Session::new(&session.workspace);
            session.save(root)?;
            println!("New session: {}", session.id);
        }
        "/fork" => {
            session.save(root)?;
            session.id = Uuid::new_v4();
            session.save(root)?;
            println!("Forked session: {}", session.id);
        }
        "/archive" => {
            let workspace = session.workspace.clone();
            let archived = session.archive(root)?;
            *session = session::Session::new(&workspace);
            session.save(root)?;
            println!(
                "Archived session to {}. New session: {}",
                archived.display(),
                session.id
            );
        }
        "/delete" => {
            let workspace = session.workspace.clone();
            let deleted = session.delete(root)?;
            *session = session::Session::new(&workspace);
            session.save(root)?;
            println!(
                "Deleted previous session: {deleted}. New session: {}",
                session.id
            );
        }
        "/resume" => {
            if argument.is_empty() {
                session::Session::list(root, &session.workspace)?;
            } else {
                let resumed = session::Session::load(
                    root,
                    Uuid::parse_str(argument).context("use /resume SESSION_UUID")?,
                    &session.workspace,
                )?;
                session.save(root)?;
                *session = resumed;
                println!(
                    "Resumed {} ({} messages). Current mode and permissions are retained.",
                    session.id,
                    session.transcript.len()
                );
            }
        }
        "/compact" => {
            let removed = session.transcript.len().saturating_sub(6);
            session.transcript.drain(..removed);
            session.save(root)?;
            println!(
                "Removed {removed} older messages; retained {}. This is local truncation, not a model-generated summary.",
                session.transcript.len()
            );
        }
        "/copy" => {
            let output = session
                .latest_output()
                .context("No completed OpenForge output to copy")?;
            print!("\x1b]52;c;{}\x07", BASE64.encode(output.as_bytes()));
            io::stdout().flush()?;
            println!("\nLatest OpenForge output sent to the terminal clipboard");
        }
        "/memories" => {
            let query = if argument == "all" { "" } else { argument };
            print_json(
                &daemon
                    .rpc(
                        "memory/search",
                        json!({"scope":"repository","query":query,"limit":100}),
                    )
                    .await?,
            )?;
        }
        "/skills" => {
            let skills = workspace_skills(&session.workspace, argument)?;
            if skills.is_empty() {
                println!("No matching SKILL.md files found in the workspace");
            } else {
                for skill in skills {
                    println!("{skill}");
                }
            }
        }
        "/mcp" => {
            if argument == "verbose" {
                let servers = daemon.rpc("mcp/list_servers", json!({})).await?;
                print_json(&servers)?;
                for server in servers
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    println!("MCP server: {server}");
                    print_json(
                        &daemon
                            .rpc(
                                "mcp/list_tools",
                                json!({"server_name":server,"policy_path":canonical_policy(policy)?}),
                            )
                            .await?,
                    )?;
                }
            } else {
                let (method, params) = if argument.is_empty() {
                    ("mcp/list_servers", json!({}))
                } else {
                    (
                        "mcp/list_tools",
                        json!({"server_name": argument, "policy_path": canonical_policy(policy)?}),
                    )
                };
                print_json(&daemon.rpc(method, params).await?)?;
            }
        }
        "/apps" | "/plugins" => print_json(&daemon.rpc("plugins/list", json!({})).await?)?,
        "/agent" | "/subagents" => {
            if let Some(id) = session.last_run {
                println!("Run task agents:");
                print_json(&daemon.rpc("task/list", json!({"run_id":id})).await?)?;
            }
            println!("ACP agent processes:");
            print_json(&daemon.rpc("acp/list", json!({})).await?)?;
        }
        "/ps" => print_json(&daemon.rpc("acp/list", json!({})).await?)?,
        "/stop" => {
            if argument.is_empty() {
                bail!("use /stop PROCESS_UUID|all");
            }
            if argument == "all" {
                let processes = daemon.rpc("acp/list", json!({})).await?;
                let ids: Vec<String> = processes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|process| process["process_id"].as_str())
                    .map(str::to_string)
                    .collect();
                for process_id in ids {
                    let _ = daemon
                        .rpc("acp/close", json!({"process_id":process_id}))
                        .await?;
                }
                println!("Stopped all ACP processes");
            } else {
                let process_id = Uuid::parse_str(argument).context("use /stop PROCESS_UUID|all")?;
                print_json(
                    &daemon
                        .rpc("acp/close", json!({"process_id":process_id}))
                        .await?,
                )?;
            }
        }
        "/runs" => print_json(&daemon.rpc("run/list", json!({"limit":20})).await?)?,
        "/tasks" | "/events" => {
            let id = session.last_run.context("No run in this session yet")?;
            print_json(
                &daemon
                    .rpc(
                        if command == "/tasks" {
                            "task/list"
                        } else {
                            "event/list"
                        },
                        json!({"run_id": id, "limit":1000}),
                    )
                    .await?,
            )?;
        }
        "/usage" => {
            let id = session.last_run.context("No run in this session yet")?;
            print_json(&daemon.rpc("budget/snapshot", json!({"run_id":id})).await?)?;
        }
        "/debug-config" => {
            print_json(&json!({
                "initialize": daemon.rpc("initialize", json!({})).await?,
                "gateway": daemon.rpc("gateway/status", json!({})).await?,
                "models": daemon.rpc("model/list", json!({})).await?,
                "policy_path": canonical_policy(policy)?,
                "policy": AgentPolicy::from_yaml(&*policy)?
            }))?;
        }
        "/logout" => {
            print_json(&daemon.rpc("gateway/disconnect", json!({})).await?)?;
        }
        "/clear" => {
            session.transcript.clear();
            session.save(root)?;
            println!("conversation context cleared");
        }
        "/connect" | "/disconnect" | "/gateway" | "/provider" | "/context" | "/files" | "/tree"
        | "/pwd" | "/whoami" | "/exit" | "/quit" => return Ok(false),
        _ if command.starts_with('/') => {
            println!(
                "Unknown command {command}. Type / for available commands. No model request was sent."
            );
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn validate_mentioned_file(workspace: &Path, value: &str) -> Result<PathBuf> {
    let candidate = workspace.join(value);
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("mentioned path {} does not exist", candidate.display()))?;
    if !canonical.starts_with(workspace) {
        bail!("mentioned file must stay inside the workspace");
    }
    if !canonical.is_file() {
        bail!("mentioned path must be a file");
    }
    if std::fs::metadata(&canonical)?.len() > 256 * 1024 {
        bail!("mentioned file exceeds 256 KiB");
    }
    Ok(canonical.strip_prefix(workspace)?.to_path_buf())
}

fn append_mentions(objective: &mut String, session: &session::Session) -> Result<()> {
    if session.mentions.is_empty() {
        return Ok(());
    }
    objective.push_str("\n\nATTACHED WORKSPACE FILES\n");
    for relative in &session.mentions {
        let canonical = session.workspace.join(relative).canonicalize()?;
        if !canonical.starts_with(&session.workspace) || !canonical.is_file() {
            bail!("mentioned file {} is no longer valid", relative.display());
        }
        if std::fs::metadata(&canonical)?.len() > 256 * 1024 {
            bail!("mentioned file {} exceeds 256 KiB", relative.display());
        }
        objective.push_str(&format!("\n--- {} ---\n", relative.display()));
        objective.push_str(&std::fs::read_to_string(&canonical).with_context(|| {
            format!(
                "mentioned file {} is not valid UTF-8 text",
                relative.display()
            )
        })?);
    }
    Ok(())
}

fn workspace_skills(workspace: &Path, filter: &str) -> Result<Vec<String>> {
    let needle = filter.to_ascii_lowercase();
    let mut skills = Vec::new();
    for entry in ignore::WalkBuilder::new(workspace)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
    {
        let entry = entry?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) && entry.file_name() == "SKILL.md" {
            let relative = entry
                .path()
                .strip_prefix(workspace)?
                .to_string_lossy()
                .replace('\\', "/");
            if needle.is_empty() || relative.to_ascii_lowercase().contains(&needle) {
                skills.push(relative);
            }
        }
    }
    skills.sort();
    Ok(skills)
}

impl DaemonClient {
    async fn execute_with_feedback(&self, run_id: Uuid, params: Value) -> Result<Value> {
        let execution = self.rpc("run/execute", params);
        tokio::pin!(execution);
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(2),
        );
        let mut after = 0;
        loop {
            tokio::select! {
                result = &mut execution => return result,
                _ = interval.tick() => {
                    if let Ok(Ok(events)) = tokio::time::timeout(std::time::Duration::from_secs(2), self.rpc("event/list", json!({"run_id":run_id,"after_sequence":after,"limit":100}))).await {
                        for event in events.as_array().into_iter().flatten() {
                            after = after.max(event["sequence"].as_i64().unwrap_or(after));
                            if let Some(kind) = event["event_type"].as_str() { println!("  · {kind}"); }
                        }
                    }
                }
            }
        }
    }
}

fn is_file_listing(input: &str) -> bool {
    let normalized = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    matches!(
        normalized.trim_end_matches(['.', '!', '?']),
        "/files"
            | "/tree"
            | "ls"
            | "ls -a"
            | "ls -all"
            | "ls -la"
            | "list all files and folders"
            | "list files and folders"
            | "list all files"
            | "list files"
            | "list all files in this folder"
            | "list files in this folder"
            | "list all files in this directory"
            | "list files in this directory"
            | "list all files and folders in this folder"
            | "list all files and folders in this directory"
    )
}

fn workspace_listing(repo: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in ignore::WalkBuilder::new(repo)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
    {
        let entry = entry.context("walk workspace")?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry.path().strip_prefix(repo)?;
        let mut display = relative
            .to_string_lossy()
            .chars()
            .flat_map(|character| {
                if character.is_control() {
                    character.escape_default().collect::<Vec<_>>()
                } else {
                    vec![character]
                }
            })
            .collect::<String>();
        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
            display.push('/');
        }
        entries.push(display);
        if entries.len() >= 50_000 {
            bail!("workspace listing exceeds 50,000 entries; narrow the project path");
        }
    }
    entries.sort();
    Ok(entries)
}

async fn gateway_command(daemon: &DaemonClient, command: GatewayCommand) -> Result<()> {
    match command {
        GatewayCommand::Status => {
            print_json(&daemon.rpc("gateway/status", json!({})).await?)?;
        }
        GatewayCommand::Connect { api_key } => {
            let key = match api_key {
                Some(value) if !value.trim().is_empty() => value.trim().to_string(),
                _ => read_secret("Sevi API key: ")?,
            };
            if key.is_empty() {
                bail!("Sevi API key cannot be empty");
            }
            print_json(
                &daemon
                    .rpc("gateway/connect", json!({"api_key": key}))
                    .await?,
            )?;
        }
        GatewayCommand::Disconnect => {
            print_json(&daemon.rpc("gateway/disconnect", json!({})).await?)?;
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
        MemoryCommand::Search {
            query,
            scope,
            limit,
        } => {
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
    if matches!(
        &command,
        Command::Chat { .. } | Command::Gateway { .. } | Command::Mcp { .. }
    ) {
        bail!("chat and gateway commands require the shared OpenForge daemon");
    }

    let config = OpenForgeConfig::load(&config_path)
        .with_context(|| format!("load {}", config_path.display()))?;
    let engine = Engine::new(config)?;

    match command {
        Command::Plan {
            objective,
            repo,
            budget,
            mode,
        } => {
            let run = engine
                .create_run(&repo, objective, mode.into(), budget)
                .await?;
            let tasks = engine.plan_run(&repo, &run).await?;
            print_json(&json!({"run": run, "tasks": tasks}))?;
        }
        Command::Execute {
            run_id,
            repo,
            policy,
            docker,
            runner,
        } => {
            let policy = AgentPolicy::from_yaml(policy)?;
            let integration_branch = engine
                .execute_run_with_backend(&repo, run_id, policy, selected_runner(runner, docker))
                .await?;
            print_json(&json!({
                "run": engine.store.get_run(run_id)?,
                "integration_branch": integration_branch,
                "cost_usd": engine.store.run_cost(run_id)?
            }))?;
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
            let run = engine
                .create_run(&repo, objective, mode.into(), budget)
                .await?;
            let tasks = engine.plan_run(&repo, &run).await?;
            eprintln!("planned {} tasks for run {}", tasks.len(), run.id);
            let policy = AgentPolicy::from_yaml(policy)?;
            let integration_branch = engine
                .execute_run_with_backend(&repo, run.id, policy, selected_runner(runner, docker))
                .await?;
            print_json(&json!({
                "run_id": run.id,
                "status": "completed",
                "integration_branch": integration_branch,
                "cost_usd": engine.store.run_cost(run.id)?
            }))?;
        }
        Command::Status { run_id } => {
            print_json(&json!({
                "run": engine.store.get_run(run_id)?,
                "tasks": engine.store.list_tasks(run_id)?,
                "cost_usd": engine.store.run_cost(run_id)?
            }))?;
        }
        Command::Events { run_id, after } => {
            print_json(&serde_json::to_value(
                engine.store.list_events(run_id, after, 1000)?,
            )?)?;
        }
        Command::Providers => {
            print_json(&json!({
                "providers": engine.providers(),
                "models": engine.models()
            }))?;
        }
        Command::Memory { command } => match command {
            MemoryCommand::Put {
                scope,
                key,
                value_json,
                project_id,
                repository_id,
            } => {
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
            MemoryCommand::Search {
                query,
                scope,
                limit,
            } => {
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
        Command::Init { .. }
        | Command::Chat { .. }
        | Command::Gateway { .. }
        | Command::Mcp { .. } => unreachable!(),
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

fn canonical_policy(policy: &Path) -> Result<String> {
    Ok(policy
        .canonicalize()
        .with_context(|| format!("policy path {} does not exist", policy.display()))?
        .to_string_lossy()
        .into_owned())
}

fn provider_array(result: &Value) -> Result<Value> {
    result
        .get("providers")
        .filter(|value| value.is_array())
        .cloned()
        .context("daemon model/providers result is missing providers array")
}

fn prepare_workspace(repo: PathBuf, yes_init_git: bool) -> Result<PathBuf> {
    let repo = repo
        .canonicalize()
        .with_context(|| format!("workspace {} does not exist", repo.display()))?;
    if git_is_repository(&repo) {
        return Ok(repo);
    }

    let initialize = yes_init_git
        || confirm(
            "This folder is not a Git repository. OpenForge uses Git worktrees for safe edits. Initialize local Git metadata and create a baseline commit? [y/N] ",
        )?;
    if !initialize {
        bail!("OpenForge execution requires a Git workspace");
    }

    run_process(
        ProcessCommand::new("git").arg("-C").arg(&repo).arg("init"),
        "git init",
    )?;
    run_process(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "-A"]),
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
            ProcessCommand::new("git").arg("-C").arg(&repo).args([
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
            run_process(
                ProcessCommand::new("git").arg("-C").arg(&repo).args([
                    "-c",
                    "user.name=OpenForge",
                    "-c",
                    "user.email=openforge@localhost",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "chore: establish OpenForge workspace baseline",
                ]),
                "create empty-folder Git baseline",
            )?;
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
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn conversation_objective(history: &[(String, String)], current: &str) -> String {
    if history.is_empty() {
        return current.to_string();
    }

    let mut context = String::from(
        "You are continuing an interactive OpenForge terminal session. Preserve prior intent, inspect the real workspace, and make only changes required by the current instruction.\n\nRECENT SESSION CONTEXT\n",
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

    if !echo_disabled {
        bail!("Cannot hide terminal input. Enter the key in the OpenForge GUI instead.");
    }
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
        tokio::fs::write(
            repo.join("config/policies/development.yaml"),
            DEFAULT_POLICY,
        )
        .await?;
    }

    println!("initialized OpenForge configuration in {}", repo.display());
    Ok(())
}

const DEFAULT_CONFIG: &str = r#"state_db: .openforge/state.db
artifact_dir: .openforge/artifacts
worktree_dir: .openforge/worktrees
plugin_dir: plugins
max_parallel_agents: 4

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

kubernetes:
  namespace: default
  image: ghcr.io/gan-007/openforge-runner:latest
  wait_seconds: 90

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
    - "secret://GOOGLE_API_KEY"
    - "secret://OPENROUTER_API_KEY"
    - "secret://AWS_ACCESS_KEY_ID"
    - "secret://AWS_SECRET_ACCESS_KEY"
    - "secret://AWS_SESSION_TOKEN"

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::post};

    #[tokio::test]
    async fn remote_cli_sends_auth_and_rejects_mismatched_responses() {
        let app = Router::new().route("/v1/rpc", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer daemon-test-token");
            assert_eq!(body["method"], "gateway/status");
            Json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"connected":true,"model":"auto-select"}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = DaemonClient::new(
            format!("http://{address}"),
            Some("daemon-test-token".into()),
        )
        .unwrap();
        let status = client.rpc("gateway/status", json!({})).await.unwrap();
        assert_eq!(status["model"], "auto-select");
        server.abort();

        let app = Router::new().route(
            "/v1/rpc",
            post(|| async { Json(json!({"jsonrpc":"2.0","id":999,"result":{}})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = DaemonClient::new(format!("http://{address}"), None).unwrap();
        assert!(
            client
                .rpc("gateway/status", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("mismatched")
        );
        server.abort();
    }

    #[test]
    fn daemon_policy_paths_are_canonical_and_provider_shape_is_stable() {
        let policy = std::env::temp_dir().join(format!("openforge-policy-{}.yaml", Uuid::new_v4()));
        std::fs::write(&policy, "autonomy: execute\n").unwrap();
        let canonical = canonical_policy(&policy).unwrap();
        assert!(Path::new(&canonical).is_absolute());
        assert_eq!(
            provider_array(&json!({"providers": ["local", "sevi"]})).unwrap(),
            json!(["local", "sevi"])
        );
        assert!(provider_array(&json!({"providers": {"providers": []}})).is_err());
        let _ = std::fs::remove_file(policy);
    }

    #[test]
    fn chat_preserves_recent_intent_with_unicode_safe_context_limit() {
        let history = vec![("user".into(), "界".repeat(10_000))];
        let objective = conversation_objective(&history, "Fix the tests");
        assert!(objective.len() <= 24_000);
        assert!(objective.ends_with("CURRENT INSTRUCTION\nFix the tests"));
        assert!(
            Cli::try_parse_from([
                "openforge",
                "--daemon-url",
                "http://localhost:8875",
                "chat",
                "."
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["openforge", "--standalone", "providers"]).is_ok());
    }
}

#[cfg(test)]
mod interactive_tests {
    use super::*;
    #[test]
    fn review_policy_denies_mutation_and_is_removed_after_use() {
        let root = tempfile::tempdir().unwrap();
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/policies/development.yaml");
        let session = session::Session::new(root.path());
        let temporary = review_policy(&source, root.path(), &session).unwrap();
        let path = temporary.0.clone();
        let policy = AgentPolicy::from_yaml(&path).unwrap();
        use openforge_policy::{CapabilityRequest, Decision};
        assert_eq!(
            policy.evaluate(CapabilityRequest::ReadPath("README.md")),
            Decision::Allow
        );
        assert_eq!(
            policy.evaluate(CapabilityRequest::WritePath("README.md")),
            Decision::Deny
        );
        assert_eq!(
            policy.evaluate(CapabilityRequest::Process(&["git".into(), "status".into()])),
            Decision::Deny
        );
        assert_eq!(
            policy.evaluate(CapabilityRequest::Mcp("github/create_issue")),
            Decision::Deny
        );
        drop(temporary);
        assert!(!path.exists());
    }
}
