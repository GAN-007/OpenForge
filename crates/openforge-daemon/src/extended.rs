use crate::{ensure_repo_access, AppState, AuthContext};
use anyhow::{bail, Context, Result};
use openforge_collab::{ApprovalStatus, ThreadStatus};
use openforge_debugger::DebugPhase;
use openforge_devtools::{
    docker_inspect, docker_inventory, introspect_database, kubernetes_inventory,
    terraform_plan_json, DatabaseConnection,
};
use openforge_edits::EditPredictionInput;
use openforge_knowledge::KnowledgeGraph;
use openforge_memory::MemoryInput;
use openforge_plugins::PluginInvocation;
use openforge_team::TeamRole;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use uuid::Uuid;

pub async fn handle_extended(
    state: &AppState,
    auth: &AuthContext,
    method: &str,
    params: &Value,
) -> Result<Option<Value>> {
    let value = match method {
        "agent/list" => serde_json::to_value(state.engine.agents())?,

        "workspace/file-read" => {
            let repo = repository(state, auth, params)?;
            let path = required_string(params, "path")?;
            let path = safe_child_path(&repo, &path, true)?;
            if !path.is_file() {
                bail!("workspace path is not a file");
            }
            let metadata = tokio::fs::metadata(&path).await?;
            if metadata.len() > 10 * 1024 * 1024 {
                bail!("workspace file exceeds 10 MiB editor limit");
            }
            let bytes = tokio::fs::read(&path).await?;
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                bail!("workspace file is binary");
            }
            let text = String::from_utf8(bytes).context("workspace file is not UTF-8")?;
            json!({
                "path": path.strip_prefix(&repo)?.to_string_lossy().replace('\\', "/"),
                "content": text,
                "bytes": metadata.len()
            })
        }
        "workspace/file-write" => {
            let repo = repository(state, auth, params)?;
            let relative = required_string(params, "path")?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .context("content is required")?;
            if content.len() > 10 * 1024 * 1024 {
                bail!("workspace file exceeds 10 MiB editor limit");
            }
            let path = safe_child_path(&repo, &relative, false)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let temporary = path.with_extension(format!(
                "{}.openforge-tmp-{}",
                path.extension().and_then(|value| value.to_str()).unwrap_or(""),
                Uuid::new_v4().simple()
            ));
            tokio::fs::write(&temporary, content.as_bytes()).await?;
            tokio::fs::rename(&temporary, &path).await?;
            json!({"path": relative, "bytes": content.len()})
        }
        "workspace/file-delete" => {
            let repo = repository(state, auth, params)?;
            let relative = required_string(params, "path")?;
            let path = safe_child_path(&repo, &relative, true)?;
            if path == repo {
                bail!("cannot delete repository root");
            }
            let metadata = tokio::fs::metadata(&path).await?;
            if metadata.is_dir() {
                tokio::fs::remove_dir_all(&path).await?;
            } else {
                tokio::fs::remove_file(&path).await?;
            }
            json!({"deleted": true, "path": relative})
        }
        "workspace/file-rename" => {
            let repo = repository(state, auth, params)?;
            let from = required_string(params, "from")?;
            let to = required_string(params, "to")?;
            let source = safe_child_path(&repo, &from, true)?;
            let target = safe_child_path(&repo, &to, false)?;
            if target.exists() {
                bail!("rename target already exists");
            }
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::rename(source, target).await?;
            json!({"renamed": true, "from": from, "to": to})
        }
        "workspace/directory-create" => {
            let repo = repository(state, auth, params)?;
            let relative = required_string(params, "path")?;
            let path = safe_child_path(&repo, &relative, false)?;
            tokio::fs::create_dir_all(path).await?;
            json!({"created": true, "path": relative})
        }
        "git/status" => {
            let repo = repository(state, auth, params)?;
            let output = run_git(
                &repo,
                &["status", "--porcelain=v2", "--branch", "--untracked-files=all"],
            )
            .await?;
            json!({"porcelain_v2": output})
        }
        "git/diff" => {
            let repo = repository(state, auth, params)?;
            let staged = params
                .get("staged")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let path = optional_string(params, "path");
            let mut arguments = vec!["diff"];
            if staged {
                arguments.push("--cached");
            }
            arguments.push("--");
            if let Some(path) = path.as_deref() {
                validate_git_path(path)?;
                arguments.push(path);
            }
            json!({"diff": run_git(&repo, &arguments).await?})
        }
        "git/stage" => {
            let repo = repository(state, auth, params)?;
            let paths = string_array(params, "paths")?
                .context("paths is required")?;
            if paths.is_empty() {
                bail!("paths cannot be empty");
            }
            let mut arguments = vec!["add", "--"];
            for path in &paths {
                validate_git_path(path)?;
                arguments.push(path);
            }
            run_git(&repo, &arguments).await?;
            json!({"staged": paths})
        }
        "git/unstage" => {
            let repo = repository(state, auth, params)?;
            let paths = string_array(params, "paths")?
                .context("paths is required")?;
            if paths.is_empty() {
                bail!("paths cannot be empty");
            }
            let mut arguments = vec!["restore", "--staged", "--"];
            for path in &paths {
                validate_git_path(path)?;
                arguments.push(path);
            }
            run_git(&repo, &arguments).await?;
            json!({"unstaged": paths})
        }
        "git/commit" => {
            let repo = repository(state, auth, params)?;
            let message = required_string(params, "message")?;
            if message.len() > 5000 {
                bail!("commit message is too long");
            }
            let output = run_git(&repo, &["commit", "-m", &message]).await?;
            json!({"output": output})
        }
        "git/log" => {
            let repo = repository(state, auth, params)?;
            let limit = bounded_usize(params, "limit", 50, 1, 500)?;
            let limit_text = limit.to_string();
            let output = run_git(
                &repo,
                &[
                    "log",
                    &format!("-n{limit_text}"),
                    "--date=iso-strict",
                    "--format=%H%x09%an%x09%ae%x09%aI%x09%s",
                ],
            )
            .await?;
            json!({"log": output})
        }
        "git/branches" => {
            let repo = repository(state, auth, params)?;
            let output = run_git(
                &repo,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)%09%(objectname)%09%(HEAD)",
                    "refs/heads",
                ],
            )
            .await?;
            json!({"branches": output})
        }
        "test/discover" => {
            let repo = repository(state, auth, params)?;
            let graph = KnowledgeGraph::build(&repo)?;
            let tests = graph
                .nodes
                .iter()
                .filter(|node| {
                    let path = node.path.to_ascii_lowercase();
                    let name = node.name.to_ascii_lowercase();
                    path.contains("/tests/")
                        || path.starts_with("tests/")
                        || path.ends_with("_test.py")
                        || path.ends_with(".test.ts")
                        || path.ends_with(".test.tsx")
                        || path.ends_with(".spec.ts")
                        || path.ends_with(".spec.tsx")
                        || name.starts_with("test_")
                        || name.ends_with("_test")
                })
                .cloned()
                .collect::<Vec<_>>();
            serde_json::to_value(tests)?
        }

        "knowledge/build" => {
            let repo = repository(state, auth, params)?;
            serde_json::to_value(KnowledgeGraph::build(repo)?)?
        }
        "knowledge/search" => {
            let repo = repository(state, auth, params)?;
            let query = required_string(params, "query")?;
            let limit = bounded_usize(params, "limit", 25, 1, 500)?;
            let graph = KnowledgeGraph::build(repo)?;
            json!({
                "symbols": graph.find_symbols(&query, limit),
                "files_indexed": graph.files_indexed,
                "nodes": graph.nodes.len(),
                "edges": graph.edges.len()
            })
        }
        "knowledge/neighbors" => {
            let repo = repository(state, auth, params)?;
            let node_id = required_string(params, "node_id")?;
            let depth = bounded_usize(params, "depth", 2, 0, 8)?;
            let graph = KnowledgeGraph::build(repo)?;
            serde_json::to_value(graph.neighbors(&node_id, depth))?
        }

        "edit/predict" => {
            let input: EditPredictionInput =
                serde_json::from_value(params.clone()).context("invalid edit prediction input")?;
            serde_json::to_value(state.engine.predict_edits(input).await?)?
        }

        "lsp/list" => serde_json::to_value(state.services.lsp_configs())?,
        "lsp/start" => {
            let name = required_string(params, "name")?;
            let repo = repository(state, auth, params)?;
            let session_id = state.services.start_lsp(&name, &repo).await?;
            json!({"session_id": session_id})
        }
        "lsp/request" => {
            let session_id = required_uuid(params, "session_id")?;
            let request_method = required_string(params, "request_method")?;
            let request_params = params
                .get("request_params")
                .cloned()
                .unwrap_or_else(|| json!({}));
            state
                .services
                .lsp_request(session_id, &request_method, request_params)
                .await?
        }
        "lsp/notify" => {
            let session_id = required_uuid(params, "session_id")?;
            let notification_method = required_string(params, "notification_method")?;
            let notification_params = params
                .get("notification_params")
                .cloned()
                .unwrap_or_else(|| json!({}));
            state
                .services
                .lsp_notify(session_id, &notification_method, notification_params)
                .await?;
            json!({"ok": true})
        }
        "lsp/notifications" => {
            let session_id = required_uuid(params, "session_id")?;
            serde_json::to_value(state.services.lsp_notifications(session_id).await?)?
        }
        "lsp/stop" => {
            let session_id = required_uuid(params, "session_id")?;
            json!({"stopped": state.services.stop_lsp(session_id).await?})
        }

        "dap/list" => serde_json::to_value(state.services.dap_configs())?,
        "dap/start" => {
            let name = required_string(params, "name")?;
            let repo = repository(state, auth, params)?;
            let (session_id, capabilities) =
                state.services.start_dap(&name, &repo).await?;
            json!({"session_id": session_id, "capabilities": capabilities})
        }
        "dap/request" => {
            let session_id = required_uuid(params, "session_id")?;
            let command = required_string(params, "command")?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            state
                .services
                .dap_request(session_id, &command, arguments)
                .await?
        }
        "dap/events" => {
            let session_id = required_uuid(params, "session_id")?;
            serde_json::to_value(state.services.dap_events(session_id).await?)?
        }
        "dap/stop" => {
            let session_id = required_uuid(params, "session_id")?;
            let terminate_debuggee = params
                .get("terminate_debuggee")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            json!({
                "stopped": state
                    .services
                    .stop_dap(session_id, terminate_debuggee)
                    .await?
            })
        }

        "terminal/spawn" => {
            let repo = repository(state, auth, params)?;
            let program = required_string(params, "program")?;
            let args = string_array(params, "args")?.unwrap_or_default();
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or(".");
            let cwd = safe_child_path(&repo, cwd, true)?;
            let environment = string_map(params, "environment")?;
            serde_json::to_value(
                state
                    .services
                    .terminals
                    .spawn(&program, &args, &cwd, &environment)
                    .await?,
            )?
        }
        "terminal/list" => serde_json::to_value(state.services.terminals.list().await)?,
        "terminal/close" => {
            let id = required_uuid(params, "terminal_id")?;
            json!({"closed": state.services.terminals.close(id).await?})
        }

        "worker/register" => {
            let name = required_string(params, "name")?;
            let endpoint = optional_string(params, "endpoint");
            let capabilities = string_set(params, "capabilities")?;
            let labels = string_set(params, "labels")?;
            serde_json::to_value(state.services.workers.register_worker(
                &name,
                endpoint.as_deref(),
                capabilities,
                labels,
            )?)?
        }
        "worker/get" => {
            let id = required_uuid(params, "worker_id")?;
            serde_json::to_value(state.services.workers.get_worker(id)?)?
        }
        "worker/heartbeat" => {
            let id = required_uuid(params, "worker_id")?;
            json!({"ok": state.services.workers.heartbeat_worker(id)?})
        }
        "worker/submit" => {
            let run_id = required_uuid(params, "run_id")?;
            let task_id = optional_uuid(params, "task_id")?;
            let payload = params
                .get("payload")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let capabilities = string_set(params, "required_capabilities")?;
            let max_attempts = bounded_u32(params, "max_attempts", 3, 1, 100)?;
            serde_json::to_value(state.services.workers.submit_job(
                run_id,
                task_id,
                payload,
                capabilities,
                max_attempts,
            )?)?
        }
        "worker/claim" => {
            let worker_id = required_uuid(params, "worker_id")?;
            let worker = state
                .services
                .workers
                .get_worker(worker_id)?
                .context("worker not found")?;
            let lease_seconds = bounded_u64(params, "lease_seconds", 60, 5, 86_400)?;
            serde_json::to_value(state.services.workers.claim(
                worker_id,
                &worker.capabilities,
                lease_seconds,
            )?)?
        }
        "worker/lease-heartbeat" => {
            let job_id = required_uuid(params, "job_id")?;
            let lease_token = required_string(params, "lease_token")?;
            let lease_seconds = bounded_u64(params, "lease_seconds", 60, 5, 86_400)?;
            json!({
                "ok": state.services.workers.heartbeat_lease(
                    job_id,
                    &lease_token,
                    lease_seconds
                )?
            })
        }
        "worker/checkpoint" => {
            let job_id = required_uuid(params, "job_id")?;
            let lease_token = required_string(params, "lease_token")?;
            let sequence = params
                .get("sequence")
                .and_then(Value::as_i64)
                .context("sequence is required")?;
            let checkpoint = params
                .get("state")
                .cloned()
                .context("state is required")?;
            serde_json::to_value(state.services.workers.checkpoint(
                job_id,
                &lease_token,
                sequence,
                &checkpoint,
            )?)?
        }
        "worker/checkpoint-latest" => {
            let job_id = required_uuid(params, "job_id")?;
            serde_json::to_value(state.services.workers.latest_checkpoint(job_id)?)?
        }
        "worker/complete" => {
            let job_id = required_uuid(params, "job_id")?;
            let lease_token = required_string(params, "lease_token")?;
            let result = params
                .get("result")
                .cloned()
                .unwrap_or_else(|| json!({}));
            json!({
                "ok": state
                    .services
                    .workers
                    .complete(job_id, &lease_token, &result)?
            })
        }
        "worker/fail" => {
            let job_id = required_uuid(params, "job_id")?;
            let lease_token = required_string(params, "lease_token")?;
            let error = required_string(params, "error")?;
            json!({
                "ok": state
                    .services
                    .workers
                    .fail(job_id, &lease_token, &error)?
            })
        }
        "worker/cancel" => {
            let job_id = required_uuid(params, "job_id")?;
            json!({"cancelled": state.services.workers.cancel(job_id)?})
        }
        "worker/requeue-expired" => {
            json!({"count": state.services.workers.requeue_expired()?})
        }

        "memory/rich-put" => {
            let input: MemoryInput =
                serde_json::from_value(params.clone()).context("invalid rich memory input")?;
            serde_json::to_value(state.services.memory.put(input)?)?
        }
        "memory/rich-search" => {
            let scope = params.get("scope").and_then(Value::as_str);
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            let embedding = params
                .get("query_embedding")
                .map(|value| serde_json::from_value::<Vec<f32>>(value.clone()))
                .transpose()
                .context("query_embedding must be a float array")?;
            let fingerprint = params
                .get("repository_fingerprint")
                .and_then(Value::as_str);
            let limit = bounded_usize(params, "limit", 50, 1, 1000)?;
            serde_json::to_value(state.services.memory.search(
                scope,
                query,
                embedding.as_deref(),
                fingerprint,
                limit,
            )?)?
        }
        "memory/rich-delete" => {
            let scope = required_string(params, "scope")?;
            let key = required_string(params, "key")?;
            json!({"deleted": state.services.memory.delete(&scope, &key)?})
        }
        "memory/purge-expired" => {
            json!({"deleted": state.services.memory.purge_expired()?})
        }
        "memory/invalidate-repository" => {
            let fingerprint = required_string(params, "repository_fingerprint")?;
            json!({
                "updated": state
                    .services
                    .memory
                    .invalidate_repository_fingerprint(&fingerprint)?
            })
        }

        "team/identity-upsert" => {
            let provider = required_string(params, "provider")?;
            let subject = required_string(params, "subject")?;
            let email = optional_string(params, "email");
            let display_name = optional_string(params, "display_name");
            serde_json::to_value(state.services.team.upsert_identity(
                &provider,
                &subject,
                email.as_deref(),
                display_name.as_deref(),
            )?)?
        }
        "team/workspace-create" => {
            let slug = required_string(params, "slug")?;
            let name = required_string(params, "name")?;
            serde_json::to_value(state.services.team.create_workspace(&slug, &name)?)?
        }
        "team/membership-set" => {
            let workspace_id = required_uuid(params, "workspace_id")?;
            let identity_id = required_uuid(params, "identity_id")?;
            let role: TeamRole = serde_json::from_value(
                params
                    .get("role")
                    .cloned()
                    .context("role is required")?,
            )
            .context("invalid team role")?;
            state
                .services
                .team
                .set_membership(workspace_id, identity_id, role)?;
            json!({"ok": true})
        }
        "team/token-issue" => {
            let workspace_id = required_uuid(params, "workspace_id")?;
            let identity_id = required_uuid(params, "identity_id")?;
            let label = required_string(params, "label")?;
            let expires_at = params
                .get("expires_at")
                .and_then(Value::as_str)
                .map(|value| {
                    chrono::DateTime::parse_from_rfc3339(value)
                        .map(|date| date.with_timezone(&chrono::Utc))
                })
                .transpose()
                .context("expires_at must be RFC3339")?;
            let token = state.services.team.issue_token(
                workspace_id,
                identity_id,
                &label,
                expires_at,
            )?;
            json!({"token": token})
        }
        "team/token-revoke" => {
            let token = required_string(params, "token")?;
            json!({"revoked": state.services.team.revoke_token(&token)?})
        }
        "team/repository-register" => {
            let workspace_id = required_uuid(params, "workspace_id")?;
            let name = required_string(params, "name")?;
            let path = required_string(params, "path")?;
            serde_json::to_value(
                state
                    .services
                    .team
                    .register_repository(workspace_id, &name, path)?,
            )?
        }
        "team/repository-list" => {
            let workspace_id = params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(Uuid::parse_str)
                .transpose()?
                .or_else(|| auth.workspace_id())
                .context("workspace_id is required")?;
            serde_json::to_value(state.services.team.repositories(workspace_id)?)?
        }

        "thread/create" => {
            let run_id = required_uuid(params, "run_id")?;
            let task_id = optional_uuid(params, "task_id")?;
            let title = required_string(params, "title")?;
            serde_json::to_value(state.services.collaboration.create_thread(
                run_id,
                task_id,
                &title,
                Some(&auth.subject),
            )?)?
        }
        "thread/status" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let status: ThreadStatus = serde_json::from_value(
                params
                    .get("status")
                    .cloned()
                    .context("status is required")?,
            )
            .context("invalid thread status")?;
            json!({
                "updated": state
                    .services
                    .collaboration
                    .set_thread_status(thread_id, status)?
            })
        }
        "thread/message" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let author_kind = params
                .get("author_kind")
                .and_then(Value::as_str)
                .unwrap_or("user");
            let message_type = params
                .get("message_type")
                .and_then(Value::as_str)
                .unwrap_or("message");
            let content = params
                .get("content")
                .cloned()
                .context("content is required")?;
            serde_json::to_value(state.services.collaboration.append_message(
                thread_id,
                author_kind,
                &auth.subject,
                message_type,
                &content,
            )?)?
        }
        "thread/messages" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let after = params
                .get("after_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let limit = bounded_usize(params, "limit", 500, 1, 5000)?;
            serde_json::to_value(
                state
                    .services
                    .collaboration
                    .messages(thread_id, after, limit)?,
            )?
        }
        "thread/instruction-queue" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let content = required_string(params, "content")?;
            serde_json::to_value(state.services.collaboration.queue_instruction(
                thread_id,
                &content,
                &auth.subject,
            )?)?
        }
        "thread/instruction-consume" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let maximum = bounded_usize(params, "maximum", 100, 1, 1000)?;
            serde_json::to_value(
                state
                    .services
                    .collaboration
                    .consume_instructions(thread_id, maximum)?,
            )?
        }
        "approval/request" => {
            let thread_id = required_uuid(params, "thread_id")?;
            let capability = required_string(params, "capability")?;
            let subject = required_string(params, "subject")?;
            let reason = required_string(params, "reason")?;
            serde_json::to_value(state.services.collaboration.request_approval(
                thread_id,
                &capability,
                &subject,
                &reason,
            )?)?
        }
        "approval/decide" => {
            let id = required_uuid(params, "approval_id")?;
            let status: ApprovalStatus = serde_json::from_value(
                params
                    .get("status")
                    .cloned()
                    .context("status is required")?,
            )
            .context("invalid approval status")?;
            json!({
                "updated": state.services.collaboration.decide_approval(
                    id,
                    status,
                    &auth.subject
                )?
            })
        }
        "presence/heartbeat" => {
            let workspace_id = auth
                .workspace_id()
                .or(optional_uuid(params, "workspace_id")?)
                .context("workspace_id is required")?;
            let surface = required_string(params, "surface")?;
            let resource = optional_string(params, "resource");
            serde_json::to_value(state.services.collaboration.heartbeat_presence(
                workspace_id,
                &auth.subject,
                &surface,
                resource.as_deref(),
            )?)?
        }
        "presence/list" => {
            let workspace_id = auth
                .workspace_id()
                .or(optional_uuid(params, "workspace_id")?)
                .context("workspace_id is required")?;
            let within_seconds = params
                .get("within_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(60)
                .clamp(5, 3600);
            serde_json::to_value(
                state
                    .services
                    .collaboration
                    .active_presence(workspace_id, within_seconds)?,
            )?
        }
        "comment/add" => {
            let workspace_id = auth
                .workspace_id()
                .or(optional_uuid(params, "workspace_id")?)
                .context("workspace_id is required")?;
            let run_id = optional_uuid(params, "run_id")?;
            let task_id = optional_uuid(params, "task_id")?;
            let file_path = optional_string(params, "file_path");
            let line = params
                .get("line")
                .and_then(Value::as_u64)
                .map(|value| u32::try_from(value).context("line exceeds u32"))
                .transpose()?;
            let body = required_string(params, "body")?;
            serde_json::to_value(state.services.collaboration.add_comment(
                workspace_id,
                run_id,
                task_id,
                file_path.as_deref(),
                line,
                &auth.subject,
                &body,
            )?)?
        }
        "comment/resolve" => {
            let id = required_uuid(params, "comment_id")?;
            json!({
                "resolved": state.services.collaboration.resolve_comment(id)?
            })
        }

        "plugin/load" => {
            let root = required_string(params, "root")?;
            serde_json::to_value(state.services.plugins.load(root).await?)?
        }
        "plugin/list" => serde_json::to_value(state.services.plugins.list().await)?,
        "plugin/unload" => {
            let id = required_string(params, "id")?;
            json!({"unloaded": state.services.plugins.unload(&id).await})
        }
        "plugin/invoke" => {
            let id = required_string(params, "id")?;
            let invocation: PluginInvocation = serde_json::from_value(
                params
                    .get("invocation")
                    .cloned()
                    .context("invocation is required")?,
            )
            .context("invalid plugin invocation")?;
            state.services.plugins.invoke(&id, invocation).await?
        }

        "database/introspect" => {
            let repo = repository(state, auth, params)?;
            let connection: DatabaseConnection = serde_json::from_value(
                params
                    .get("connection")
                    .cloned()
                    .context("connection is required")?,
            )
            .context("invalid database connection")?;
            introspect_database(&connection, repo).await?
        }
        "devops/docker-inventory" => {
            let repo = repository(state, auth, params)?;
            docker_inventory(repo).await?
        }
        "devops/docker-inspect" => {
            let repo = repository(state, auth, params)?;
            let object = required_string(params, "object")?;
            docker_inspect(repo, &object).await?
        }
        "devops/kubernetes-inventory" => {
            let repo = repository(state, auth, params)?;
            let namespace = optional_string(params, "namespace");
            kubernetes_inventory(repo, namespace.as_deref()).await?
        }
        "devops/terraform-plan" => {
            let repo = repository(state, auth, params)?;
            let files = string_array(params, "variable_files")?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            terraform_plan_json(repo, &files).await?
        }

        "debug/create" => {
            let run_id = required_uuid(params, "run_id")?;
            let task_id = optional_uuid(params, "task_id")?;
            let issue = required_string(params, "issue")?;
            serde_json::to_value(
                state
                    .services
                    .create_debug_session(run_id, task_id, issue)
                    .await?,
            )?
        }
        "debug/get" => {
            let id = required_uuid(params, "debug_id")?;
            serde_json::to_value(state.services.get_debug_session(id).await?)?
        }
        "debug/advance" => {
            let id = required_uuid(params, "debug_id")?;
            let phase: DebugPhase = serde_json::from_value(
                params
                    .get("phase")
                    .cloned()
                    .context("phase is required")?,
            )
            .context("invalid debug phase")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.advance(phase)?;
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/hypothesis-add" => {
            let id = required_uuid(params, "debug_id")?;
            let statement = required_string(params, "statement")?;
            let confidence = params
                .get("confidence")
                .and_then(Value::as_f64)
                .context("confidence is required")? as f32;
            let mut session = state.services.get_debug_session(id).await?;
            let hypothesis_id = session.add_hypothesis(statement, confidence)?;
            state.services.update_debug_session(session).await?;
            json!({"hypothesis_id": hypothesis_id})
        }
        "debug/observation-add" => {
            let id = required_uuid(params, "debug_id")?;
            let kind = required_string(params, "kind")?;
            let source = required_string(params, "source")?;
            let data = params
                .get("data")
                .cloned()
                .unwrap_or(Value::Null);
            let mut session = state.services.get_debug_session(id).await?;
            let observation_id = session.record_observation(kind, source, data);
            state.services.update_debug_session(session).await?;
            json!({"observation_id": observation_id})
        }
        "debug/evidence-attach" => {
            let id = required_uuid(params, "debug_id")?;
            let hypothesis_id = required_uuid(params, "hypothesis_id")?;
            let observation_id = required_uuid(params, "observation_id")?;
            let supports = params
                .get("supports")
                .and_then(Value::as_bool)
                .context("supports is required")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.attach_evidence(hypothesis_id, observation_id, supports)?;
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/hypotheses-rank" => {
            let id = required_uuid(params, "debug_id")?;
            serde_json::to_value(
                state
                    .services
                    .get_debug_session(id)
                    .await?
                    .ranked_hypotheses(),
            )?
        }
        "debug/hypothesis-select" => {
            let id = required_uuid(params, "debug_id")?;
            let hypothesis_id = required_uuid(params, "hypothesis_id")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.select_hypothesis(hypothesis_id)?;
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/instrumentation-add" => {
            let id = required_uuid(params, "debug_id")?;
            let description = required_string(params, "description")?;
            let file = required_string(params, "file")?;
            let reversible_patch = required_string(params, "reversible_patch")?;
            let mut session = state.services.get_debug_session(id).await?;
            let instrumentation_id =
                session.add_instrumentation(description, file, reversible_patch)?;
            state.services.update_debug_session(session).await?;
            json!({"instrumentation_id": instrumentation_id})
        }
        "debug/instrumentation-applied" => {
            let id = required_uuid(params, "debug_id")?;
            let instrumentation_id = required_uuid(params, "instrumentation_id")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.mark_instrumentation_applied(instrumentation_id)?;
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/instrumentation-removed" => {
            let id = required_uuid(params, "debug_id")?;
            let instrumentation_id = required_uuid(params, "instrumentation_id")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.mark_instrumentation_removed(instrumentation_id)?;
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/fix-summary" => {
            let id = required_uuid(params, "debug_id")?;
            let summary = required_string(params, "summary")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.fix_summary = Some(summary);
            serde_json::to_value(state.services.update_debug_session(session).await?)?
        }
        "debug/verification-summary" => {
            let id = required_uuid(params, "debug_id")?;
            let summary = required_string(params, "summary")?;
            let mut session = state.services.get_debug_session(id).await?;
            session.verification_summary = Some(summary);
            let can_complete = session.can_complete();
            let session = state.services.update_debug_session(session).await?;
            json!({"session": session, "can_complete": can_complete})
        }

        _ => return Ok(None),
    };

    Ok(Some(value))
}

async fn run_git(repo: &std::path::Path, arguments: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(arguments)
        .current_dir(repo)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("execute git command")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            arguments,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn validate_git_path(value: &str) -> Result<()> {
    let path = std::path::Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        bail!("invalid repository-relative Git path");
    }
    Ok(())
}

fn repository(state: &AppState, auth: &AuthContext, params: &Value) -> Result<PathBuf> {
    let repo = required_string(params, "repo")?;
    ensure_repo_access(state, auth, &repo)
}

fn safe_child_path(root: &std::path::Path, relative: &str, must_exist: bool) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .context("repository root is unavailable")?;
    let candidate = root.join(relative);
    let resolved = if must_exist {
        candidate
            .canonicalize()
            .with_context(|| format!("path {} unavailable", candidate.display()))?
    } else if candidate.exists() {
        candidate.canonicalize()?
    } else {
        let parent = candidate
            .parent()
            .context("path has no parent")?
            .canonicalize()?;
        if !parent.starts_with(&root) {
            bail!("path escapes repository");
        }
        candidate
    };
    if !resolved.starts_with(&root) {
        bail!("path escapes repository");
    }
    Ok(resolved)
}

fn required_string(params: &Value, field: &str) -> Result<String> {
    let value = params
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("{field} is required"))?
        .trim();
    if value.is_empty() {
        bail!("{field} cannot be empty");
    }
    Ok(value.to_string())
}

fn optional_string(params: &Value, field: &str) -> Option<String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_uuid(params: &Value, field: &str) -> Result<Uuid> {
    Ok(Uuid::parse_str(&required_string(params, field)?)?)
}

fn optional_uuid(params: &Value, field: &str) -> Result<Option<Uuid>> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(Into::into)
}

fn string_array(params: &Value, field: &str) -> Result<Option<Vec<String>>> {
    params
        .get(field)
        .map(|value| {
            value
                .as_array()
                .with_context(|| format!("{field} must be an array"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .with_context(|| format!("{field} must contain only strings"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()
}

fn string_set(params: &Value, field: &str) -> Result<BTreeSet<String>> {
    Ok(string_array(params, field)?
        .unwrap_or_default()
        .into_iter()
        .collect())
}

fn string_map(params: &Value, field: &str) -> Result<BTreeMap<String, String>> {
    params
        .get(field)
        .map(|value| {
            let object = value
                .as_object()
                .with_context(|| format!("{field} must be an object"))?;
            object
                .iter()
                .map(|(key, value)| {
                    let value = value
                        .as_str()
                        .with_context(|| format!("{field}.{key} must be a string"))?;
                    Ok((key.clone(), value.to_string()))
                })
                .collect::<Result<BTreeMap<_, _>>>()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn bounded_usize(
    params: &Value,
    field: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize> {
    let raw = params
        .get(field)
        .and_then(Value::as_u64)
        .unwrap_or(default as u64);
    let value = usize::try_from(raw).context("value exceeds usize")?;
    if !(minimum..=maximum).contains(&value) {
        bail!("{field} must be between {minimum} and {maximum}");
    }
    Ok(value)
}

fn bounded_u32(
    params: &Value,
    field: &str,
    default: u32,
    minimum: u32,
    maximum: u32,
) -> Result<u32> {
    let raw = params
        .get(field)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(default));
    let value = u32::try_from(raw).context("value exceeds u32")?;
    if !(minimum..=maximum).contains(&value) {
        bail!("{field} must be between {minimum} and {maximum}");
    }
    Ok(value)
}

fn bounded_u64(
    params: &Value,
    field: &str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64> {
    let value = params
        .get(field)
        .and_then(Value::as_u64)
        .unwrap_or(default);
    if !(minimum..=maximum).contains(&value) {
        bail!("{field} must be between {minimum} and {maximum}");
    }
    Ok(value)
}
