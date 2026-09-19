use anyhow::Result;
use openforge_protocol::{ModelRequirements, ModelSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct RoutingWeights {
    pub quality: f64,
    pub reliability: f64,
    pub privacy: f64,
    pub cost: f64,
    pub latency: f64,
}

impl Default for RoutingWeights {
    fn default() -> Self {
        Self {
            quality: 0.45,
            reliability: 0.15,
            privacy: 0.15,
            cost: 0.15,
            latency: 0.10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingCandidate {
    pub provider: String,
    pub model: String,
    pub family: String,
    pub estimated_cost_usd: f64,
    pub score: f64,
}

#[derive(Default)]
pub struct ModelRouter {
    pub weights: RoutingWeights,
}

impl ModelRouter {
    pub fn eligible(&self, model: &ModelSpec, requirements: &ModelRequirements) -> bool {
        model.context_tokens >= requirements.context_tokens
            && (!requirements.requires_tools || model.supports_tools)
            && (!requirements.requires_vision || model.supports_vision)
            && (!requirements.requires_structured_output || model.supports_structured_output)
            && model.max_data_classification >= requirements.data_classification
            && !requirements
                .excluded_model_families
                .iter()
                .any(|family| family == &model.family)
            && (requirements.preferred_model_families.is_empty()
                || requirements
                    .preferred_model_families
                    .iter()
                    .any(|family| family == &model.family))
    }

    pub fn estimated_cost(
        &self,
        model: &ModelSpec,
        expected_input: u64,
        expected_output: u64,
    ) -> f64 {
        expected_input as f64 / 1_000_000.0 * model.input_usd_per_million
            + expected_output as f64 / 1_000_000.0 * model.output_usd_per_million
    }

    pub fn score(
        &self,
        model: &ModelSpec,
        requirements: &ModelRequirements,
        expected_input: u64,
        expected_output: u64,
    ) -> Option<f64> {
        if !self.eligible(model, requirements) {
            return None;
        }

        let cost = self.estimated_cost(model, expected_input, expected_output);
        if cost > requirements.max_cost_usd {
            return None;
        }

        if let Some(max_latency_ms) = requirements.max_latency_ms {
            let estimated_ms = (model.latency_score.clamp(0.0, 1.0) * 60_000.0) as u64;
            if estimated_ms > max_latency_ms {
                return None;
            }
        }

        let normalized_cost = if requirements.max_cost_usd > 0.0 {
            cost / requirements.max_cost_usd
        } else if cost <= f64::EPSILON {
            0.0
        } else {
            return None;
        };

        Some(
            self.weights.quality * model.quality_score
                + self.weights.reliability * 0.9
                + self.weights.privacy * model.privacy_score
                - self.weights.cost * normalized_cost
                - self.weights.latency * model.latency_score.clamp(0.0, 1.0),
        )
    }

    pub fn rank<'a>(
        &self,
        models: &'a [ModelSpec],
        requirements: &ModelRequirements,
        expected_input: u64,
        expected_output: u64,
    ) -> Vec<(&'a ModelSpec, RoutingCandidate)> {
        let mut ranked: Vec<(&ModelSpec, RoutingCandidate)> = models
            .iter()
            .filter_map(|model| {
                let score = self.score(model, requirements, expected_input, expected_output)?;
                Some((
                    model,
                    RoutingCandidate {
                        provider: model.provider.clone(),
                        model: model.model.clone(),
                        family: model.family.clone(),
                        estimated_cost_usd: self.estimated_cost(
                            model,
                            expected_input,
                            expected_output,
                        ),
                        score,
                    },
                ))
            })
            .collect();

        ranked.sort_by(|left, right| {
            right
                .1
                .score
                .total_cmp(&left.1.score)
                .then_with(|| {
                    left.1
                        .estimated_cost_usd
                        .total_cmp(&right.1.estimated_cost_usd)
                })
                .then_with(|| left.1.provider.cmp(&right.1.provider))
                .then_with(|| left.1.model.cmp(&right.1.model))
        });
        ranked
    }

    pub fn plan(
        &self,
        models: &[ModelSpec],
        requirements: &ModelRequirements,
        expected_input: u64,
        expected_output: u64,
    ) -> Vec<RoutingCandidate> {
        self.rank(models, requirements, expected_input, expected_output)
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect()
    }

    pub fn select<'a>(
        &self,
        models: &'a [ModelSpec],
        requirements: &ModelRequirements,
        expected_input: u64,
        expected_output: u64,
    ) -> Result<&'a ModelSpec> {
        self.rank(models, requirements, expected_input, expected_output)
            .first()
            .map(|(model, _)| *model)
            .ok_or_else(|| anyhow::anyhow!("no model satisfies routing constraints"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openforge_protocol::{DataClassification, ModelRequirements, ModelSpec};

    fn model(name: &str, cost: f64, quality: f64) -> ModelSpec {
        ModelSpec {
            provider: "test".into(),
            model: name.into(),
            family: "test".into(),
            context_tokens: 32_000,
            supports_tools: true,
            supports_vision: false,
            supports_structured_output: true,
            input_usd_per_million: cost,
            output_usd_per_million: cost,
            latency_score: 0.2,
            quality_score: quality,
            privacy_score: 1.0,
            max_data_classification: DataClassification::Restricted,
        }
    }

    #[test]
    fn rank_excludes_models_above_hard_budget() {
        let router = ModelRouter::default();
        let models = vec![model("cheap", 1.0, 0.8), model("expensive", 1000.0, 1.0)];
        let requirements = ModelRequirements {
            task_class: "coding".into(),
            context_tokens: 1000,
            requires_tools: false,
            requires_vision: false,
            requires_structured_output: false,
            max_cost_usd: 0.01,
            max_latency_ms: None,
            data_classification: DataClassification::Internal,
            preferred_model_families: vec![],
            excluded_model_families: vec![],
        };
        let plan = router.plan(&models, &requirements, 1000, 1000);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].model, "cheap");
    }
}
