use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::sync::atomic::{AtomicU64,Ordering};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader,Lines},process::{Child,ChildStdin,ChildStdout,Command}};

pub struct AcpAgentClient{child:Child,stdin:ChildStdin,lines:Lines<BufReader<ChildStdout>>,next_id:AtomicU64}
impl AcpAgentClient{
    pub async fn spawn(program:&str,args:&[String])->Result<Self>{
        let mut child=Command::new(program).args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn().with_context(||format!("spawn ACP agent {program}"))?;
        let stdin=child.stdin.take().context("ACP stdin unavailable")?;let stdout=child.stdout.take().context("ACP stdout unavailable")?;
        Ok(Self{child,stdin,lines:BufReader::new(stdout).lines(),next_id:AtomicU64::new(1)})
    }
    pub async fn request(&mut self,method:&str,params:Value)->Result<Value>{
        let id=self.next_id.fetch_add(1,Ordering::Relaxed);
        let msg=json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.stdin.write_all(serde_json::to_string(&msg)?.as_bytes()).await?;self.stdin.write_all(b"\n").await?;self.stdin.flush().await?;
        loop{
            let line=self.lines.next_line().await?.context("ACP agent exited")?;
            if line.trim().is_empty(){continue}
            let v:Value=serde_json::from_str(&line).context("invalid ACP JSON")?;
            if v.get("id").and_then(Value::as_u64)==Some(id){
                if let Some(e)=v.get("error"){bail!("ACP error: {e}")}
                return Ok(v.get("result").cloned().unwrap_or(Value::Null))
            }
        }
    }
    pub async fn close(mut self)->Result<()> {let _=self.child.kill().await;Ok(())}
}
