use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use openforge_core::{Engine, OpenForgeConfig, RunnerBackend};
use openforge_policy::AgentPolicy;
use openforge_protocol::AutonomyLevel;
use serde_json::Value;
use std::path::PathBuf;
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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(default_value = ".")]
        repo: PathBuf,
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
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
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

#[derive(Clone, Copy, ValueEnum)]
enum Runner {
    Local,
    Docker,
    Kubernetes,
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Command::Init { repo } = cli.command {
        return init(repo).await;
    }

    let config = OpenForgeConfig::load(&cli.config)
        .with_context(|| format!("load {}", cli.config.display()))?;
    let engine = Engine::new(config)?;

    match cli.command {
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
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"run": run, "tasks": tasks}))?
            );
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
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "run": engine.store.get_run(run_id)?,
                    "integration_branch": integration_branch,
                    "cost_usd": engine.store.run_cost(run_id)?
                }))?
            );
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

            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "run_id": run.id,
                    "status": "completed",
                    "integration_branch": integration_branch,
                    "cost_usd": engine.store.run_cost(run.id)?
                }))?
            );
        }
        Command::Status { run_id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "run": engine.store.get_run(run_id)?,
                    "tasks": engine.store.list_tasks(run_id)?,
                    "cost_usd": engine.store.run_cost(run_id)?
                }))?
            );
        }
        Command::Events { run_id, after } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&engine.store.list_events(run_id, after, 1000)?)?
            );
        }
        Command::Providers => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"providers": engine.providers()})
                )?
            );
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
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"stored": true, "scope": scope, "key": key})
                    )?
                );
            }
            MemoryCommand::Search {
                query,
                scope,
                limit,
            } => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&engine.store.memory_search(
                        scope.as_deref(),
                        &query,
                        limit
                    )?)?
                );
            }
            MemoryCommand::Forget { key, scope } => {
                let deleted = engine.store.memory_delete(scope.as_deref(), &key)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"deleted": deleted}))?
                );
            }
        },
        Command::Init { .. } => unreachable!(),
    }

    Ok(())
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
