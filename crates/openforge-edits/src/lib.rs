use anyhow::{bail, Context, Result};
use openforge_models::{ModelProvider, ModelRouter};
use openforge_protocol::{
    ChatMessage, DataClassification, ModelRequest, ModelRequirements,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentEdit {
    pub file_path: String,
    pub before: String,
    pub after: String,
    pub cursor_line: u32,
    pub cursor_column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticContext {
    pub file_path: String,
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPredictionInput {
    pub run_id: Uuid,
    pub file_path: String,
    pub language: String,
    pub prefix: String,
    pub suffix: String,
    #[serde(default)]
    pub recent_edits: Vec<RecentEdit>,
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticContext>,
    #[serde(default)]
    pub semantic_context: String,
    #[serde(default)]
    pub preferred_model_families: Vec<String>,
    pub max_cost_usd: f64,
    #[serde(default = "default_latency")]
    pub max_latency_ms: u64,
}

fn default_latency() -> u64 {
    1500
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRange {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictedEdit {
    pub file_path: String,
    pub range: EditRange,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorTarget {
    pub file_path: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPrediction {
    pub edits: Vec<PredictedEdit>,
    pub next_cursor: Option<CursorTarget>,
    pub confidence: f32,
    pub provider: String,
    pub model: String,
    pub latency_ms: u64,
    pub cost_usd: f64,
    pub cache_hit: bool,
}

#[derive(Debug, Deserialize)]
struct ModelPrediction {
    #[serde(default)]
    edits: Vec<PredictedEdit>,
    next_cursor: Option<CursorTarget>,
    #[serde(default)]
    confidence: f32,
}

#[derive(Clone)]
struct CachedPrediction {
    created_at: Instant,
    prediction: EditPrediction,
}

pub struct EditPredictor {
    provider: Arc<dyn ModelProvider>,
    router: ModelRouter,
    cache: Mutex<HashMap<String, CachedPrediction>>,
    cache_ttl: Duration,
    max_cache_entries: usize,
}

impl EditPredictor {
    pub fn new(provider: Arc<dyn ModelProvider>) -> Self {
        Self {
            provider,
            router: ModelRouter::default(),
            cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(20),
            max_cache_entries: 512,
        }
    }

    pub async fn predict(&self, input: EditPredictionInput) -> Result<EditPrediction> {
        validate_input(&input)?;
        let cache_key = cache_key(&input)?;
        {
            let mut cache = self.cache.lock().await;
            cache.retain(|_, value| value.created_at.elapsed() <= self.cache_ttl);
            if let Some(value) = cache.get(&cache_key) {
                let mut prediction = value.prediction.clone();
                prediction.cache_hit = true;
                return Ok(prediction);
            }
        }

        let prompt = serde_json::json!({
            "file_path": input.file_path,
            "language": input.language,
            "prefix": tail_chars(&input.prefix, 12_000),
            "suffix": head_chars(&input.suffix, 4_000),
            "recent_edits": input.recent_edits.iter().rev().take(12).collect::<Vec<_>>(),
            "diagnostics": input.diagnostics.iter().take(50).collect::<Vec<_>>(),
            "semantic_context": head_chars(&input.semantic_context, 12_000)
        });

        let request = ModelRequest {
            invocation_id: Uuid::now_v7(),
            run_id: input.run_id,
            task_id: None,
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: concat!(
                        "You are OpenForge Edit Prediction. Predict the smallest high-confidence ",
                        "next edits the developer is likely to make. Return strict JSON with keys ",
                        "edits, next_cursor, confidence. edits is an array of {file_path,range:{",
                        "start_line,start_column,end_line,end_column},new_text}. ",
                        "Use zero-based lines and columns. Prefer no prediction over speculative edits. ",
                        "You may predict multiple files only when recent edits and diagnostics strongly support it."
                    )
                    .into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: serde_json::to_string(&prompt)?,
                },
            ],
            requirements: ModelRequirements {
                task_class: "edit_prediction".into(),
                context_tokens: 20_000,
                requires_tools: false,
                requires_vision: false,
                requires_structured_output: true,
                max_cost_usd: input.max_cost_usd,
                max_latency_ms: Some(input.max_latency_ms),
                data_classification: DataClassification::Internal,
                preferred_model_families: input.preferred_model_families,
                excluded_model_families: Vec::new(),
            },
            temperature: 0.0,
            max_output_tokens: 1200,
            response_schema: None,
        };

        let model = self.router.select(
            self.provider.catalog(),
            &request.requirements,
            14_000,
            900,
        )?;
        let response = self.provider.invoke(model, &request).await?;
        if response.cost_usd > input.max_cost_usd {
            bail!("edit prediction exceeded hard call budget");
        }
        let parsed: ModelPrediction = serde_json::from_str(extract_json(&response.text))
            .context("edit prediction model returned invalid JSON")?;

        if !parsed.confidence.is_finite() || !(0.0..=1.0).contains(&parsed.confidence) {
            bail!("edit prediction confidence must be between zero and one");
        }
        if parsed.edits.len() > 12 {
            bail!("edit prediction returned too many edits");
        }
        for edit in &parsed.edits {
            if edit.file_path.trim().is_empty() {
                bail!("edit prediction contains empty file path");
            }
            if edit.range.end_line < edit.range.start_line
                || (edit.range.end_line == edit.range.start_line
                    && edit.range.end_column < edit.range.start_column)
            {
                bail!("edit prediction contains invalid range");
            }
        }

        let prediction = EditPrediction {
            edits: parsed.edits,
            next_cursor: parsed.next_cursor,
            confidence: parsed.confidence,
            provider: response.provider,
            model: response.model,
            latency_ms: response.latency_ms,
            cost_usd: response.cost_usd,
            cache_hit: false,
        };

        let mut cache = self.cache.lock().await;
        if cache.len() >= self.max_cache_entries {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            cache_key,
            CachedPrediction {
                created_at: Instant::now(),
                prediction: prediction.clone(),
            },
        );
        Ok(prediction)
    }
}

fn validate_input(input: &EditPredictionInput) -> Result<()> {
    if input.file_path.trim().is_empty() || input.language.trim().is_empty() {
        bail!("edit prediction file_path and language are required");
    }
    if !input.max_cost_usd.is_finite() || input.max_cost_usd < 0.0 {
        bail!("edit prediction max_cost_usd must be finite and non-negative");
    }
    if input.max_latency_ms == 0 {
        bail!("edit prediction max_latency_ms must be greater than zero");
    }
    Ok(())
}

fn cache_key(input: &EditPredictionInput) -> Result<String> {
    let bytes = serde_json::to_vec(input)?;
    Ok(hex_digest(&bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn extract_json(value: &str) -> &str {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    trimmed
}

fn tail_chars(value: &str, maximum: usize) -> String {
    value
        .chars()
        .rev()
        .take(maximum)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn head_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}
