use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub average: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub generated_at: DateTime<Utc>,
    pub counters: BTreeMap<String, u64>,
    pub gauges: BTreeMap<String, f64>,
    pub histograms: BTreeMap<String, HistogramSnapshot>,
    pub recent_events: Vec<TelemetryEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub timestamp: DateTime<Utc>,
    pub name: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
struct HistogramState {
    count: u64,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
}

impl HistogramState {
    fn observe(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.count += 1;
        self.sum += value;
        self.min = Some(self.min.map_or(value, |current| current.min(value)));
        self.max = Some(self.max.map_or(value, |current| current.max(value)));
    }

    fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            count: self.count,
            sum: self.sum,
            min: self.min,
            max: self.max,
            average: (self.count > 0).then_some(self.sum / self.count as f64),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    histograms: BTreeMap<String, HistogramState>,
    events: Vec<TelemetryEvent>,
}

#[derive(Clone)]
pub struct TelemetryRegistry {
    state: Arc<RwLock<State>>,
    max_events: usize,
}

impl Default for TelemetryRegistry {
    fn default() -> Self {
        Self::new(2048)
    }
}

impl TelemetryRegistry {
    pub fn new(max_events: usize) -> Self {
        Self {
            state: Arc::new(RwLock::new(State::default())),
            max_events: max_events.max(1),
        }
    }

    pub fn increment(&self, metric: impl Into<String>, by: u64) {
        let metric = metric.into();
        let mut state = self.state.write().expect("telemetry lock poisoned");
        *state.counters.entry(metric).or_insert(0) = state
            .counters
            .get(&metric)
            .copied()
            .unwrap_or(0)
            .saturating_add(by);
    }

    pub fn set_gauge(&self, metric: impl Into<String>, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut state = self.state.write().expect("telemetry lock poisoned");
        state.gauges.insert(metric.into(), value);
    }

    pub fn observe(&self, metric: impl Into<String>, value: f64) {
        let mut state = self.state.write().expect("telemetry lock poisoned");
        state
            .histograms
            .entry(metric.into())
            .or_default()
            .observe(value);
    }

    pub fn event(
        &self,
        name: impl Into<String>,
        attributes: BTreeMap<String, Value>,
    ) {
        let mut state = self.state.write().expect("telemetry lock poisoned");
        state.events.push(TelemetryEvent {
            timestamp: Utc::now(),
            name: name.into(),
            attributes,
        });
        if state.events.len() > self.max_events {
            let excess = state.events.len() - self.max_events;
            state.events.drain(0..excess);
        }
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let state = self.state.read().expect("telemetry lock poisoned");
        TelemetrySnapshot {
            generated_at: Utc::now(),
            counters: state.counters.clone(),
            gauges: state.gauges.clone(),
            histograms: state
                .histograms
                .iter()
                .map(|(name, histogram)| (name.clone(), histogram.snapshot()))
                .collect(),
            recent_events: state.events.clone(),
        }
    }

    pub fn reset(&self) {
        let mut state = self.state.write().expect("telemetry lock poisoned");
        *state = State::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_metrics_and_bounded_events() {
        let registry = TelemetryRegistry::new(2);
        registry.increment("rpc.calls", 1);
        registry.increment("rpc.calls", 2);
        registry.set_gauge("workers.active", 3.0);
        registry.observe("rpc.ms", 10.0);
        registry.observe("rpc.ms", 20.0);
        registry.event("a", BTreeMap::new());
        registry.event("b", BTreeMap::new());
        registry.event("c", BTreeMap::new());

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.counters["rpc.calls"], 3);
        assert_eq!(snapshot.gauges["workers.active"], 3.0);
        assert_eq!(snapshot.histograms["rpc.ms"].average, Some(15.0));
        assert_eq!(snapshot.recent_events.len(), 2);
        assert_eq!(snapshot.recent_events[0].name, "b");
    }
}
