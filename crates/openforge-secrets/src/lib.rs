use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use openforge_protocol::{SecretLeaseDescriptor, SecretLeaseId};
use std::{collections::BTreeSet, fmt};
use zeroize::Zeroizing;

pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

pub struct SecretLease {
    pub descriptor: SecretLeaseDescriptor,
    value: SecretValue,
}

impl SecretLease {
    pub fn expose(&self) -> Result<&str> {
        if Utc::now() >= self.descriptor.expires_at {
            bail!("secret lease {} has expired", self.descriptor.id);
        }
        Ok(self.value.expose())
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.descriptor.expires_at
    }
}

impl fmt::Debug for SecretLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretLease")
            .field("descriptor", &self.descriptor)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
pub trait SecretBroker: Send + Sync {
    async fn lease(&self, name: &str, audience: &str, ttl_seconds: u64) -> Result<SecretLease>;

    async fn resolve(&self, name: &str) -> Result<SecretValue> {
        let lease = self.lease(name, "compatibility-resolve", 30).await?;
        Ok(SecretValue::new(lease.expose()?.to_string()))
    }
}

pub struct EnvironmentSecretBroker {
    allowed: BTreeSet<String>,
    max_ttl_seconds: u64,
}

impl EnvironmentSecretBroker {
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self::with_max_ttl(allowed, 300)
    }

    pub fn with_max_ttl(allowed: impl IntoIterator<Item = String>, max_ttl_seconds: u64) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
            max_ttl_seconds: max_ttl_seconds.max(1),
        }
    }

    fn validate_request(&self, name: &str, audience: &str) -> Result<()> {
        if !self.allowed.contains(name) {
            bail!("secret {name} is not permitted by broker policy");
        }
        if audience.trim().is_empty() {
            bail!("secret lease audience cannot be empty");
        }
        Ok(())
    }
}

#[async_trait]
impl SecretBroker for EnvironmentSecretBroker {
    async fn lease(&self, name: &str, audience: &str, ttl_seconds: u64) -> Result<SecretLease> {
        self.validate_request(name, audience)?;

        let ttl = ttl_seconds.clamp(1, self.max_ttl_seconds);
        let value =
            std::env::var(name).with_context(|| format!("secret {name} is not available"))?;
        let issued_at = Utc::now();
        let expires_at = issued_at
            .checked_add_signed(Duration::seconds(ttl as i64))
            .context("secret lease expiry overflow")?;

        Ok(SecretLease {
            descriptor: SecretLeaseDescriptor {
                id: SecretLeaseId::new(),
                secret_name: name.to_string(),
                issued_at,
                expires_at,
                audience: audience.to_string(),
                renewable: false,
            },
            value: SecretValue::new(value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn denied_secret_is_never_resolved() {
        let broker = EnvironmentSecretBroker::new(Vec::<String>::new());
        assert!(broker.lease("HOME", "test", 10).await.is_err());
    }

    #[test]
    fn debug_output_redacts_secret_value() {
        let value = SecretValue::new("very-secret".into());
        assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
    }
}
