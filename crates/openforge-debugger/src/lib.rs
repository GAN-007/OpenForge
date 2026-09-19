use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebugPhase {
    Reproduce,
    Hypothesize,
    Instrument,
    Observe,
    SelectFix,
    ApplyFix,
    Verify,
    Cleanup,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: Uuid,
    pub statement: String,
    pub confidence: f32,
    #[serde(default)]
    pub evidence_for: Vec<Uuid>,
    #[serde(default)]
    pub evidence_against: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugObservation {
    pub id: Uuid,
    pub kind: String,
    pub source: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrumentation {
    pub id: Uuid,
    pub description: String,
    pub file: String,
    pub reversible_patch: String,
    pub applied: bool,
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub id: Uuid,
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub issue: String,
    pub phase: DebugPhase,
    pub hypotheses: Vec<Hypothesis>,
    pub observations: Vec<DebugObservation>,
    pub instrumentation: Vec<Instrumentation>,
    pub selected_hypothesis: Option<Uuid>,
    pub fix_summary: Option<String>,
    pub verification_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DebugSession {
    pub fn new(run_id: Uuid, task_id: Option<Uuid>, issue: impl Into<String>) -> Result<Self> {
        let issue = issue.into();
        if issue.trim().is_empty() {
            bail!("debug issue cannot be empty");
        }
        let now = Utc::now();
        Ok(Self {
            id: Uuid::now_v7(),
            run_id,
            task_id,
            issue,
            phase: DebugPhase::Reproduce,
            hypotheses: Vec::new(),
            observations: Vec::new(),
            instrumentation: Vec::new(),
            selected_hypothesis: None,
            fix_summary: None,
            verification_summary: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn advance(&mut self, next: DebugPhase) -> Result<()> {
        if !valid_transition(self.phase, next) {
            bail!(
                "invalid debug phase transition {:?} -> {:?}",
                self.phase,
                next
            );
        }
        self.phase = next;
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn add_hypothesis(
        &mut self,
        statement: impl Into<String>,
        confidence: f32,
    ) -> Result<Uuid> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            bail!("hypothesis confidence must be between zero and one");
        }
        let statement = statement.into();
        if statement.trim().is_empty() {
            bail!("hypothesis statement cannot be empty");
        }
        let id = Uuid::now_v7();
        self.hypotheses.push(Hypothesis {
            id,
            statement,
            confidence,
            evidence_for: Vec::new(),
            evidence_against: Vec::new(),
        });
        self.updated_at = Utc::now();
        Ok(id)
    }

    pub fn record_observation(
        &mut self,
        kind: impl Into<String>,
        source: impl Into<String>,
        data: Value,
    ) -> Uuid {
        let id = Uuid::now_v7();
        self.observations.push(DebugObservation {
            id,
            kind: kind.into(),
            source: source.into(),
            data,
            created_at: Utc::now(),
        });
        self.updated_at = Utc::now();
        id
    }

    pub fn attach_evidence(
        &mut self,
        hypothesis_id: Uuid,
        observation_id: Uuid,
        supports: bool,
    ) -> Result<()> {
        if !self
            .observations
            .iter()
            .any(|value| value.id == observation_id)
        {
            bail!("unknown debug observation {observation_id}");
        }
        let hypothesis = self
            .hypotheses
            .iter_mut()
            .find(|value| value.id == hypothesis_id)
            .ok_or_else(|| anyhow::anyhow!("unknown debug hypothesis {hypothesis_id}"))?;
        let target = if supports {
            &mut hypothesis.evidence_for
        } else {
            &mut hypothesis.evidence_against
        };
        if !target.contains(&observation_id) {
            target.push(observation_id);
        }
        Ok(())
    }

    pub fn ranked_hypotheses(&self) -> Vec<(Uuid, f32)> {
        let mut values = self
            .hypotheses
            .iter()
            .map(|hypothesis| {
                let support = hypothesis.evidence_for.len() as f32;
                let against = hypothesis.evidence_against.len() as f32;
                let evidence_score = if support + against > 0.0 {
                    (support - against) / (support + against)
                } else {
                    0.0
                };
                (
                    hypothesis.id,
                    (hypothesis.confidence * 0.7 + (evidence_score + 1.0) * 0.15).clamp(0.0, 1.0),
                )
            })
            .collect::<Vec<_>>();
        values.sort_by(|left, right| right.1.total_cmp(&left.1));
        values
    }

    pub fn select_hypothesis(&mut self, id: Uuid) -> Result<()> {
        if !self.hypotheses.iter().any(|value| value.id == id) {
            bail!("cannot select unknown debug hypothesis");
        }
        self.selected_hypothesis = Some(id);
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn add_instrumentation(
        &mut self,
        description: impl Into<String>,
        file: impl Into<String>,
        reversible_patch: impl Into<String>,
    ) -> Result<Uuid> {
        let description = description.into();
        let file = file.into();
        let reversible_patch = reversible_patch.into();
        if description.trim().is_empty()
            || file.trim().is_empty()
            || reversible_patch.trim().is_empty()
        {
            bail!("debug instrumentation requires description, file and reversible patch");
        }
        let id = Uuid::now_v7();
        self.instrumentation.push(Instrumentation {
            id,
            description,
            file,
            reversible_patch,
            applied: false,
            removed: false,
        });
        Ok(id)
    }

    pub fn mark_instrumentation_applied(&mut self, id: Uuid) -> Result<()> {
        let item = self
            .instrumentation
            .iter_mut()
            .find(|value| value.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown instrumentation {id}"))?;
        item.applied = true;
        Ok(())
    }

    pub fn mark_instrumentation_removed(&mut self, id: Uuid) -> Result<()> {
        let item = self
            .instrumentation
            .iter_mut()
            .find(|value| value.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown instrumentation {id}"))?;
        if !item.applied {
            bail!("cannot remove instrumentation that was never applied");
        }
        item.removed = true;
        Ok(())
    }

    pub fn can_complete(&self) -> bool {
        self.selected_hypothesis.is_some()
            && self
                .fix_summary
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .verification_summary
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .instrumentation
                .iter()
                .all(|item| !item.applied || item.removed)
    }
}

fn valid_transition(current: DebugPhase, next: DebugPhase) -> bool {
    use DebugPhase::*;
    matches!(
        (current, next),
        (Reproduce, Hypothesize)
            | (Hypothesize, Instrument)
            | (Hypothesize, Observe)
            | (Instrument, Observe)
            | (Observe, Hypothesize)
            | (Observe, SelectFix)
            | (SelectFix, ApplyFix)
            | (ApplyFix, Verify)
            | (Verify, ApplyFix)
            | (Verify, Cleanup)
            | (Cleanup, Completed)
            | (_, Failed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_session_requires_evidence_flow() {
        let mut session = DebugSession::new(Uuid::new_v4(), None, "panic").unwrap();
        session.advance(DebugPhase::Hypothesize).unwrap();
        let hypothesis = session.add_hypothesis("null state", 0.7).unwrap();
        session.advance(DebugPhase::Observe).unwrap();
        let observation = session.record_observation("log", "stderr", serde_json::json!("null"));
        session
            .attach_evidence(hypothesis, observation, true)
            .unwrap();
        assert_eq!(session.ranked_hypotheses()[0].0, hypothesis);
    }
}
