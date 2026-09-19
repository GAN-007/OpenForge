use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use openforge_protocol::{CapabilityDomain, TaskBudget};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub schema: String,
    pub id: String,
    pub role: String,
    pub description: String,
    pub system_prompt: String,
    pub model_profile: String,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,
    #[serde(default)]
    pub memory_scopes: BTreeSet<String>,
    #[serde(default)]
    pub capabilities: BTreeSet<CapabilityDomain>,
    pub budget: TaskBudget,
    #[serde(default = "default_iterations")]
    pub max_iterations: u32,
    #[serde(default)]
    pub required_reviews: BTreeSet<String>,
    #[serde(default)]
    pub independent_review_model_family: bool,
    #[serde(default)]
    pub verification: VerificationProfile,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

fn default_iterations() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationProfile {
    #[serde(default = "default_true")]
    pub compile: bool,
    #[serde(default = "default_true")]
    pub lint: bool,
    #[serde(default = "default_true")]
    pub test: bool,
    #[serde(default)]
    pub runtime: bool,
    #[serde(default)]
    pub security_review: bool,
    #[serde(default)]
    pub final_review: bool,
}

impl Default for VerificationProfile {
    fn default() -> Self {
        Self {
            compile: true,
            lint: true,
            test: true,
            runtime: false,
            security_review: false,
            final_review: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedAgentManifest {
    pub manifest: AgentManifest,
    pub digest_sha256: String,
    pub signer_public_key_base64: String,
    pub signature_base64: String,
}

impl AgentManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != "openforge.agent/v1" {
            bail!("unsupported agent manifest schema {}", self.schema);
        }
        if self.id.trim().is_empty()
            || self.role.trim().is_empty()
            || self.system_prompt.trim().is_empty()
            || self.model_profile.trim().is_empty()
        {
            bail!("agent id, role, system_prompt and model_profile are required");
        }
        if self.max_iterations == 0 {
            bail!("agent max_iterations must be greater than zero");
        }
        if !self.budget.max_usd.is_finite() || self.budget.max_usd < 0.0 {
            bail!("agent budget must be finite and non-negative");
        }
        if self
            .allowed_tools
            .iter()
            .any(|tool| self.denied_tools.contains(tool))
        {
            bail!("an agent tool cannot be both allowed and denied");
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn digest_sha256(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(self.canonical_bytes()?)))
    }
}

impl SignedAgentManifest {
    pub fn verify(&self) -> Result<()> {
        self.manifest.validate()?;
        let canonical = self.manifest.canonical_bytes()?;
        let digest = hex::encode(Sha256::digest(&canonical));
        if digest != self.digest_sha256 {
            bail!("agent manifest digest mismatch");
        }

        let key_bytes = BASE64
            .decode(self.signer_public_key_base64.as_bytes())
            .context("decode agent manifest public key")?;
        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("agent signer key must be 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&key_array).context("invalid Ed25519 public key")?;
        let signature_bytes = BASE64
            .decode(self.signature_base64.as_bytes())
            .context("decode agent manifest signature")?;
        let signature =
            Signature::from_slice(&signature_bytes).context("invalid Ed25519 signature")?;
        key.verify(&canonical, &signature)
            .context("agent manifest signature verification failed")
    }
}

#[derive(Default, Clone)]
pub struct AgentRegistry {
    manifests: BTreeMap<String, AgentManifest>,
}

impl AgentRegistry {
    pub fn load_directory(path: impl AsRef<Path>) -> Result<Self> {
        let mut registry = Self::default();
        if !path.as_ref().exists() {
            return Ok(registry);
        }

        let mut entries = fs::read_dir(path.as_ref())?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !matches!(extension, "yaml" | "yml" | "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read agent manifest {}", path.display()))?;
            let manifest: AgentManifest = if extension == "json" {
                serde_json::from_str(&raw)
                    .with_context(|| format!("parse agent manifest {}", path.display()))?
            } else {
                serde_yaml::from_str(&raw)
                    .with_context(|| format!("parse agent manifest {}", path.display()))?
            };
            registry.insert(manifest)?;
        }

        Ok(registry)
    }

    pub fn insert(&mut self, manifest: AgentManifest) -> Result<()> {
        manifest.validate()?;
        if self.manifests.contains_key(&manifest.id) {
            bail!("duplicate agent manifest id {}", manifest.id);
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&AgentManifest> {
        self.manifests.get(id)
    }

    pub fn by_role(&self, role: &str) -> Vec<&AgentManifest> {
        self.manifests
            .values()
            .filter(|manifest| manifest.role == role)
            .collect()
    }

    pub fn all(&self) -> Vec<&AgentManifest> {
        self.manifests.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AgentManifest {
        AgentManifest {
            schema: "openforge.agent/v1".into(),
            id: "backend-primary".into(),
            role: "backend-engineer".into(),
            description: "Backend implementation".into(),
            system_prompt: "Implement and verify backend changes.".into(),
            model_profile: "agent".into(),
            allowed_tools: BTreeSet::from(["read_file".into(), "write_file".into()]),
            denied_tools: BTreeSet::new(),
            memory_scopes: BTreeSet::from(["project".into()]),
            capabilities: BTreeSet::new(),
            budget: TaskBudget {
                max_usd: 2.0,
                max_model_calls: 20,
                max_tool_calls: 100,
                max_wall_seconds: 1800,
            },
            max_iterations: 30,
            required_reviews: BTreeSet::from(["reviewer".into()]),
            independent_review_model_family: true,
            verification: VerificationProfile::default(),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn registry_rejects_duplicate_ids() {
        let mut registry = AgentRegistry::default();
        registry.insert(manifest()).unwrap();
        assert!(registry.insert(manifest()).is_err());
    }
}
