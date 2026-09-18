use crate::{ModelProvider, ModelRouter};
use anyhow::{bail, Result};
use async_trait::async_trait;
use openforge_protocol::{ModelRequest, ModelResponse, ModelSpec};
use std::sync::Arc;

pub struct FabricProvider {
    providers: Vec<Arc<dyn ModelProvider>>,
    catalog: Vec<ModelSpec>,
    max_fallback_attempts: usize,
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
                if catalog.iter().any(|existing: &ModelSpec| {
                    existing.provider == model.provider && existing.model == model.model
                }) {
                    bail!(
                        "duplicate model registration {}/{}",
                        model.provider,
                        model.model
                    );
                }
                catalog.push(model.clone());
            }
        }

        if catalog.is_empty() {
            bail!("model fabric has no configured models");
        }

        Ok(Self {
            providers,
            catalog,
            max_fallback_attempts: 3,
        })
    }

    pub fn with_max_fallback_attempts(mut self, attempts: usize) -> Self {
        self.max_fallback_attempts = attempts.max(1);
        self
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|provider| provider.name().to_owned())
            .collect()
    }

    async fn invoke_exact(
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

    fn fallback_order<'a>(
        &'a self,
        preferred: &'a ModelSpec,
        request: &ModelRequest,
    ) -> Vec<&'a ModelSpec> {
        let expected_input = request
            .messages
            .iter()
            .map(|message| message.content.chars().count() as u64)
            .sum::<u64>()
            .div_ceil(4)
            .max(1);
        let expected_output = request.max_output_tokens as u64;
        let router = ModelRouter::default();

        let mut models = vec![preferred];
        for (candidate, _) in router.rank(
            &self.catalog,
            &request.requirements,
            expected_input,
            expected_output,
        ) {
            if candidate.provider == preferred.provider && candidate.model == preferred.model {
                continue;
            }
            models.push(candidate);
        }
        models.truncate(self.max_fallback_attempts);
        models
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
        let mut failures = Vec::new();

        for candidate in self.fallback_order(model, request) {
            match self.invoke_exact(candidate, request).await {
                Ok(response) => return Ok(response),
                Err(error) => failures.push(format!(
                    "{}/{}: {}",
                    candidate.provider, candidate.model, error
                )),
            }
        }

        bail!(
            "all eligible model attempts failed: {}",
            failures.join(" | ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use openforge_protocol::{
        DataClassification, ModelRequirements, ModelSpec,
    };

    struct Provider {
        name: String,
        models: Vec<ModelSpec>,
        fail: bool,
    }

    #[async_trait]
    impl ModelProvider for Provider {
        fn name(&self) -> &str {
            &self.name
        }

        fn catalog(&self) -> &[ModelSpec] {
            &self.models
        }

        async fn invoke(
            &self,
            model: &ModelSpec,
            _request: &ModelRequest,
        ) -> Result<ModelResponse> {
            if self.fail {
                bail!("intentional failure");
            }
            Ok(ModelResponse {
                provider: self.name.clone(),
                model: model.model.clone(),
                text: "ok".into(),
                input_tokens: 1,
                output_tokens: 1,
                latency_ms: 1,
                cost_usd: 0.0,
                provider_request_id: None,
            })
        }
    }

    fn spec(provider: &str, model: &str) -> ModelSpec {
        ModelSpec {
            provider: provider.into(),
            model: model.into(),
            family: "test".into(),
            context_tokens: 32_000,
            supports_tools: true,
            supports_vision: false,
            supports_structured_output: true,
            input_usd_per_million: 0.0,
            output_usd_per_million: 0.0,
            latency_score: 0.1,
            quality_score: 0.8,
            privacy_score: 1.0,
            max_data_classification: DataClassification::Restricted,
        }
    }

    #[tokio::test]
    async fn falls_back_to_second_provider() {
        let first = Arc::new(Provider {
            name: "first".into(),
            models: vec![spec("first", "a")],
            fail: true,
        });
        let second = Arc::new(Provider {
            name: "second".into(),
            models: vec![spec("second", "b")],
            fail: false,
        });
        let fabric = FabricProvider::new(vec![first, second]).unwrap();
        let request = ModelRequest {
            invocation_id: uuid::Uuid::new_v4(),
            run_id: uuid::Uuid::new_v4(),
            task_id: None,
            messages: vec![],
            requirements: ModelRequirements {
                task_class: "test".into(),
                context_tokens: 100,
                requires_tools: false,
                requires_vision: false,
                requires_structured_output: false,
                max_cost_usd: 1.0,
                max_latency_ms: None,
                data_classification: DataClassification::Internal,
                preferred_model_families: vec![],
                excluded_model_families: vec![],
            },
            temperature: 0.0,
            max_output_tokens: 16,
            response_schema: None,
        };

        let response = fabric.invoke(&fabric.catalog[0], &request).await.unwrap();
        assert_eq!(response.provider, "second");
    }
}
