use anyhow::Result;
use openforge_protocol::{ModelRequirements, ModelSpec};

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

#[derive(Default)]
pub struct ModelRouter {
    pub weights: RoutingWeights,
}

impl ModelRouter {
    pub fn eligible(
        &self,
        model: &ModelSpec,
        requirements: &ModelRequirements,
    ) -> bool {
        model.context_tokens >= requirements.context_tokens
            && (!requirements.requires_tools || model.supports_tools)
            && (!requirements.requires_vision || model.supports_vision)
            && (!requirements.requires_structured_output
                || model.supports_structured_output)
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
            + expected_output as f64 / 1_000_000.0
                * model.output_usd_per_million
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
        } else {
            0.0
        };

        Some(
            self.weights.quality * model.quality_score
                + self.weights.reliability * 0.9
                + self.weights.privacy * model.privacy_score
                - self.weights.cost * normalized_cost
                - self.weights.latency * model.latency_score.clamp(0.0, 1.0),
        )
    }

    pub fn select<'a>(
        &self,
        models: &'a [ModelSpec],
        requirements: &ModelRequirements,
        expected_input: u64,
        expected_output: u64,
    ) -> Result<&'a ModelSpec> {
        let mut ranked: Vec<(&ModelSpec, f64)> = models
            .iter()
            .filter_map(|model| {
                self.score(
                    model,
                    requirements,
                    expected_input,
                    expected_output,
                )
                .map(|score| (model, score))
            })
            .collect();

        ranked.sort_by(|left, right| right.1.total_cmp(&left.1));

        ranked
            .first()
            .map(|(model, _)| *model)
            .ok_or_else(|| {
                anyhow::anyhow!("no model satisfies routing constraints")
            })
    }
}
