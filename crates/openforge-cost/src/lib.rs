use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

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
    reserved: f64,
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
        Self::with_usage(limits, 0.0, 0.0, 0.0)
    }
    pub fn with_usage(limits: BudgetLimits, task: f64, run: f64, daily: f64) -> Result<Self> {
        for value in [
            limits.per_call,
            limits.per_task,
            limits.per_run,
            limits.daily,
            task,
            run,
            daily,
        ] {
            if !value.is_finite() || value < 0.0 {
                bail!("invalid budget limit or usage");
            }
        }
        if task > limits.per_task || run > limits.per_run || daily > limits.daily {
            bail!("existing usage exceeds budget limit");
        }
        Ok(Self {
            limits,
            usage: Arc::new(Mutex::new(Usage {
                task,
                run,
                daily,
                reserved: 0.0,
            })),
        })
    }
    pub async fn reserve(&self, estimated: f64) -> Result<Reservation> {
        if estimated < 0.0 || !estimated.is_finite() {
            bail!("invalid estimate");
        }
        let mut usage = self.usage.lock().expect("budget mutex poisoned");
        let committed = usage.reserved + estimated;
        if estimated > self.limits.per_call
            || usage.task + committed > self.limits.per_task
            || usage.run + committed > self.limits.per_run
            || usage.daily + committed > self.limits.daily
        {
            bail!("budget exceeded");
        }
        usage.reserved += estimated;
        Ok(Reservation {
            guard: self.clone(),
            estimated,
            settled: false,
        })
    }
    /// Settled spend only; outstanding reservations still constrain new calls.
    pub async fn snapshot(&self) -> (f64, f64, f64) {
        let usage = self.usage.lock().expect("budget mutex poisoned");
        (usage.task, usage.run, usage.daily)
    }
}
impl Reservation {
    pub async fn settle(mut self, actual: f64) -> Result<()> {
        if actual < 0.0 || !actual.is_finite() {
            bail!("invalid actual cost");
        }
        let mut usage = self.guard.usage.lock().expect("budget mutex poisoned");
        let remaining = (usage.reserved - self.estimated).max(0.0);
        if actual > self.guard.limits.per_call
            || usage.task + remaining + actual > self.guard.limits.per_task
            || usage.run + remaining + actual > self.guard.limits.per_run
            || usage.daily + remaining + actual > self.guard.limits.daily
        {
            bail!("settlement would exceed hard budget");
        }
        usage.reserved = remaining;
        usage.task += actual;
        usage.run += actual;
        usage.daily += actual;
        self.settled = true;
        Ok(())
    }
    pub fn estimated(&self) -> f64 {
        self.estimated
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.settled {
            let mut usage = self.guard.usage.lock().expect("budget mutex poisoned");
            usage.reserved = (usage.reserved - self.estimated).max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn guard() -> BudgetGuard {
        BudgetGuard::new(BudgetLimits {
            per_call: 10.0,
            per_task: 10.0,
            per_run: 10.0,
            daily: 10.0,
        })
        .unwrap()
    }
    #[tokio::test]
    async fn reservations_prevent_oversubscription_and_release_on_drop() {
        let guard = guard();
        let first = guard.reserve(7.0).await.unwrap();
        assert!(guard.clone().reserve(4.0).await.is_err());
        drop(first);
        guard
            .reserve(10.0)
            .await
            .unwrap()
            .settle(8.0)
            .await
            .unwrap();
        assert_eq!(guard.snapshot().await, (8.0, 8.0, 8.0));
        assert!(guard.reserve(3.0).await.is_err());
    }
    #[tokio::test]
    async fn settlement_respects_other_pending_calls() {
        let guard = guard();
        let first = guard.reserve(5.0).await.unwrap();
        let second = guard.reserve(5.0).await.unwrap();
        assert!(first.settle(6.0).await.is_err());
        second.settle(4.0).await.unwrap();
        guard.reserve(6.0).await.unwrap().settle(6.0).await.unwrap();
        assert_eq!(guard.snapshot().await, (10.0, 10.0, 10.0));
    }
}
