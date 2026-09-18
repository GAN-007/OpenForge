use anyhow::{bail,Result};
use async_trait::async_trait;
use std::collections::BTreeSet;

#[async_trait]
pub trait SecretBroker:Send+Sync{
    async fn resolve(&self,name:&str)->Result<String>;
}
pub struct EnvironmentSecretBroker{allowed:BTreeSet<String>}
impl EnvironmentSecretBroker{
    pub fn new(allowed:impl IntoIterator<Item=String>)->Self{Self{allowed:allowed.into_iter().collect()}}
}
#[async_trait]
impl SecretBroker for EnvironmentSecretBroker{
    async fn resolve(&self,name:&str)->Result<String>{
        if !self.allowed.contains(name){bail!("secret {name} is not permitted by broker policy")}
        std::env::var(name).map_err(|_|anyhow::anyhow!("secret {name} is not available"))
    }
}
