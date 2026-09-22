use crate::config::{BrowserWorkerConfig, McpServerConfig};
use anyhow::{Context, Result};
use openforge_browser::BrowserClient;
use openforge_mcp::{McpProcessConfig, McpStdioClient};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tokio::sync::Mutex;

pub struct ToolBus {
    mcp_servers: HashMap<String, McpServerConfig>,
    browser_config: BrowserWorkerConfig,
    browser: Mutex<Option<BrowserClient>>,
}

impl ToolBus {
    pub fn new(
        mcp_servers: Vec<McpServerConfig>,
        browser_config: BrowserWorkerConfig,
    ) -> Result<Self> {
        let mut servers = HashMap::new();
        for server in mcp_servers {
            if server.name.trim().is_empty() {
                anyhow::bail!("MCP server name cannot be empty");
            }
            if server.program.trim().is_empty() {
                anyhow::bail!("MCP server {} has empty program", server.name);
            }
            if servers.insert(server.name.clone(), server).is_some() {
                anyhow::bail!("duplicate MCP server name");
            }
        }

        Ok(Self {
            mcp_servers: servers,
            browser_config,
            browser: Mutex::new(None),
        })
    }

    pub fn has_mcp_server(&self, name: &str) -> bool {
        self.mcp_servers.contains_key(name)
    }

    pub async fn mcp_catalog(&self, policy: &openforge_policy::AgentPolicy) -> Value {
        let mut catalog = Vec::new();
        let mut servers: Vec<_> = self.mcp_servers.values().collect();
        servers.sort_by_key(|server| &server.name);
        for server in servers {
            let subject = format!("{}/tools/list", server.name);
            if policy.evaluate(openforge_policy::CapabilityRequest::Mcp(&subject))
                != openforge_policy::Decision::Allow
            {
                continue;
            }
            let result = async {
                let mut config = McpProcessConfig::new(server.program.clone(), server.args.clone());
                config.cwd = server.cwd.as_ref().map(PathBuf::from);
                config.environment = server.environment.clone();
                config.request_timeout = Duration::from_secs(server.timeout_seconds.clamp(1, 30));
                config.max_response_bytes = server.max_response_bytes.clamp(1024, 256 * 1024);
                let mut client = McpStdioClient::spawn_with_config(config).await?;
                let result = async {
                    client
                        .initialize("openforge", env!("CARGO_PKG_VERSION"))
                        .await?;
                    client.list_tools().await
                }
                .await;
                let _ = client.shutdown().await;
                result
            }
            .await;
            match result {
                Ok(tools) => {
                    for tool in tools {
                        let subject = format!("{}/{}", server.name, tool.name);
                        if policy.evaluate(openforge_policy::CapabilityRequest::Mcp(&subject))
                            == openforge_policy::Decision::Allow
                        {
                            catalog.push(json!({"server": server.name, "tool": tool}));
                        }
                    }
                }
                Err(_) => catalog
                    .push(json!({"server": server.name, "error": "Tool discovery unavailable"})),
            }
        }
        json!(catalog)
    }

    pub async fn mcp_call(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value> {
        let server = self
            .mcp_servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server {server_name}"))?;

        let mut config = McpProcessConfig::new(server.program.clone(), server.args.clone());
        config.cwd = server.cwd.as_ref().map(PathBuf::from);
        config.environment = server.environment.clone();
        config.request_timeout = Duration::from_secs(server.timeout_seconds.max(1));
        config.max_response_bytes = server.max_response_bytes.max(1024);

        let mut client = McpStdioClient::spawn_with_config(config).await?;
        let result = async {
            client
                .initialize("openforge", env!("CARGO_PKG_VERSION"))
                .await?;
            let response = client.call_tool(tool_name, arguments).await?;
            Ok::<Value, anyhow::Error>(serde_json::to_value(response)?)
        }
        .await;

        let shutdown = client.shutdown().await;
        match (result, shutdown) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.context("MCP shutdown failed")),
            (Err(error), _) => Err(error),
        }
    }

    pub async fn browser_navigate(&self, url: &str, timeout_ms: u64) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        let client = self.browser_client(&mut guard).await?;
        Ok(serde_json::to_value(
            client.navigate(url, timeout_ms).await?,
        )?)
    }

    pub async fn browser_click(&self, selector: &str, timeout_ms: u64) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        let client = self.browser_client(&mut guard).await?;
        let url = client.click(selector, timeout_ms).await?;
        Ok(json!({"url": url}))
    }

    pub async fn browser_fill(
        &self,
        selector: &str,
        value: &str,
        timeout_ms: u64,
    ) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        let client = self.browser_client(&mut guard).await?;
        client.fill(selector, value, timeout_ms).await?;
        Ok(json!({"ok": true}))
    }

    pub async fn browser_text(&self, selector: Option<&str>, timeout_ms: u64) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        let client = self.browser_client(&mut guard).await?;
        let text = client.text(selector, timeout_ms).await?;
        Ok(json!({"text": text}))
    }

    pub async fn browser_screenshot(&self, full_page: bool) -> Result<Value> {
        let mut guard = self.browser.lock().await;
        let client = self.browser_client(&mut guard).await?;
        Ok(serde_json::to_value(client.screenshot(full_page).await?)?)
    }

    pub async fn close(&self) -> Result<()> {
        let mut guard = self.browser.lock().await;
        if let Some(client) = guard.take() {
            client.close().await?;
        }
        Ok(())
    }

    async fn browser_client<'a>(
        &'a self,
        slot: &'a mut Option<BrowserClient>,
    ) -> Result<&'a mut BrowserClient> {
        if slot.is_none() {
            let timeout = Duration::from_secs(self.browser_config.timeout_seconds.max(1));
            *slot = Some(
                BrowserClient::spawn(
                    &self.browser_config.program,
                    &self.browser_config.args,
                    timeout,
                )
                .await
                .context("start browser worker")?,
            );
        }
        Ok(slot.as_mut().expect("browser initialized"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openforge_policy::AgentPolicy;

    #[tokio::test]
    async fn bundled_github_discovery_exposes_only_policy_allowed_tools() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let server = McpServerConfig {
            name: "github".into(),
            program: "python3".into(),
            args: vec![
                root.join("plugins/builtin/github/server.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            cwd: None,
            environment: Default::default(),
            timeout_seconds: 5,
            max_response_bytes: 65536,
        };
        let bus = ToolBus::new(vec![server], BrowserWorkerConfig::default()).unwrap();
        let policy = AgentPolicy::from_yaml(root.join("config/policies/development.yaml")).unwrap();
        let catalog = bus.mcp_catalog(&policy).await;
        let tools = catalog.as_array().unwrap();
        assert_eq!(tools.len(), 4);
        assert!(
            tools
                .iter()
                .any(|entry| entry["tool"]["name"] == "get_repository")
        );
        assert!(
            !tools
                .iter()
                .any(|entry| entry["tool"]["name"] == "create_issue")
        );
    }
}
