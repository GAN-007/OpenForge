use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize,Serialize};
use std::{collections::BTreeMap,path::{Path,PathBuf},process::Stdio};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct SandboxPolicy{
    pub cpus:f32,
    pub memory_mb:u64,
    pub pids_limit:u32,
    pub network_enabled:bool,
    #[serde(default)]
    pub environment:BTreeMap<String,String>,
}
impl Default for SandboxPolicy{
    fn default()->Self{Self{cpus:2.0,memory_mb:4096,pids_limit:256,network_enabled:false,environment:BTreeMap::new()}}
}
#[derive(Debug,Clone)]
pub struct SandboxLease{
    pub id:Uuid,
    pub backend:String,
    pub workspace:PathBuf,
    pub policy:SandboxPolicy,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ExecRequest{
    pub argv:Vec<String>,
    pub cwd:String,
    #[serde(default)]
    pub environment:BTreeMap<String,String>,
    pub timeout_seconds:u64,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ExecResult{
    pub exit_code:i32,
    pub stdout:String,
    pub stderr:String,
    pub timed_out:bool,
}

#[async_trait]
pub trait SandboxBackend:Send+Sync{
    fn name(&self)->&str;
    async fn create(&self,workspace:&Path,policy:SandboxPolicy)->Result<SandboxLease>;
    async fn exec(&self,lease:&SandboxLease,req:ExecRequest)->Result<ExecResult>;
    async fn destroy(&self,lease:SandboxLease)->Result<()>;
}

pub struct LocalProcessBackend;
#[async_trait]
impl SandboxBackend for LocalProcessBackend{
    fn name(&self)->&str{"local-process"}
    async fn create(&self,workspace:&Path,policy:SandboxPolicy)->Result<SandboxLease>{
        let canonical=workspace.canonicalize().context("workspace does not exist")?;
        Ok(SandboxLease{id:Uuid::new_v4(),backend:self.name().into(),workspace:canonical,policy})
    }
    async fn exec(&self,lease:&SandboxLease,req:ExecRequest)->Result<ExecResult>{
        if req.argv.is_empty(){bail!("empty argv")}
        let cwd=lease.workspace.join(&req.cwd);
        let canonical=cwd.canonicalize().context("invalid cwd")?;
        if !canonical.starts_with(&lease.workspace){bail!("cwd escapes workspace")}
        let mut cmd=Command::new(&req.argv[0]);
        cmd.args(&req.argv[1..]).current_dir(canonical).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.env_clear();
        for (k,v) in std::env::vars().filter(|(k,_)|["PATH","HOME","LANG","LC_ALL","TMPDIR"].contains(&k.as_str())){cmd.env(k,v);}
        for (k,v) in &lease.policy.environment{cmd.env(k,v);}
        for (k,v) in req.environment{cmd.env(k,v);}
        let duration=std::time::Duration::from_secs(req.timeout_seconds.max(1));
        match tokio::time::timeout(duration,cmd.output()).await{
            Ok(output)=>{let o=output?;Ok(ExecResult{exit_code:o.status.code().unwrap_or(-1),stdout:String::from_utf8_lossy(&o.stdout).into(),stderr:String::from_utf8_lossy(&o.stderr).into(),timed_out:false})}
            Err(_)=>Ok(ExecResult{exit_code:-1,stdout:String::new(),stderr:"command exceeded timeout".into(),timed_out:true})
        }
    }
    async fn destroy(&self,_lease:SandboxLease)->Result<()>{Ok(())}
}

pub struct DockerBackend{pub image:String}
#[async_trait]
impl SandboxBackend for DockerBackend{
    fn name(&self)->&str{"docker"}
    async fn create(&self,workspace:&Path,policy:SandboxPolicy)->Result<SandboxLease>{
        let canonical=workspace.canonicalize().context("workspace does not exist")?;
        let status=Command::new("docker").args(["version","--format","{{.Server.Version}}"]).status().await.context("docker executable unavailable")?;
        if !status.success(){bail!("docker daemon unavailable")}
        Ok(SandboxLease{id:Uuid::new_v4(),backend:self.name().into(),workspace:canonical,policy})
    }
    async fn exec(&self,lease:&SandboxLease,req:ExecRequest)->Result<ExecResult>{
        if req.argv.is_empty(){bail!("empty argv")}
        let mount=format!("{}:/workspace:rw",lease.workspace.display());
        let mut argv=vec![
            "run".to_string(),"--rm".into(),"--init".into(),
            "--cpus".into(),lease.policy.cpus.to_string(),
            "--memory".into(),format!("{}m",lease.policy.memory_mb),
            "--pids-limit".into(),lease.policy.pids_limit.to_string(),
            "-v".into(),mount,
            "-w".into(),format!("/workspace/{}",req.cwd.trim_start_matches('/')),
        ];
        if !lease.policy.network_enabled{argv.push("--network".into());argv.push("none".into());}
        for (k,v) in lease.policy.environment.iter().chain(req.environment.iter()){
            argv.push("-e".into());argv.push(format!("{k}={v}"));
        }
        argv.push(self.image.clone());
        argv.extend(req.argv);
        let local=SandboxLease{id:lease.id,backend:"docker-command".into(),workspace:PathBuf::from("/"),policy:lease.policy.clone()};
        LocalProcessBackend.exec(&local,ExecRequest{argv:[vec!["docker".into()],argv].concat(),cwd:".".into(),environment:BTreeMap::new(),timeout_seconds:req.timeout_seconds}).await
    }
    async fn destroy(&self,_lease:SandboxLease)->Result<()>{Ok(())}
}
