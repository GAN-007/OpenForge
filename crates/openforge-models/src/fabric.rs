use crate::ModelProvider;
use anyhow::{bail, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use std::sync::Arc;

pub struct FabricProvider {
    providers: Vec<Arc<dyn ModelProvider>>,
    catalog: Vec<ModelSpec>,
}

impl FabricProvider {
    pub fn new(providers: Vec<Arc<dyn ModelProvider>>) -> Result<Self> {
        if providers.is_empty() {
            bail!("model fabric requires at least one provider");
        }

        let mut catalog = Vec::new();
        for provider in &providers {
            for model in provider.catalog() {
                if model.provider != provider.name() {
                    bail!(
                        "model {} declares provider {} but is registered by {}",
                        model.model,
                        model.provider,
                        provider.name()
                    );
                }
                catalog.push(model.clone());
            }
        }

        if catalog.is_empty() {
            bail!("model fabric has no configured models");
        }

        Ok(Self { providers, catalog })
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|provider| provider.name().to_owned())
            .collect()
    }
}

#[async_trait]
impl ModelProvider for FabricProvider {
    fn name(&self) -> &str {
        "openforge-model-fabric"
    }

    fn catalog(&self) -> &[ModelSpec] {
        &self.catalog
    }

    async fn invoke(
        &self,
        model: &ModelSpec,
        request: &ModelRequest,
    ) -> Result<ModelResponse> {
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.name() == model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no provider {} is registered for model {}",
                    model.provider,
                    model.model
                )
            })?;

        provider.invoke(model, request).await
    }
}
