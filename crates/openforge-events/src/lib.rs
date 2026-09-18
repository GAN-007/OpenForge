use anyhow::{bail, Result};
use openforge_protocol::EventEnvelope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventIntegrityReport {
    pub valid: bool,
    pub verified_events: usize,
    pub first_sequence: Option<i64>,
    pub last_sequence: Option<i64>,
    pub terminal_hash: Option<String>,
    pub violations: Vec<String>,
}

pub fn event_hash(event: &EventEnvelope) -> Result<String> {
    let canonical = serde_json::json!({
        "event_id": event.event_id,
        "run_id": event.run_id,
        "task_id": event.task_id,
        "timestamp": event.timestamp,
        "actor": &event.actor,
        "event_type": &event.event_type,
        "payload": &event.payload,
        "previous_event_hash": &event.previous_event_hash
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?)))
}

pub fn verify_event_chain(events: &[EventEnvelope]) -> Result<EventIntegrityReport> {
    let mut violations = Vec::new();
    let mut previous_hash: Option<&str> = None;
    let mut previous_sequence: Option<i64> = None;

    for event in events {
        if let Some(sequence) = previous_sequence {
            if event.sequence <= sequence {
                violations.push(format!(
                    "sequence {} is not strictly greater than {}",
                    event.sequence, sequence
                ));
            }
        }

        if event.previous_event_hash.as_deref() != previous_hash {
            violations.push(format!(
                "event {} has previous hash {:?}, expected {:?}",
                event.event_id,
                event.previous_event_hash.as_deref(),
                previous_hash
            ));
        }

        let calculated = event_hash(event)?;
        if calculated != event.event_hash {
            violations.push(format!(
                "event {} hash mismatch: stored={}, calculated={}",
                event.event_id, event.event_hash, calculated
            ));
        }

        previous_sequence = Some(event.sequence);
        previous_hash = Some(event.event_hash.as_str());
    }

    Ok(EventIntegrityReport {
        valid: violations.is_empty(),
        verified_events: events.len(),
        first_sequence: events.first().map(|event| event.sequence),
        last_sequence: events.last().map(|event| event.sequence),
        terminal_hash: events.last().map(|event| event.event_hash.clone()),
        violations,
    })
}

pub fn require_valid_chain(events: &[EventEnvelope]) -> Result<()> {
    let report = verify_event_chain(events)?;
    if !report.valid {
        bail!("event chain integrity failure: {}", report.violations.join("; "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use openforge_protocol::Actor;
    use uuid::Uuid;

    fn event(sequence: i64, previous: Option<String>) -> EventEnvelope {
        let mut value = EventEnvelope {
            event_id: Uuid::new_v4(),
            sequence,
            run_id: None,
            task_id: None,
            timestamp: Utc::now(),
            actor: Actor {
                kind: "system".into(),
                id: "test".into(),
            },
            event_type: "test".into(),
            payload: serde_json::json!({"sequence": sequence}),
            previous_event_hash: previous,
            event_hash: String::new(),
        };
        value.event_hash = event_hash(&value).unwrap();
        value
    }

    #[test]
    fn validates_hash_linked_events() {
        let first = event(1, None);
        let second = event(2, Some(first.event_hash.clone()));
        let report = verify_event_chain(&[first, second]).unwrap();
        assert!(report.valid);
        assert_eq!(report.verified_events, 2);
    }

    #[test]
    fn detects_tampering() {
        let first = event(1, None);
        let mut second = event(2, Some(first.event_hash.clone()));
        second.payload = serde_json::json!({"tampered": true});
        let report = verify_event_chain(&[first, second]).unwrap();
        assert!(!report.valid);
    }
}
