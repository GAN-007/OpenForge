use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{timeout, Duration},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationResult {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub base64: String,
    pub url: String,
}

pub struct BrowserClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    request_timeout: Duration,
}

impl BrowserClient {
    pub async fn spawn(
        program: &str,
        args: &[String],
        request_timeout: Duration,
    ) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn browser worker {program}"))?;

        let stdin = child.stdin.take().context("browser worker stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("browser worker stdout unavailable")?;

        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: AtomicU64::new(1),
            request_timeout,
        })
    }

    pub async fn navigate(&mut self, url: &str, timeout_ms: u64) -> Result<NavigationResult> {
        validate_http_url(url)?;
        let value = self
            .request(
                "navigate",
                json!({"url": url, "timeout_ms": timeout_ms}),
            )
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn click(&mut self, selector: &str, timeout_ms: u64) -> Result<String> {
        let value = self
            .request(
                "click",
                json!({"selector": selector, "timeout_ms": timeout_ms}),
            )
            .await?;
        value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("browser click response missing url")
    }

    pub async fn fill(
        &mut self,
        selector: &str,
        value: &str,
        timeout_ms: u64,
    ) -> Result<()> {
        self.request(
            "fill",
            json!({
                "selector": selector,
                "value": value,
                "timeout_ms": timeout_ms
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn text(&mut self, selector: Option<&str>, timeout_ms: u64) -> Result<String> {
        let value = self
            .request(
                "text",
                json!({
                    "selector": selector.unwrap_or("body"),
                    "timeout_ms": timeout_ms
                }),
            )
            .await?;
        value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("browser text response missing text")
    }

    pub async fn html(&mut self, selector: Option<&str>, timeout_ms: u64) -> Result<String> {
        let value = self
            .request(
                "html",
                json!({
                    "selector": selector.unwrap_or("body"),
                    "timeout_ms": timeout_ms
                }),
            )
            .await?;
        value
            .get("html")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("browser html response missing html")
    }

    pub async fn screenshot(&mut self, full_page: bool) -> Result<ScreenshotResult> {
        let value = self
            .request("screenshot", json!({"full_page": full_page}))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn console(&mut self, duration_ms: u64) -> Result<Vec<String>> {
        let value = self
            .request("console", json!({"duration_ms": duration_ms}))
            .await?;
        Ok(serde_json::from_value(
            value
                .get("entries")
                .cloned()
                .context("browser console response missing entries")?,
        )?)
    }

    pub async fn close(mut self) -> Result<()> {
        let _ = self.request("close", json!({})).await;
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
        }
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.stdin
            .write_all(serde_json::to_string(&message)?.as_bytes())
            .await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        timeout(self.request_timeout, async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await?
                    .context("browser worker closed stdout")?;
                if line.trim().is_empty() {
                    continue;
                }

                let response: Value =
                    serde_json::from_str(&line).context("invalid browser worker JSON")?;
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }

                if let Some(error) = response.get("error") {
                    bail!("browser worker error: {error}");
                }
                return response
                    .get("result")
                    .cloned()
                    .context("browser response missing result");
            }
        })
        .await
        .context("browser worker request timed out")?
    }
}

fn validate_http_url(url: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        bail!("browser navigation only permits http:// or https:// URLs");
    }
    if lower.contains('@') && lower.split('@').next().is_some_and(|prefix| prefix.contains("://")) {
        bail!("browser navigation URLs containing userinfo are not permitted");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_non_http_schemes() {
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("javascript:alert(1)").is_err());
        assert!(validate_http_url("https://example.com").is_ok());
    }
}
