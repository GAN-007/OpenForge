use anyhow::{bail, Context, Result};
use openforge_models::{ModelProvider, ModelRouter};
use openforge_policy::{AgentPolicy, CapabilityRequest, Decision};
use openforge_protocol::{
    Actor, ChatMessage, ModelRequest, ModelRequirements, TaskNode,
};
use openforge_sandbox::{ExecRequest, SandboxBackend, SandboxLease};
use openforge_store::Store;
use crate::ToolBus;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentAction {
    ReadFile { path: String },
    WriteFile { path: String, content: String },
    Exec {
        argv: Vec<String>,
        cwd: String,
        timeout_seconds: u64,
    },
    McpCall {
        server: String,
        tool: String,
        #[serde(default)]
        arguments: serde_json::Value,
    },
    BrowserNavigate {
        url: String,
        timeout_ms: u64,
    },
    BrowserClick {
        selector: String,
        timeout_ms: u64,
    },
    BrowserFill {
        selector: String,
        value: String,
        timeout_ms: u64,
    },
    BrowserText {
        selector: Option<String>,
        timeout_ms: u64,
    },
    BrowserScreenshot {
        full_page: bool,
    },
    Finish { success: bool, summary: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDecision {
    pub reasoning_summary: String,
    pub action: AgentAction,
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub success: bool,
    pub summary: String,
    pub iterations: u32,
}

pub struct AgentLoop {
    pub store: Store,
    pub provider: Arc<dyn ModelProvider>,
    pub router: ModelRouter,
    pub sandbox: Arc<dyn SandboxBackend>,
    pub tools: Arc<ToolBus>,
    pub policy: AgentPolicy,
    pub max_iterations: u32,
}

impl AgentLoop {
    pub async fn run(
        &self,
        task: &TaskNode,
        workspace: &Path,
        lease: &SandboxLease,
        context: String,
    ) -> Result<AgentOutcome> {
        let mut history = vec![
            ChatMessage {
                role: "system".into(),
                content: system_prompt(&task.role),
            },
            ChatMessage {
                role: "user".into(),
                content: format!(
                    "TASK\n{}\n\nDESCRIPTION\n{}\n\nREPOSITORY CONTEXT\n{}",
                    task.title, task.description, context
                ),
            },
        ];

        let model_call_limit =
            self.max_iterations.min(task.budget.max_model_calls).max(1);
        let mut task_spend = 0.0_f64;
        let mut tool_calls = 0_u32;

        for iteration in 1..=model_call_limit {
            let remaining_budget = (task.budget.max_usd - task_spend).max(0.0);
            if remaining_budget <= f64::EPSILON {
                bail!(
                    "task model budget exhausted after spending {:.6}",
                    task_spend
                );
            }

            let request = ModelRequest {
                invocation_id: Uuid::new_v4(),
                run_id: task.run_id,
                task_id: Some(task.id),
                messages: history.clone(),
                requirements: ModelRequirements {
                    task_class: task.role.clone(),
                    context_tokens: 16_000,
                    requires_tools: false,
                    requires_vision: false,
                    requires_structured_output: true,
                    max_cost_usd: remaining_budget,
                    max_latency_ms: None,
                    data_classification: Default::default(),
                    preferred_model_families: vec![],
                    excluded_model_families: vec![],
                },
                temperature: 0.1,
                max_output_tokens: 3500,
                response_schema: None,
            };

            let model = self.router.select(
                self.provider.catalog(),
                &request.requirements,
                12_000,
                2_000,
            )?;

            let response = self.provider.invoke(model, &request).await?;
            if response.cost_usd > remaining_budget {
                bail!(
                    "model call cost {:.6} exceeded remaining task budget {:.6}",
                    response.cost_usd,
                    remaining_budget
                );
            }
            task_spend += response.cost_usd;

            self.store.record_cost(
                task.run_id,
                Some(task.id),
                Some(&task.role),
                &response.provider,
                &response.model,
                response.cost_usd,
                response.input_tokens,
                response.output_tokens,
            )?;

            self.store.append_event(
                Some(task.run_id),
                Some(task.id),
                Actor {
                    kind: "agent".into(),
                    id: task.role.clone(),
                },
                "model.completed",
                json!({
                    "model": response.model,
                    "provider": response.provider,
                    "cost_usd": response.cost_usd,
                    "task_spend_usd": task_spend,
                    "latency_ms": response.latency_ms
                }),
            )?;

            let decision: AgentDecision =
                serde_json::from_str(extract_json(&response.text))
                    .context("agent returned invalid decision JSON")?;

            let observation = match &decision.action {
                AgentAction::ReadFile { path } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(
                        self.policy
                            .evaluate(CapabilityRequest::ReadPath(path)),
                    )?;
                    let path = safe_existing_path(workspace, path).await?;
                    let data = tokio::fs::read_to_string(&path)
                        .await
                        .with_context(|| format!("read {}", path.display()))?;
                    format!(
                        "READ {}\n{}",
                        path.display(),
                        truncate(&data, 40_000)
                    )
                }
                AgentAction::WriteFile { path, content } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(
                        self.policy
                            .evaluate(CapabilityRequest::WritePath(path)),
                    )?;
                    let path = safe_write_path(workspace, path).await?;
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&path, content).await?;
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "file.changed",
                        json!({
                            "path": path.strip_prefix(workspace).unwrap_or(&path),
                            "bytes": content.len()
                        }),
                    )?;
                    format!(
                        "WROTE {} ({} bytes)",
                        path.display(),
                        content.len()
                    )
                }
                AgentAction::Exec {
                    argv,
                    cwd,
                    timeout_seconds,
                } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(
                        self.policy
                            .evaluate(CapabilityRequest::Process(argv)),
                    )?;
                    let result = self
                        .sandbox
                        .exec(
                            lease,
                            ExecRequest {
                                argv: argv.clone(),
                                cwd: cwd.clone(),
                                environment: Default::default(),
                                timeout_seconds: (*timeout_seconds)
                                    .min(task.budget.max_wall_seconds)
                                    .max(1),
                            },
                        )
                        .await?;

                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({
                            "tool": "sandbox.exec",
                            "argv": argv,
                            "exit_code": result.exit_code,
                            "timed_out": result.timed_out
                        }),
                    )?;

                    format!(
                        "EXIT {}\nSTDOUT\n{}\nSTDERR\n{}",
                        result.exit_code,
                        truncate(&result.stdout, 30_000),
                        truncate(&result.stderr, 30_000)
                    )
                }
                AgentAction::McpCall {
                    server,
                    tool,
                    arguments,
                } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    let subject = format!("{server}/{tool}");
                    require(self.policy.evaluate(CapabilityRequest::Mcp(&subject)))?;
                    let result = self
                        .tools
                        .mcp_call(server, tool, arguments.clone())
                        .await?;
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({
                            "tool": "mcp",
                            "server": server,
                            "method": tool
                        }),
                    )?;
                    format!(
                        "MCP {}\n{}",
                        subject,
                        truncate(&serde_json::to_string_pretty(&result)?, 40_000)
                    )
                }
                AgentAction::BrowserNavigate { url, timeout_ms } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(self.policy.evaluate(CapabilityRequest::Browser(url)))?;
                    require(self.policy.evaluate(CapabilityRequest::Network(url)))?;
                    let result = self.tools.browser_navigate(url, *timeout_ms).await?;
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({"tool": "browser.navigate", "url": url}),
                    )?;
                    truncate(&serde_json::to_string_pretty(&result)?, 20_000)
                }
                AgentAction::BrowserClick {
                    selector,
                    timeout_ms,
                } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(self.policy.evaluate(CapabilityRequest::Browser("click")))?;
                    let result = self.tools.browser_click(selector, *timeout_ms).await?;
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({"tool": "browser.click", "selector": selector}),
                    )?;
                    truncate(&serde_json::to_string_pretty(&result)?, 20_000)
                }
                AgentAction::BrowserFill {
                    selector,
                    value,
                    timeout_ms,
                } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(self.policy.evaluate(CapabilityRequest::Browser("fill")))?;
                    let result = self
                        .tools
                        .browser_fill(selector, value, *timeout_ms)
                        .await?;
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({
                            "tool": "browser.fill",
                            "selector": selector,
                            "value_bytes": value.len()
                        }),
                    )?;
                    serde_json::to_string_pretty(&result)?
                }
                AgentAction::BrowserText {
                    selector,
                    timeout_ms,
                } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(self.policy.evaluate(CapabilityRequest::Browser("text")))?;
                    let result = self
                        .tools
                        .browser_text(selector.as_deref(), *timeout_ms)
                        .await?;
                    truncate(&serde_json::to_string_pretty(&result)?, 40_000)
                }
                AgentAction::BrowserScreenshot { full_page } => {
                    consume_tool_budget(&mut tool_calls, task.budget.max_tool_calls)?;
                    require(self.policy.evaluate(CapabilityRequest::Browser("screenshot")))?;
                    let result = self.tools.browser_screenshot(*full_page).await?;
                    let summary = result
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    self.store.append_event(
                        Some(task.run_id),
                        Some(task.id),
                        Actor {
                            kind: "agent".into(),
                            id: task.role.clone(),
                        },
                        "tool.completed",
                        json!({
                            "tool": "browser.screenshot",
                            "url": summary,
                            "full_page": full_page
                        }),
                    )?;
                    let base64_bytes = result
                        .get("base64")
                        .and_then(serde_json::Value::as_str)
                        .map(str::len)
                        .unwrap_or(0);
                    format!(
                        "BROWSER SCREENSHOT url={} base64_bytes={}",
                        summary, base64_bytes
                    )
                }
                AgentAction::Finish { success, summary } => {
                    return Ok(AgentOutcome {
                        success: *success,
                        summary: summary.clone(),
                        iterations: iteration,
                    });
                }
            };

            history.push(ChatMessage {
                role: "assistant".into(),
                content: serde_json::to_string(&decision)?,
            });
            history.push(ChatMessage {
                role: "user".into(),
                content: format!(
                    "TOOL OBSERVATION\n{observation}\nContinue. Return one JSON AgentDecision."
                ),
            });
        }

        bail!("agent exceeded {model_call_limit} model calls")
    }
}

fn consume_tool_budget(current: &mut u32, maximum: u32) -> Result<()> {
    if *current >= maximum {
        bail!("task tool-call budget exhausted");
    }
    *current += 1;
    Ok(())
}

fn require(decision: Decision) -> Result<()> {
    match decision {
        Decision::Allow => Ok(()),
        Decision::Ask => bail!("action requires approval"),
        Decision::Deny => bail!("action denied by policy"),
    }
}

async fn safe_existing_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative(relative)?;
    let canonical_root = root.canonicalize().context("canonicalize workspace")?;
    let candidate = canonical_root.join(relative);
    let canonical = tokio::fs::canonicalize(&candidate)
        .await
        .with_context(|| format!("canonicalize {}", candidate.display()))?;
    if !canonical.starts_with(&canonical_root) {
        bail!("path escapes workspace: {relative}");
    }
    Ok(canonical)
}

async fn safe_write_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative(relative)?;
    let canonical_root = root.canonicalize().context("canonicalize workspace")?;
    let candidate = canonical_root.join(relative);

    if tokio::fs::try_exists(&candidate).await? {
        let canonical = tokio::fs::canonicalize(&candidate).await?;
        if !canonical.starts_with(&canonical_root) {
            bail!("write target escapes workspace: {relative}");
        }
        return Ok(canonical);
    }

    let mut ancestor = candidate.parent();
    while let Some(path) = ancestor {
        if tokio::fs::try_exists(path).await? {
            let canonical_parent = tokio::fs::canonicalize(path).await?;
            if !canonical_parent.starts_with(&canonical_root) {
                bail!("write parent escapes workspace: {relative}");
            }
            break;
        }
        ancestor = path.parent();
    }

    Ok(candidate)
}

fn validate_relative(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("unsafe workspace path: {relative}");
    }
    Ok(())
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.into();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n...[truncated {} bytes]",
        &value[..end],
        value.len() - end
    )
}

fn extract_json(value: &str) -> &str {
    let trimmed = value.trim();
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        &trimmed[start..=end]
    } else {
        trimmed
    }
}

fn system_prompt(role: &str) -> String {
    format!(
        r#"You are the OpenForge {role} agent operating inside a controlled engineering workspace.
You must make one concrete, verifiable action at a time and return ONLY JSON matching:
{{"reasoning_summary":"brief factual rationale","action":{{"type":"read_file","path":"..."}}}}
{{"reasoning_summary":"...","action":{{"type":"write_file","path":"...","content":"complete file contents"}}}}
{{"reasoning_summary":"...","action":{{"type":"exec","argv":["command","arg"],"cwd":".","timeout_seconds":120}}}}
{{"reasoning_summary":"...","action":{{"type":"mcp_call","server":"configured-server","tool":"tool-name","arguments":{{}}}}}}
{{"reasoning_summary":"...","action":{{"type":"browser_navigate","url":"https://app.example","timeout_ms":30000}}}}
{{"reasoning_summary":"...","action":{{"type":"browser_click","selector":"button[type=submit]","timeout_ms":10000}}}}
{{"reasoning_summary":"...","action":{{"type":"browser_fill","selector":"input[name=email]","value":"...","timeout_ms":10000}}}}
{{"reasoning_summary":"...","action":{{"type":"browser_text","selector":"body","timeout_ms":10000}}}}
{{"reasoning_summary":"...","action":{{"type":"browser_screenshot","full_page":true}}}}
{{"reasoning_summary":"...","action":{{"type":"finish","success":true,"summary":"verified outcome"}}}}
Never request secrets, never access outside the workspace, never claim a test passed unless you executed it and observed success, and do not finish while required acceptance checks are failing."#
    )
}
