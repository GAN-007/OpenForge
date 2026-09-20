use anyhow::{Context, Result};
use globset::Glob;
use openforge_protocol::AutonomyLevel;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternRules {
    #[serde(default = "default_ask")]
    pub default: Decision,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

fn default_ask() -> Decision {
    Decision::Ask
}

impl Default for PatternRules {
    fn default() -> Self {
        Self {
            default: Decision::Ask,
            allow: Vec::new(),
            ask: Vec::new(),
            deny: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemPolicy {
    #[serde(default)]
    pub read: PatternRules,
    #[serde(default)]
    pub write: PatternRules,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentPolicy {
    #[serde(default)]
    pub autonomy: AutonomyLevel,
    #[serde(default)]
    pub filesystem: FilesystemPolicy,
    #[serde(default)]
    pub process: PatternRules,
    #[serde(default)]
    pub network: PatternRules,
    #[serde(default)]
    pub mcp: PatternRules,
    #[serde(default)]
    pub acp: PatternRules,
    #[serde(default)]
    pub database_read: PatternRules,
    #[serde(default)]
    pub database_write: PatternRules,
    #[serde(default)]
    pub secrets: PatternRules,
    #[serde(default)]
    pub cloud_read: PatternRules,
    #[serde(default)]
    pub cloud_write: PatternRules,
    #[serde(default)]
    pub deployment: PatternRules,
    #[serde(default)]
    pub browser: PatternRules,
    #[serde(default)]
    pub git: PatternRules,
    #[serde(default)]
    pub delegation: PatternRules,
}

#[derive(Debug, Clone, Copy)]
pub enum CapabilityRequest<'a> {
    ReadPath(&'a str),
    WritePath(&'a str),
    Process(&'a [String]),
    Network(&'a str),
    Mcp(&'a str),
    Acp(&'a str),
    DatabaseRead(&'a str),
    DatabaseWrite(&'a str),
    Secret(&'a str),
    CloudRead(&'a str),
    CloudWrite(&'a str),
    Deployment(&'a str),
    Browser(&'a str),
    Git(&'a [String]),
    Delegate(&'a str),
}

impl AgentPolicy {
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        let raw = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read policy {}", path.as_ref().display()))?;
        serde_yaml::from_str(&raw).context("parse policy YAML")
    }

    pub fn allowed_secrets(&self) -> Vec<String> {
        self.secrets
            .allow
            .iter()
            .map(|pattern| pattern.trim_start_matches("secret://").to_string())
            .filter(|name| !name.trim().is_empty())
            .collect()
    }

    pub fn evaluate(&self, request: CapabilityRequest<'_>) -> Decision {
        let (rules, subject) = match request {
            CapabilityRequest::ReadPath(path) => (&self.filesystem.read, path.to_string()),
            CapabilityRequest::WritePath(path) => (&self.filesystem.write, path.to_string()),
            CapabilityRequest::Process(argv) => (&self.process, shell_join(argv)),
            CapabilityRequest::Network(host) => (&self.network, host.to_string()),
            CapabilityRequest::Mcp(tool) => (&self.mcp, tool.to_string()),
            CapabilityRequest::Acp(agent) => (&self.acp, agent.to_string()),
            CapabilityRequest::DatabaseRead(target) => (&self.database_read, target.to_string()),
            CapabilityRequest::DatabaseWrite(target) => (&self.database_write, target.to_string()),
            CapabilityRequest::Secret(secret) => (&self.secrets, secret.to_string()),
            CapabilityRequest::CloudRead(resource) => (&self.cloud_read, resource.to_string()),
            CapabilityRequest::CloudWrite(resource) => (&self.cloud_write, resource.to_string()),
            CapabilityRequest::Deployment(target) => (&self.deployment, target.to_string()),
            CapabilityRequest::Browser(target) => (&self.browser, target.to_string()),
            CapabilityRequest::Git(argv) => (&self.git, shell_join(argv)),
            CapabilityRequest::Delegate(agent) => (&self.delegation, agent.to_string()),
        };

        self.apply_autonomy_ceiling(request, rules.evaluate(&subject))
    }

    fn apply_autonomy_ceiling(
        &self,
        request: CapabilityRequest<'_>,
        decision: Decision,
    ) -> Decision {
        if decision == Decision::Deny {
            return Decision::Deny;
        }

        match self.autonomy {
            AutonomyLevel::Observe => match request {
                CapabilityRequest::ReadPath(_)
                | CapabilityRequest::DatabaseRead(_)
                | CapabilityRequest::CloudRead(_)
                | CapabilityRequest::Git(_) => decision.min(Decision::Ask),
                _ => Decision::Deny,
            },
            AutonomyLevel::Suggest => match request {
                CapabilityRequest::ReadPath(_)
                | CapabilityRequest::Network(_)
                | CapabilityRequest::DatabaseRead(_)
                | CapabilityRequest::CloudRead(_)
                | CapabilityRequest::Browser(_)
                | CapabilityRequest::Git(_) => decision,
                _ => Decision::Deny,
            },
            AutonomyLevel::Edit => match request {
                CapabilityRequest::WritePath(_) => decision,
                CapabilityRequest::Process(_)
                | CapabilityRequest::Mcp(_)
                | CapabilityRequest::Acp(_)
                | CapabilityRequest::DatabaseWrite(_)
                | CapabilityRequest::Secret(_)
                | CapabilityRequest::CloudWrite(_)
                | CapabilityRequest::Deployment(_)
                | CapabilityRequest::Git(_) => promote_allow_to_ask(decision),
                _ => decision,
            },
            AutonomyLevel::Execute => match request {
                CapabilityRequest::Secret(_)
                | CapabilityRequest::CloudWrite(_)
                | CapabilityRequest::Deployment(_)
                | CapabilityRequest::DatabaseWrite(_) => promote_allow_to_ask(decision),
                _ => decision,
            },
            AutonomyLevel::Autonomous => decision,
        }
    }
}

impl PatternRules {
    pub fn evaluate(&self, subject: &str) -> Decision {
        if self
            .deny
            .iter()
            .any(|pattern| pattern_matches(pattern, subject))
        {
            return Decision::Deny;
        }
        if self
            .ask
            .iter()
            .any(|pattern| pattern_matches(pattern, subject))
        {
            return Decision::Ask;
        }
        if self
            .allow
            .iter()
            .any(|pattern| pattern_matches(pattern, subject))
        {
            return Decision::Allow;
        }
        self.default
    }
}

impl Ord for Decision {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        rank(*self).cmp(&rank(*other))
    }
}

impl PartialOrd for Decision {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn rank(decision: Decision) -> u8 {
    match decision {
        Decision::Deny => 0,
        Decision::Ask => 1,
        Decision::Allow => 2,
    }
}

fn promote_allow_to_ask(decision: Decision) -> Decision {
    if decision == Decision::Allow {
        Decision::Ask
    } else {
        decision
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher().is_match(value))
        .unwrap_or(false)
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if argument
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_./:=@".contains(character))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_dominates_allow() {
        let rules = PatternRules {
            default: Decision::Ask,
            allow: vec!["**".into()],
            ask: vec![],
            deny: vec!["**/.env".into()],
        };
        assert_eq!(rules.evaluate("app/.env"), Decision::Deny);
        assert_eq!(rules.evaluate("src/lib.rs"), Decision::Allow);
    }

    #[test]
    fn execute_mode_still_asks_for_database_write() {
        let policy = AgentPolicy {
            autonomy: AutonomyLevel::Execute,
            database_write: PatternRules {
                default: Decision::Allow,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            policy.evaluate(CapabilityRequest::DatabaseWrite("prod.users")),
            Decision::Ask
        );
    }

    #[test]
    fn autonomous_mode_honors_explicit_deny() {
        let policy = AgentPolicy {
            autonomy: AutonomyLevel::Autonomous,
            deployment: PatternRules {
                default: Decision::Allow,
                deny: vec!["prod/**".into()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            policy.evaluate(CapabilityRequest::Deployment("prod/api")),
            Decision::Deny
        );
    }
}
