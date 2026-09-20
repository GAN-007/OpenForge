use crate::{AgentLoop, OpenForgeConfig, ToolBus};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use futures_util::future::join_all;
use openforge_acp::AcpAgentClient;
use openforge_context::RepositoryIndex;
use openforge_git::{GitBroker, GitWorkspace};
use openforge_models::{
    AnthropicConfig, AnthropicProvider, BedrockCliConfig, BedrockCliProvider, FabricProvider,
    GeminiConfig, GeminiProvider, ModelProvider, ModelRouter, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider,
};
use openforge_policy::AgentPolicy;
use openforge_protocol::{
    Actor, AutonomyLevel, Budget, CapabilityDomain, ChatMessage, ModelRequest, ModelRequirements,
    ResourceLimits, Run, RunStatus, SandboxSecurityProfile, TaskBudget, TaskNode, TaskRequirements,
    TaskStatus,
};
use openforge_sandbox::{
    DockerBackend, ExecRequest, KubernetesBackend, LocalProcessBackend, SandboxBackend,
    SandboxPolicy,
};
use openforge_scheduler::{SchedulerConfig, schedule_wave, validate_dag};
use openforge_search::SearchIndex;
use openforge_store::{CostRecord, Store};
use openforge_symbols::SymbolGraph;
use serde::Deserialize;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerBackend {
    Local,
    Docker,
    Kubernetes,
}

impl RunnerBackend {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "local" | "local-process" => Ok(Self::Local),
            "docker" => Ok(Self::Docker),
            "kubernetes" | "k8s" => Ok(Self::Kubernetes),
            other => bail!("unknown runner backend {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionInput {
    pub run_id: Uuid,
    pub file_path: String,
    pub language: String,
    pub prefix: String,
    pub suffix: String,
    pub max_output_tokens: u32,
    pub max_cost_usd: f64,
}

pub struct Engine {
    pub config: OpenForgeConfig,
    pub store: Store,
    pub tool_bus: Arc<ToolBus>,
    pub acp_clients: tokio::sync::Mutex<
        std::collections::HashMap<Uuid, Arc<tokio::sync::Mutex<AcpAgentClient>>>,
    >,
    fabric: Arc<dyn ModelProvider>,
    provider_names: Vec<String>,
}

struct TaskExecution {
    task: TaskNode,
    commit: Option<String>,
    summary: String,
    iterations: u32,
}

impl Engine {
    pub fn new(config: OpenForgeConfig) -> Result<Self> {
        let store = Store::open(&config.state_db)?;
        let mut providers: Vec<Arc<dyn ModelProvider>> = Vec::new();

        for provider in &config.providers {
            let models = provider
                .models
                .iter()
                .map(|model| model.to_spec(&provider.name))
                .collect::<Vec<_>>();

            match provider.kind.as_str() {
                "openai-compatible" => {
                    if provider.base_url.trim().is_empty() {
                        bail!("provider {} requires base_url", provider.name);
                    }
                    let api_key = provider
                        .api_key_env
                        .as_ref()
                        .and_then(|name| std::env::var(name).ok());
                    providers.push(Arc::new(OpenAiCompatibleProvider::new(
                        OpenAiCompatibleConfig {
                            provider_name: provider.name.clone(),
                            base_url: provider.base_url.clone(),
                            api_key,
                            extra_headers: provider
                                .headers
                                .iter()
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect(),
                            models,
                        },
                    )?));
                }
                "anthropic" => {
                    let env = provider
                        .api_key_env
                        .as_ref()
                        .context("Anthropic provider requires api_key_env")?;
                    let api_key = std::env::var(env).with_context(|| format!("missing {env}"))?;
                    providers.push(Arc::new(AnthropicProvider::new(AnthropicConfig {
                        provider_name: provider.name.clone(),
                        base_url: if provider.base_url.is_empty() {
                            "https://api.anthropic.com".into()
                        } else {
                            provider.base_url.clone()
                        },
                        api_key,
                        models,
                    })?));
                }
                "gemini" => {
                    let env = provider
                        .api_key_env
                        .as_ref()
                        .context("Gemini provider requires api_key_env")?;
                    let api_key = std::env::var(env).with_context(|| format!("missing {env}"))?;
                    providers.push(Arc::new(GeminiProvider::new(GeminiConfig {
                        provider_name: provider.name.clone(),
                        base_url: if provider.base_url.is_empty() {
                            "https://generativelanguage.googleapis.com/v1beta".into()
                        } else {
                            provider.base_url.clone()
                        },
                        api_key,
                        models,
                    })?));
                }
                "bedrock-aws-cli" => {
                    let region = provider
                        .region
                        .clone()
                        .or_else(|| std::env::var("AWS_REGION").ok())
                        .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                        .unwrap_or_else(|| "us-east-1".into());
                    providers.push(Arc::new(BedrockCliProvider::new(BedrockCliConfig {
                        provider_name: provider.name.clone(),
                        region,
                        models,
                    })));
                }
                other => bail!("unsupported provider kind {other}"),
            }
        }

        let fabric_impl = Arc::new(FabricProvider::new(providers)?);
        let provider_names = fabric_impl.provider_names();
        let fabric: Arc<dyn ModelProvider> = fabric_impl;

        let tool_bus = Arc::new(ToolBus::new(
            config.mcp_servers.clone(),
            config.browser.clone(),
        )?);

        Ok(Self {
            config,
            store,
            tool_bus,
            acp_clients: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            fabric,
            provider_names,
        })
    }

    pub fn models(&self) -> &[openforge_protocol::ModelSpec] {
        self.fabric.catalog()
    }

    pub fn providers(&self) -> &[String] {
        &self.provider_names
    }

    pub async fn create_run(
        &self,
        repo: &Path,
        objective: String,
        autonomy: AutonomyLevel,
        budget_usd: f64,
    ) -> Result<Run> {
        if objective.trim().is_empty() {
            bail!("objective cannot be empty");
        }
        if !budget_usd.is_finite() || budget_usd <= 0.0 {
            bail!("budget must be positive");
        }

        let head = git_output(repo, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .to_string();

        let run = Run {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            objective,
            base_sha: head,
            status: RunStatus::Planning,
            autonomy,
            budget: Budget {
                currency: "USD".into(),
                soft_limit: Some(budget_usd * 0.75),
                hard_limit: budget_usd,
                spent: 0.0,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.store.create_run(&run)?;
        self.store.append_event(
            Some(run.id),
            None,
            Actor {
                kind: "user".into(),
                id: "local".into(),
            },
            "run.created",
            json!({
                "objective": run.objective,
                "base_sha": run.base_sha,
                "autonomy": run.autonomy
            }),
        )?;
        Ok(run)
    }

    pub async fn plan_run(&self, repo: &Path, run: &Run) -> Result<Vec<TaskNode>> {
        let index = RepositoryIndex::build(repo)?;
        let relevant = index.relevant_files(&run.objective, 40);
        let mut context_block = String::new();
        if !relevant.is_empty() {
            context_block.push_str(
                "\n\nRELEVANT REPOSITORY CONTEXT\nUse these files to scope tasks precisely:\n",
            );
            for path in &relevant {
                if let Some(record) = index.file(path) {
                    context_block.push_str(&format!(
                        "- {} ({} lines, {} bytes, language: {})\n",
                        record.path, record.lines, record.bytes, record.language
                    ));
                }
            }
        }
        let enriched_objective = format!("{}{}", run.objective, context_block);
        let summary = serde_json::to_string(&index)?;
        let planning_budget = (run.budget.remaining() * 0.20).max(0.05);

        let request = ModelRequest {
            invocation_id: Uuid::new_v4(),
            run_id: run.id,
            task_id: None,
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: planner_prompt(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: format!(
                        "OBJECTIVE\n{enriched_objective}\n\nREPOSITORY INDEX\n{summary}"
                    ),
                },
            ],
            requirements: ModelRequirements {
                task_class: "planning".into(),
                context_tokens: 32_000,
                requires_tools: false,
                requires_vision: false,
                requires_structured_output: true,
                max_cost_usd: planning_budget,
                max_latency_ms: None,
                data_classification: Default::default(),
                preferred_model_families: vec![],
                excluded_model_families: vec![],
            },
            temperature: 0.1,
            max_output_tokens: 6000,
            response_schema: None,
        };

        let router = ModelRouter::default();
        let model = router.select(self.fabric.catalog(), &request.requirements, 20_000, 4_000)?;
        let response = self.fabric.invoke(model, &request).await?;

        self.store.record_cost(CostRecord {
            run_id: run.id,
            task_id: None,
            agent_id: Some("planner"),
            provider: &response.provider,
            model: &response.model,
            amount_usd: response.cost_usd,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
        })?;

        if response.cost_usd > planning_budget {
            bail!("planner exceeded its hard cost budget");
        }

        let plan: Plan = serde_json::from_str(extract_json(&response.text))
            .context("planner returned invalid JSON")?;
        if plan.tasks.is_empty() {
            bail!("planner returned no tasks");
        }

        let allocated = plan.tasks.iter().map(|task| task.max_usd).sum::<f64>();
        let available = (run.budget.hard_limit - self.store.run_cost(run.id)?).max(0.0);
        if !allocated.is_finite() || allocated <= 0.0 || allocated > available {
            bail!("planner allocated task budget {allocated:.4} but only {available:.4} remains");
        }

        let now = Utc::now();
        let mut id_map = std::collections::HashMap::new();
        for task in &plan.tasks {
            if id_map.insert(task.key.clone(), Uuid::new_v4()).is_some() {
                bail!("duplicate task key {}", task.key);
            }
        }

        let mut tasks = Vec::with_capacity(plan.tasks.len());
        for task in plan.tasks {
            let dependencies = task
                .depends_on
                .iter()
                .map(|key| {
                    id_map
                        .get(key)
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("unknown dependency key {key}"))
                })
                .collect::<Result<Vec<_>>>()?;

            let node = TaskNode {
                id: id_map[&task.key],
                run_id: run.id,
                title: task.title,
                description: task.description,
                role: task.role,
                dependencies,
                required_reviews: task.required_reviews.clone(),
                acceptance: task
                    .acceptance
                    .into_iter()
                    .map(|argv| openforge_protocol::AcceptanceCommand {
                        argv,
                        timeout_seconds: task.resources.wall_seconds.clamp(1, 600),
                    })
                    .collect(),
                status: TaskStatus::Pending,
                attempts: 0,
                max_attempts: task.max_attempts.max(1),
                budget: TaskBudget {
                    max_usd: task.max_usd,
                    max_model_calls: task.max_model_calls.max(1),
                    max_tool_calls: task.max_tool_calls.max(1),
                    max_wall_seconds: task.resources.wall_seconds.max(1),
                },
                requirements: TaskRequirements {
                    capabilities: task.capabilities,
                    resources: task.resources,
                    preferred_languages: task.preferred_languages,
                    required_reviews: task.required_reviews,
                    exclusive_resources: task.exclusive_resources,
                },
                created_at: now,
                updated_at: now,
            };

            self.store.upsert_task(&node)?;
            tasks.push(node);
        }

        validate_dag(&tasks)?;
        self.store
            .update_run_status(run.id, RunStatus::AwaitingApproval)?;
        self.store.append_event(
            Some(run.id),
            None,
            Actor {
                kind: "agent".into(),
                id: "planner".into(),
            },
            "run.planned",
            json!({"tasks": tasks}),
        )?;

        Ok(tasks)
    }

    pub async fn execute_run(
        &self,
        repo: &Path,
        run_id: Uuid,
        policy: AgentPolicy,
        use_docker: bool,
    ) -> Result<String> {
        self.execute_run_with_backend(
            repo,
            run_id,
            policy,
            if use_docker {
                RunnerBackend::Docker
            } else {
                RunnerBackend::Local
            },
        )
        .await
    }

    pub async fn execute_run_with_backend(
        &self,
        repo: &Path,
        run_id: Uuid,
        policy: AgentPolicy,
        runner_backend: RunnerBackend,
    ) -> Result<String> {
        let run = self.store.get_run(run_id)?.context("run not found")?;
        let tasks = self.store.list_tasks(run_id)?;
        if tasks.is_empty() {
            bail!("run must be planned before execution");
        }
        validate_dag(&tasks)?;
        if run.autonomy == AutonomyLevel::Autonomous && runner_backend == RunnerBackend::Local {
            bail!("autonomous mode requires an isolated sandbox backend");
        }
        self.store.claim_run(run_id)?;
        let result = self
            .execute_claimed_run(repo, run_id, policy, runner_backend)
            .await;
        if let Err(error) = &result {
            self.store.update_run_status(run_id, RunStatus::Failed)?;
            self.store.append_event(
                Some(run_id),
                None,
                Actor {
                    kind: "system".into(),
                    id: "engine".into(),
                },
                "run.failed",
                json!({"error": error.to_string()}),
            )?;
        }
        result
    }

    async fn execute_claimed_run(
        &self,
        repo: &Path,
        run_id: Uuid,
        policy: AgentPolicy,
        runner_backend: RunnerBackend,
    ) -> Result<String> {
        let run = self.store.get_run(run_id)?.context("run not found")?;
        let mut tasks = self.store.list_tasks(run_id)?;
        validate_dag(&tasks)?;

        if run.autonomy == AutonomyLevel::Autonomous && runner_backend == RunnerBackend::Local {
            bail!("autonomous mode requires an isolated sandbox backend");
        }

        let git = GitBroker::open(repo, PathBuf::from(&self.config.worktree_dir)).await?;
        let integration = git
            .create_integration_workspace(run_id, &run.base_sha)
            .await?;

        loop {
            tasks = self.store.list_tasks(run_id)?;
            if tasks
                .iter()
                .all(|task| task.status == TaskStatus::Completed)
            {
                break;
            }

            let remaining_run_budget =
                (run.budget.hard_limit - self.store.run_cost(run.id)?).max(0.0);
            let wave = schedule_wave(
                &tasks,
                &SchedulerConfig {
                    max_parallel: self.config.max_parallel_agents.max(1),
                    max_wave_cost_usd: remaining_run_budget,
                    retry_failed: true,
                },
            )?;
            if wave.tasks.is_empty() {
                let failed = tasks
                    .iter()
                    .filter(|task| {
                        matches!(
                            task.status,
                            TaskStatus::Failed
                                | TaskStatus::Blocked
                                | TaskStatus::Cancelled
                                | TaskStatus::AwaitingApproval
                        )
                    })
                    .count();
                bail!(
                    "run cannot progress; scheduler admitted no tasks ({failed} failed, blocked, cancelled, or awaiting approval; remaining_budget={remaining_run_budget:.4})"
                );
            }

            self.store.append_event(
                Some(run.id),
                None,
                Actor {
                    kind: "system".into(),
                    id: "scheduler".into(),
                },
                "scheduler.waveAdmitted",
                json!({
                    "tasks": wave.tasks,
                    "reserved_cost_usd": wave.reserved_cost_usd,
                    "critical_depth": wave.critical_depth
                }),
            )?;

            let base_sha = git.workspace_head(&integration).await?;
            let by_id: std::collections::HashMap<Uuid, TaskNode> =
                tasks.iter().cloned().map(|task| (task.id, task)).collect();
            let batch: Vec<TaskNode> = wave
                .tasks
                .iter()
                .filter_map(|id| by_id.get(id).cloned())
                .collect();

            let results = join_all(batch.iter().map(|task| {
                self.execute_task(&git, &run, task, policy.clone(), runner_backend, &base_sha)
            }))
            .await;

            for (task, result) in batch.iter().zip(results) {
                let execution = result?;

                if let Some(commit) = &execution.commit {
                    let pre_integration_sha = git.workspace_head(&integration).await?;
                    let integrated_sha = match git.integrate_commit(&integration, commit).await {
                        Ok(sha) => sha,
                        Err(error) => {
                            let mut failed = task.clone();
                            failed.status = TaskStatus::Failed;
                            failed.updated_at = Utc::now();
                            let _ = self.store.upsert_task(&failed);
                            return Err(error);
                        }
                    };

                    if let Err(error) = self
                        .verify_acceptance(&integration, &execution.task, runner_backend)
                        .await
                    {
                        git.reset_hard(&integration, &pre_integration_sha).await?;
                        let mut failed = task.clone();
                        failed.status = TaskStatus::Failed;
                        failed.updated_at = Utc::now();
                        self.store.upsert_task(&failed)?;
                        self.store.append_event(
                            Some(run.id),
                            Some(task.id),
                            Actor {
                                kind: "system".into(),
                                id: "merge-coordinator".into(),
                            },
                            "git.integrationRolledBack",
                            json!({
                                "source_commit": commit,
                                "reverted_to": pre_integration_sha,
                                "failed_integration_sha": integrated_sha,
                                "error": error.to_string()
                            }),
                        )?;
                        return Err(error.context("post-integration acceptance failed"));
                    }

                    self.store.append_event(
                        Some(run.id),
                        Some(task.id),
                        Actor {
                            kind: "system".into(),
                            id: "merge-coordinator".into(),
                        },
                        "git.integrationCompleted",
                        json!({
                            "source_commit": commit,
                            "integration_sha": integrated_sha,
                            "branch": integration.branch
                        }),
                    )?;
                }

                let mut completed = execution.task;
                completed.status = TaskStatus::Completed;
                completed.updated_at = Utc::now();
                self.store.upsert_task(&completed)?;
                self.store.append_event(
                    Some(run.id),
                    Some(task.id),
                    Actor {
                        kind: "agent".into(),
                        id: task.role.clone(),
                    },
                    "task.completed",
                    json!({
                        "summary": execution.summary,
                        "commit": execution.commit,
                        "iterations": execution.iterations
                    }),
                )?;
            }
        }

        let integration_sha = git.workspace_head(&integration).await?;
        let integration_branch = integration.branch.clone();
        git.remove_workspace(&integration).await?;

        self.store.update_run_status(run_id, RunStatus::Completed)?;
        self.store.append_event(
            Some(run_id),
            None,
            Actor {
                kind: "system".into(),
                id: "engine".into(),
            },
            "run.completed",
            json!({
                "cost_usd": self.store.run_cost(run_id)?,
                "integration_branch": integration_branch,
                "integration_sha": integration_sha
            }),
        )?;

        Ok(integration_branch)
    }

    pub async fn completion(&self, input: CompletionInput) -> Result<String> {
        let CompletionInput {
            run_id,
            file_path,
            language,
            prefix,
            suffix,
            max_output_tokens,
            max_cost_usd,
        } = input;
        let run = self.store.get_run(run_id)?.context("run not found")?;
        if max_output_tokens == 0 || max_output_tokens > 2048 {
            bail!("max_output_tokens must be between 1 and 2048");
        }
        if !max_cost_usd.is_finite() || max_cost_usd < 0.0 {
            bail!("invalid completion budget");
        }

        let request = ModelRequest {
            invocation_id: Uuid::new_v4(),
            run_id,
            task_id: None,
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "Return only the code that should be inserted at the cursor. Do not use Markdown fences, explanations, or duplicate the existing prefix/suffix.".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: format!(
                        "FILE: {file_path}\nLANGUAGE: {language}\n\nPREFIX:\n{prefix}\n\nSUFFIX:\n{suffix}"
                    ),
                },
            ],
            requirements: ModelRequirements {
                task_class: "autocomplete".into(),
                context_tokens: 8_192,
                requires_tools: false,
                requires_vision: false,
                requires_structured_output: false,
                max_cost_usd,
                max_latency_ms: Some(4_000),
                data_classification: Default::default(),
                preferred_model_families: vec![],
                excluded_model_families: vec![],
            },
            temperature: 0.0,
            max_output_tokens,
            response_schema: None,
        };

        let router = ModelRouter::default();
        let model = router.select(
            self.fabric.catalog(),
            &request.requirements,
            6_000,
            max_output_tokens as u64,
        )?;
        let response = self.fabric.invoke(model, &request).await?;

        self.store.record_cost(CostRecord {
            run_id,
            task_id: None,
            agent_id: Some("autocomplete"),
            provider: &response.provider,
            model: &response.model,
            amount_usd: response.cost_usd,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
        })?;

        if response.cost_usd > max_cost_usd {
            bail!("completion exceeded its hard cost budget");
        }
        if self.store.run_cost(run_id)? > run.budget.hard_limit {
            bail!("run budget exhausted");
        }

        self.store.append_event(
            Some(run_id),
            None,
            Actor {
                kind: "agent".into(),
                id: "autocomplete".into(),
            },
            "model.completed",
            json!({
                "provider": response.provider,
                "model": response.model,
                "cost_usd": response.cost_usd,
                "latency_ms": response.latency_ms,
                "file_path": file_path
            }),
        )?;

        Ok(strip_fences(&response.text))
    }

    async fn execute_task(
        &self,
        git: &GitBroker,
        run: &Run,
        task: &TaskNode,
        policy: AgentPolicy,
        runner_backend: RunnerBackend,
        base_sha: &str,
    ) -> Result<TaskExecution> {
        let mut current = task.clone();
        current.status = TaskStatus::Running;
        current.attempts += 1;
        current.updated_at = Utc::now();
        self.store.upsert_task(&current)?;
        self.store.append_event(
            Some(run.id),
            Some(task.id),
            Actor {
                kind: "system".into(),
                id: "scheduler".into(),
            },
            "task.started",
            json!({
                "role": task.role,
                "attempt": current.attempts,
                "base_sha": base_sha
            }),
        )?;

        let workspace = git.create_task_workspace(task.id, base_sha).await?;
        let backend = self.backend(runner_backend);
        let resource_limits = &current.requirements.resources;
        let network_enabled = current
            .requirements
            .capabilities
            .contains(&CapabilityDomain::Network);
        let lease = backend
            .create(
                &workspace.path,
                SandboxPolicy {
                    cpus: resource_limits.cpu_cores,
                    memory_mb: resource_limits.memory_mb,
                    pids_limit: resource_limits.pids,
                    network_enabled,
                    disk_mb: resource_limits.disk_mb,
                    max_stdout_bytes: resource_limits.max_stdout_bytes,
                    max_stderr_bytes: resource_limits.max_stderr_bytes,
                    environment: self.config.environment.clone(),
                    security: SandboxSecurityProfile {
                        network_mode: if network_enabled {
                            "restricted".into()
                        } else {
                            "none".into()
                        },
                        ..SandboxSecurityProfile::default()
                    },
                },
            )
            .await?;
        let context = self.task_context(&workspace, &current).await?;

        let tools = Arc::new(ToolBus::new(
            self.config.mcp_servers.clone(),
            self.config.browser.clone(),
        )?);
        let agent = AgentLoop {
            store: self.store.clone(),
            provider: self.fabric.clone(),
            router: ModelRouter::default(),
            sandbox: backend.clone(),
            tools: tools.clone(),
            policy,
            max_iterations: 30,
        };

        let result = agent.run(&current, &workspace.path, &lease, context).await;
        let tool_cleanup = tools.close().await;

        let execution = match result {
            Ok(outcome) if outcome.success => {
                tool_cleanup.context("tool bus cleanup failed")?;
                self.verify_with_backend(&backend, &lease, &current).await?;
                let commit = git
                    .commit_all(
                        &workspace,
                        &format!("feat(openforge-agent): {}", current.title),
                    )
                    .await?;

                current.status = TaskStatus::Reviewing;
                current.updated_at = Utc::now();
                self.store.upsert_task(&current)?;

                TaskExecution {
                    task: current,
                    commit,
                    summary: outcome.summary,
                    iterations: outcome.iterations,
                }
            }
            Ok(outcome) => {
                if let Err(cleanup_error) = tool_cleanup {
                    tracing::warn!(
                        task_id = %task.id,
                        error = %cleanup_error,
                        "tool bus cleanup failed after agent failure"
                    );
                }
                current.status = TaskStatus::Failed;
                current.updated_at = Utc::now();
                self.store.upsert_task(&current)?;
                backend.destroy(lease).await?;
                git.remove_workspace(&workspace).await?;
                bail!("agent reported failure: {}", outcome.summary);
            }
            Err(error) => {
                if let Err(cleanup_error) = tool_cleanup {
                    tracing::warn!(
                        task_id = %task.id,
                        error = %cleanup_error,
                        "tool bus cleanup failed after agent error"
                    );
                }
                current.status = if error.to_string().contains("requires approval") {
                    TaskStatus::AwaitingApproval
                } else {
                    TaskStatus::Failed
                };
                current.updated_at = Utc::now();
                self.store.upsert_task(&current)?;
                let _ = self.store.append_event(
                    Some(run.id),
                    Some(task.id),
                    Actor {
                        kind: "system".into(),
                        id: "engine".into(),
                    },
                    "task.failed",
                    json!({"error": error.to_string()}),
                );
                backend.destroy(lease).await?;
                git.remove_workspace(&workspace).await?;
                return Err(error);
            }
        };

        backend.destroy(lease).await?;
        git.remove_workspace(&workspace).await?;
        Ok(execution)
    }

    fn backend(&self, runner_backend: RunnerBackend) -> Arc<dyn SandboxBackend> {
        match runner_backend {
            RunnerBackend::Local => Arc::new(LocalProcessBackend),
            RunnerBackend::Docker => Arc::new(DockerBackend {
                image: "ghcr.io/gan-007/openforge-runner:latest".into(),
            }),
            RunnerBackend::Kubernetes => Arc::new(KubernetesBackend {
                image: self.config.kubernetes.image.clone(),
                namespace: self.config.kubernetes.namespace.clone(),
                wait_seconds: self.config.kubernetes.wait_seconds,
            }),
        }
    }

    async fn verify_with_backend(
        &self,
        backend: &Arc<dyn SandboxBackend>,
        lease: &openforge_sandbox::SandboxLease,
        task: &TaskNode,
    ) -> Result<()> {
        for check in &task.acceptance {
            if check.argv.is_empty() {
                bail!("acceptance command cannot be empty");
            }
            let result = backend
                .exec(
                    lease,
                    ExecRequest {
                        argv: check.argv.clone(),
                        cwd: ".".into(),
                        environment: Default::default(),
                        timeout_seconds: check.timeout_seconds.max(1),
                    },
                )
                .await?;

            if result.exit_code != 0 {
                bail!(
                    "acceptance command failed: {:?}\nSTDOUT\n{}\nSTDERR\n{}",
                    check.argv,
                    result.stdout,
                    result.stderr
                );
            }
        }
        Ok(())
    }

    async fn verify_acceptance(
        &self,
        workspace: &GitWorkspace,
        task: &TaskNode,
        runner_backend: RunnerBackend,
    ) -> Result<()> {
        if task.acceptance.is_empty() {
            return Ok(());
        }

        let backend = self.backend(runner_backend);
        let lease = backend
            .create(&workspace.path, SandboxPolicy::default())
            .await?;
        let result = self.verify_with_backend(&backend, &lease, task).await;
        backend.destroy(lease).await?;
        result
    }

    async fn task_context(&self, workspace: &GitWorkspace, task: &TaskNode) -> Result<String> {
        let index = RepositoryIndex::build(&workspace.path)?;
        let search = SearchIndex::build(&workspace.path)?;
        let symbols = SymbolGraph::build(&workspace.path)?;
        let query = format!("{} {}", task.title, task.description);

        let lexical_hits = search.query(&query, 24);
        let symbol_hits = symbols.search(&query, 24);
        let mut relevant = index.relevant_files(&query, 24);

        for hit in &lexical_hits {
            relevant.push(hit.path.clone());
        }
        for symbol in &symbol_hits {
            relevant.push(symbol.path.clone());
        }
        relevant.sort();
        relevant.dedup();
        relevant.truncate(36);

        let mut output = String::new();
        output.push_str("REPOSITORY SNAPSHOT\n");
        output.push_str(&serde_json::to_string(&serde_json::json!({
            "fingerprint": index.fingerprint,
            "files": index.files.len(),
            "total_bytes": index.total_bytes,
            "total_lines": index.total_lines,
            "languages": index.language_counts,
            "search_stats": search.stats(),
            "symbol_stats": symbols.stats()
        }))?);

        output.push_str("\n\nLEXICAL HITS\n");
        output.push_str(&serde_json::to_string(&lexical_hits)?);
        output.push_str("\n\nSYMBOL HITS\n");
        output.push_str(&serde_json::to_string(&symbol_hits)?);

        for relative in relevant {
            let path = workspace.path.join(&relative);
            if let Ok(text) = tokio::fs::read_to_string(&path).await {
                let end = text
                    .char_indices()
                    .nth(16_000)
                    .map(|(index, _)| index)
                    .unwrap_or(text.len());
                output.push_str(&format!("\n\nFILE {relative}\n{}", &text[..end]));
            }
        }

        Ok(output)
    }
}

#[derive(Debug, Deserialize)]
struct Plan {
    tasks: Vec<PlanTask>,
}

#[derive(Debug, Deserialize)]
struct PlanTask {
    key: String,
    title: String,
    description: String,
    role: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    required_reviews: Vec<String>,
    #[serde(default)]
    acceptance: Vec<Vec<String>>,
    #[serde(default)]
    capabilities: Vec<CapabilityDomain>,
    #[serde(default)]
    resources: ResourceLimits,
    #[serde(default)]
    preferred_languages: Vec<String>,
    #[serde(default)]
    exclusive_resources: Vec<String>,
    #[serde(default = "default_task_attempts")]
    max_attempts: u32,
    #[serde(default = "default_model_calls")]
    max_model_calls: u32,
    #[serde(default = "default_tool_calls")]
    max_tool_calls: u32,
    max_usd: f64,
}

fn default_task_attempts() -> u32 {
    2
}
fn default_model_calls() -> u32 {
    30
}
fn default_tool_calls() -> u32 {
    200
}

fn planner_prompt() -> String {
    r#"You are OpenForge's deterministic engineering planner. Return ONLY JSON:
{"tasks":[{"key":"unique-key","title":"concise","description":"complete implementation requirements","role":"architect|researcher|backend-engineer|frontend-engineer|database-engineer|devops-engineer|debugger|tester|reviewer|security-reviewer|documentation-engineer","depends_on":[],"required_reviews":[],"acceptance":[["command","arg"]],"capabilities":["filesystem_read","filesystem_write","process"],"resources":{"cpu_cores":2.0,"memory_mb":4096,"disk_mb":20480,"pids":256,"wall_seconds":2700,"max_stdout_bytes":8388608,"max_stderr_bytes":8388608},"preferred_languages":[],"exclusive_resources":[],"max_attempts":2,"max_model_calls":30,"max_tool_calls":200,"max_usd":1.0}]}
Build a finite acyclic implementation DAG. Every coding task must have executable acceptance checks appropriate to the repository. Keep independent tasks parallelizable. Put integration/testing after implementation and security review after security-sensitive work. Do not invent external credentials or services."#
        .into()
}

fn extract_json(value: &str) -> &str {
    let trimmed = value.trim();
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        &trimmed[start..=end]
    } else {
        trimmed
    }
}

fn strip_fences(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_owned();
    }

    let without_open = trimmed.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
    without_open
        .strip_suffix("```")
        .unwrap_or(without_open)
        .trim_end()
        .to_owned()
}

async fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .await?;

    if !output.status.success() {
        bail!("git failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into())
}
