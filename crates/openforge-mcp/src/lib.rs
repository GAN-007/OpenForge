use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::sync::atomic::{AtomicU64,Ordering};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader,Lines},process::{Child,ChildStdin,ChildStdout,Command}};

pub struct McpStdioClient{child:Child,stdin:ChildStdin,lines:Lines<BufReader<ChildStdout>>,next_id:AtomicU64}
impl McpStdioClient{
    pub async fn spawn(program:&str,args:&[String])->Result<Self>{
        let mut child=Command::new(program).args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn().with_context(||format!("spawn MCP server {program}"))?;
        let stdin=child.stdin.take().context("MCP stdin unavailable")?;let stdout=child.stdout.take().context("MCP stdout unavailable")?;
        Ok(Self{child,stdin,lines:BufReader::new(stdout).lines(),next_id:AtomicU64::new(1)})
    }
    pub async fn initialize(&mut self,client_name:&str,client_version:&str)->Result<Value>{
        let result=self.request("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":client_name,"version":client_version}})).await?;
        self.notify("notifications/initialized",json!({})).await?;Ok(result)
    }
    pub async fn list_tools(&mut self)->Result<Value>{self.request("tools/list",json!({})).await}
    pub async fn call_tool(&mut self,name:&str,arguments:Value)->Result<Value>{self.request("tools/call",json!({"name":name,"arguments":arguments})).await}
    pub async fn notify(&mut self,method:&str,params:Value)->Result<()>{
        let msg=json!({"jsonrpc":"2.0","method":method,"params":params});self.write(&msg).await
    }
    pub async fn request(&mut self,method:&str,params:Value)->Result<Value>{
        let id=self.next_id.fetch_add(1,Ordering::Relaxed);
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
        loop{
            let line=self.lines.next_line().await?.context("MCP server closed stdout")?;
            if line.trim().is_empty(){continue}
            let v:Value=serde_json::from_str(&line).context("invalid MCP JSON")?;
            if v.get("id").and_then(Value::as_u64)==Some(id){
                if let Some(e)=v.get("error"){bail!("MCP error: {e}")}
                return v.get("result").cloned().ok_or_else(||anyhow::anyhow!("MCP response missing result"))
            }
        }
    }
    async fn write(&mut self,v:&Value)->Result<()>{
        self.stdin.write_all(serde_json::to_string(v)?.as_bytes()).await?;self.stdin.write_all(b"\n").await?;self.stdin.flush().await?;Ok(())
    }
    pub async fn shutdown(mut self)->Result<()> {let _=self.child.kill().await;Ok(())}
}
