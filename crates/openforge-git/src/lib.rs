use anyhow::{bail,Context,Result};
use std::{path::{Path,PathBuf},sync::Arc};
use tokio::{process::Command,sync::Mutex};
use uuid::Uuid;

#[derive(Debug,Clone)]
pub struct GitWorkspace{pub id:String,pub branch:String,pub path:PathBuf,pub base_sha:String}
#[derive(Clone)]
pub struct GitBroker{repo:PathBuf,root:PathBuf,lock:Arc<Mutex<()>>}

impl GitBroker{
    pub async fn open(repo:impl AsRef<Path>,root:impl AsRef<Path>)->Result<Self>{
        let repo=repo.as_ref().canonicalize().context("repository not found")?;
        if git(&repo,&["rev-parse","--is-inside-work-tree"]).await?.trim()!="true"{bail!("not a Git working tree")}
        let root=if root.as_ref().is_absolute(){root.as_ref().to_path_buf()}else{repo.join(root)};
        tokio::fs::create_dir_all(&root).await?;
        Ok(Self{repo,root,lock:Arc::new(Mutex::new(()))})
    }
    pub async fn head_sha(&self)->Result<String>{Ok(git(&self.repo,&["rev-parse","HEAD"]).await?.trim().into())}
    pub async fn workspace_head(&self,ws:&GitWorkspace)->Result<String>{Ok(git(&ws.path,&["rev-parse","HEAD"]).await?.trim().into())}
    pub async fn create_task_workspace(&self,task_id:Uuid,base_sha:&str)->Result<GitWorkspace>{
        self.create_workspace(task_id.to_string(),format!("of/task/{task_id}"),self.root.join("tasks").join(task_id.to_string()),base_sha).await
    }
    pub async fn create_integration_workspace(&self,run_id:Uuid,base_sha:&str)->Result<GitWorkspace>{
        self.create_workspace(format!("integration-{run_id}"),format!("of/integration/{run_id}"),self.root.join("integration").join(run_id.to_string()),base_sha).await
    }
    async fn create_workspace(&self,id:String,branch:String,path:PathBuf,base_sha:&str)->Result<GitWorkspace>{
        let _guard=self.lock.lock().await;
        if path.exists(){bail!("workspace already exists: {}",path.display())}
        if let Some(parent)=path.parent(){tokio::fs::create_dir_all(parent).await?}
        let p=path.to_string_lossy().to_string();
        run_git(&self.repo,&["worktree","add","-b",&branch,&p,base_sha]).await?;
        Ok(GitWorkspace{id,branch,path,base_sha:base_sha.into()})
    }
    pub async fn diff(&self,ws:&GitWorkspace)->Result<String>{git(&ws.path,&["diff","--binary","HEAD"]).await}
    pub async fn commit_all(&self,ws:&GitWorkspace,message:&str)->Result<Option<String>>{
        let _guard=self.lock.lock().await;
        run_git(&ws.path,&["add","-A"]).await?;
        let changed=git(&ws.path,&["status","--porcelain"]).await?;
        if changed.trim().is_empty(){return Ok(None)}
        run_git(&ws.path,&["commit","-m",message]).await?;
        Ok(Some(git(&ws.path,&["rev-parse","HEAD"]).await?.trim().into()))
    }
    pub async fn integrate_commit(&self,integration:&GitWorkspace,commit:&str)->Result<String>{
        let _guard=self.lock.lock().await;
        let out=Command::new("git").args(["cherry-pick",commit]).current_dir(&integration.path).output().await?;
        if !out.status.success(){
            let _=Command::new("git").args(["cherry-pick","--abort"]).current_dir(&integration.path).output().await;
            bail!("integration conflict for commit {commit}: {}",String::from_utf8_lossy(&out.stderr))
        }
        Ok(git(&integration.path,&["rev-parse","HEAD"]).await?.trim().into())
    }
    pub async fn remove_workspace(&self,ws:&GitWorkspace)->Result<()>{
        let _guard=self.lock.lock().await;
        let p=ws.path.to_string_lossy().to_string();
        run_git(&self.repo,&["worktree","remove","--force",&p]).await?;
        Ok(())
    }
}
async fn git(cwd:&Path,args:&[&str])->Result<String>{let o=Command::new("git").args(args).current_dir(cwd).output().await.context("run git")?;if !o.status.success(){bail!("git failed: {}",String::from_utf8_lossy(&o.stderr))}Ok(String::from_utf8_lossy(&o.stdout).into())}
async fn run_git(cwd:&Path,args:&[&str])->Result<()> {git(cwd,args).await.map(|_|())}
