use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimits {
    pub per_call: f64,
    pub per_task: f64,
    pub per_run: f64,
    pub daily: f64,
}
#[derive(Debug, Default)]
struct Usage {
    task: f64,
    run: f64,
    daily: f64,
}
#[derive(Clone)]
pub struct BudgetGuard {
    limits: BudgetLimits,
    usage: Arc<Mutex<Usage>>,
}
pub struct Reservation {
    guard: BudgetGuard,
    estimated: f64,
    settled: bool,
}
impl BudgetGuard {
    pub fn new(limits: BudgetLimits) -> Result<Self> {
        for v in [
            limits.per_call,
            limits.per_task,
            limits.per_run,
            limits.daily,
        ] {
            if !v.is_finite() || v < 0.0 {
                bail!("invalid budget limit")
            }
        }
        Ok(Self {
            limits,
            usage: Arc::new(Mutex::new(Usage::default())),
        })
    }
    pub async fn reserve(&self, estimated: f64) -> Result<Reservation> {
        if estimated < 0.0 || !estimated.is_finite() {
            bail!("invalid estimate")
        }
        let u = self.usage.lock().await;
        if estimated > self.limits.per_call
            || u.task + estimated > self.limits.per_task
            || u.run + estimated > self.limits.per_run
            || u.daily + estimated > self.limits.daily
        {
            bail!("budget exceeded")
        }
        drop(u);
        Ok(Reservation {
            guard: self.clone(),
            estimated,
            settled: false,
        })
    }
    pub async fn snapshot(&self) -> (f64, f64, f64) {
        let u = self.usage.lock().await;
        (u.task, u.run, u.daily)
    }
}
impl Reservation {
    pub async fn settle(mut self, actual: f64) -> Result<()> {
        if actual < 0.0 || !actual.is_finite() {
            bail!("invalid actual cost")
        }
        if actual > self.guard.limits.per_call {
            bail!("actual call cost exceeds hard limit")
        }
        let mut u = self.guard.usage.lock().await;
        if u.task + actual > self.guard.limits.per_task
            || u.run + actual > self.guard.limits.per_run
            || u.daily + actual > self.guard.limits.daily
        {
            bail!("settlement would exceed hard budget")
        }
        u.task += actual;
        u.run += actual;
        u.daily += actual;
        self.settled = true;
        Ok(())
    }
    pub fn estimated(&self) -> f64 {
        self.estimated
    }
}
