use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{process::Command, sync::Mutex};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct GitWorkspace {
    pub id: String,
    pub branch: String,
    pub path: PathBuf,
    pub base_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub head_sha: String,
    pub branch: String,
    pub clean: bool,
    pub changed_paths: Vec<String>,
}

#[derive(Clone)]
pub struct GitBroker {
    repo: PathBuf,
    root: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl GitBroker {
    pub async fn open(repo: impl AsRef<Path>, root: impl AsRef<Path>) -> Result<Self> {
        let repo = repo
            .as_ref()
            .canonicalize()
            .context("repository not found")?;
        if git(&repo, &["rev-parse", "--is-inside-work-tree"])
            .await?
            .trim()
            != "true"
        {
            bail!("not a Git working tree");
        }

        let root = if root.as_ref().is_absolute() {
            root.as_ref().to_path_buf()
        } else {
            repo.join(root)
        };
        tokio::fs::create_dir_all(&root).await?;

        Ok(Self {
            repo,
            root,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn head_sha(&self) -> Result<String> {
        Ok(git(&self.repo, &["rev-parse", "HEAD"]).await?.trim().into())
    }

    pub async fn workspace_head(&self, workspace: &GitWorkspace) -> Result<String> {
        Ok(git(&workspace.path, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .into())
    }

    pub async fn create_task_workspace(
        &self,
        task_id: Uuid,
        base_sha: &str,
    ) -> Result<GitWorkspace> {
        self.create_workspace(
            task_id.to_string(),
            format!("of/task/{task_id}"),
            self.root.join("tasks").join(task_id.to_string()),
            base_sha,
        )
        .await
    }

    pub async fn create_integration_workspace(
        &self,
        run_id: Uuid,
        base_sha: &str,
    ) -> Result<GitWorkspace> {
        self.create_workspace(
            format!("integration-{run_id}"),
            format!("of/integration/{run_id}"),
            self.root.join("integration").join(run_id.to_string()),
            base_sha,
        )
        .await
    }

    async fn create_workspace(
        &self,
        id: String,
        branch: String,
        path: PathBuf,
        base_sha: &str,
    ) -> Result<GitWorkspace> {
        let _guard = self.lock.lock().await;
        verify_commit(&self.repo, base_sha).await?;

        if path.exists() {
            bail!("workspace already exists: {}", path.display());
        }
        if branch_exists(&self.repo, &branch).await? {
            bail!("workspace branch already exists: {branch}");
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let path_string = path.to_string_lossy().to_string();
        run_git(
            &self.repo,
            &["worktree", "add", "-b", &branch, &path_string, base_sha],
        )
        .await?;

        if let Err(error) = configure_identity(&path).await {
            let _ = run_git(&self.repo, &["worktree", "remove", "--force", &path_string]).await;
            let _ = run_git(&self.repo, &["branch", "-D", &branch]).await;
            return Err(error);
        }

        Ok(GitWorkspace {
            id,
            branch,
            path,
            base_sha: base_sha.into(),
        })
    }

    pub async fn diff(&self, workspace: &GitWorkspace) -> Result<String> {
        git(&workspace.path, &["diff", "--binary", "HEAD"]).await
    }

    pub async fn status(&self, workspace: &GitWorkspace) -> Result<WorkspaceStatus> {
        let head_sha = self.workspace_head(workspace).await?;
        let porcelain = git(&workspace.path, &["status", "--porcelain=v1"]).await?;
        let mut changed_paths = porcelain
            .lines()
            .filter_map(|line| line.get(3..).map(str::trim))
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        changed_paths.sort();
        changed_paths.dedup();

        Ok(WorkspaceStatus {
            head_sha,
            branch: workspace.branch.clone(),
            clean: changed_paths.is_empty(),
            changed_paths,
        })
    }

    pub async fn commit_all(
        &self,
        workspace: &GitWorkspace,
        message: &str,
    ) -> Result<Option<String>> {
        let _guard = self.lock.lock().await;
        if message.trim().is_empty() {
            bail!("commit message cannot be empty");
        }

        run_git(&workspace.path, &["add", "-A"]).await?;
        let changed = git(&workspace.path, &["status", "--porcelain=v1"]).await?;
        if changed.trim().is_empty() {
            return Ok(None);
        }

        run_git(&workspace.path, &["commit", "--no-gpg-sign", "-m", message]).await?;
        Ok(Some(
            git(&workspace.path, &["rev-parse", "HEAD"])
                .await?
                .trim()
                .into(),
        ))
    }

    pub async fn integrate_commit(
        &self,
        integration: &GitWorkspace,
        commit: &str,
    ) -> Result<String> {
        let _guard = self.lock.lock().await;
        verify_commit(&self.repo, commit).await?;

        let output = Command::new("git")
            .args(["cherry-pick", commit])
            .current_dir(&integration.path)
            .output()
            .await
            .context("run git cherry-pick")?;

        if !output.status.success() {
            let conflicts = git(
                &integration.path,
                &["diff", "--name-only", "--diff-filter=U"],
            )
            .await
            .unwrap_or_default();
            let _ = Command::new("git")
                .args(["cherry-pick", "--abort"])
                .current_dir(&integration.path)
                .output()
                .await;
            bail!(
                "integration conflict for commit {commit}: {}; conflicts={}",
                String::from_utf8_lossy(&output.stderr).trim(),
                conflicts.trim()
            );
        }

        Ok(git(&integration.path, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .into())
    }

    pub async fn reset_hard(
        &self,
        workspace: &GitWorkspace,
        target_sha: &str,
    ) -> Result<()> {
        let _guard = self.lock.lock().await;
        verify_commit(&self.repo, target_sha).await?;
        run_git(&workspace.path, &["reset", "--hard", target_sha]).await?;
        run_git(&workspace.path, &["clean", "-fd"]).await?;
        Ok(())
    }

    pub async fn remove_workspace(&self, workspace: &GitWorkspace) -> Result<()> {
        let _guard = self.lock.lock().await;
        let path = workspace.path.to_string_lossy().to_string();
        run_git(&self.repo, &["worktree", "remove", "--force", &path]).await?;
        run_git(&self.repo, &["worktree", "prune"]).await?;
        Ok(())
    }
}

async fn configure_identity(workspace: &Path) -> Result<()> {
    run_git(workspace, &["config", "user.name", "OpenForge Agent"]).await?;
    run_git(
        workspace,
        &[
            "config",
            "user.email",
            "openforge-agent@users.noreply.github.com",
        ],
    )
    .await?;
    Ok(())
}

async fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .current_dir(repo)
        .status()
        .await?;
    Ok(output.success())
}

async fn verify_commit(repo: &Path, sha: &str) -> Result<()> {
    if sha.trim().is_empty() || sha.contains(char::is_whitespace) {
        bail!("invalid commit identifier");
    }
    let expression = format!("{sha}^{{commit}}");
    run_git(repo, &["cat-file", "-e", &expression])
        .await
        .with_context(|| format!("commit {sha} does not exist"))?;
    Ok(())
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .context("run git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into())
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    git(cwd, args).await.map(|_| ())
}
