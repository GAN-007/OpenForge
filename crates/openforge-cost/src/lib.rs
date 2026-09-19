use anyhow::{bail, Result};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub task_spent: f64,
    pub run_spent: f64,
    pub daily_spent: f64,
    pub reserved: f64,
    pub limits: BudgetLimits,
}

#[derive(Debug, Default)]
struct Usage {
    task_spent: f64,
    run_spent: f64,
    daily_spent: f64,
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
    finished: bool,
}

impl BudgetGuard {
    pub fn new(limits: BudgetLimits) -> Result<Self> {
        Self::with_usage(limits, 0.0, 0.0, 0.0, 0.0)
    }

    pub fn with_usage(
        limits: BudgetLimits,
        task_spent: f64,
        run_spent: f64,
        daily_spent: f64,
        reserved: f64,
    ) -> Result<Self> {
        validate_limits(&limits)?;
        for value in [task_spent, run_spent, daily_spent, reserved] {
            if !value.is_finite() || value < 0.0 {
                bail!("invalid budget usage");
            }
        }
        if task_spent + reserved > limits.per_task
            || run_spent + reserved > limits.per_run
            || daily_spent + reserved > limits.daily
        {
            bail!("existing usage exceeds budget limits");
        }
        Ok(Self {
            limits,
            usage: Arc::new(Mutex::new(Usage {
                task_spent,
                run_spent,
                daily_spent,
                reserved,
            })),
        })
    }

    pub fn limits(&self) -> &BudgetLimits {
        &self.limits
    }

    pub async fn reserve(&self, estimated: f64) -> Result<Reservation> {
        if !estimated.is_finite() || estimated < 0.0 {
            bail!("invalid estimate");
        }
        let mut usage = self.usage.lock().await;
        if estimated > self.limits.per_call
            || usage.task_spent + usage.reserved + estimated > self.limits.per_task
            || usage.run_spent + usage.reserved + estimated > self.limits.per_run
            || usage.daily_spent + usage.reserved + estimated > self.limits.daily
        {
            bail!("budget exceeded");
        }
        usage.reserved += estimated;
        drop(usage);
        Ok(Reservation {
            guard: self.clone(),
            estimated,
            finished: false,
        })
    }

    pub async fn snapshot(&self) -> (f64, f64, f64) {
        let usage = self.usage.lock().await;
        (usage.task_spent, usage.run_spent, usage.daily_spent)
    }

    pub async fn detailed_snapshot(&self) -> BudgetSnapshot {
        let usage = self.usage.lock().await;
        BudgetSnapshot {
            task_spent: usage.task_spent,
            run_spent: usage.run_spent,
            daily_spent: usage.daily_spent,
            reserved: usage.reserved,
            limits: self.limits.clone(),
        }
    }
}

impl Reservation {
    pub async fn settle(mut self, actual: f64) -> Result<()> {
        if !actual.is_finite() || actual < 0.0 {
            self.release().await;
            bail!("invalid actual cost");
        }
        let mut usage = self.guard.usage.lock().await;
        let remaining_reserved = (usage.reserved - self.estimated).max(0.0);
        if actual > self.guard.limits.per_call
            || usage.task_spent + remaining_reserved + actual > self.guard.limits.per_task
            || usage.run_spent + remaining_reserved + actual > self.guard.limits.per_run
            || usage.daily_spent + remaining_reserved + actual > self.guard.limits.daily
        {
            usage.reserved = remaining_reserved;
            self.finished = true;
            bail!("settlement would exceed hard budget");
        }
        usage.reserved = remaining_reserved;
        usage.task_spent += actual;
        usage.run_spent += actual;
        usage.daily_spent += actual;
        self.finished = true;
        Ok(())
    }

    pub async fn cancel(mut self) {
        self.release().await;
    }

    pub fn estimated(&self) -> f64 {
        self.estimated
    }

    async fn release(&mut self) {
        if self.finished {
            return;
        }
        let mut usage = self.guard.usage.lock().await;
        usage.reserved = (usage.reserved - self.estimated).max(0.0);
        self.finished = true;
    }
}

fn validate_limits(limits: &BudgetLimits) -> Result<()> {
    for value in [limits.per_call, limits.per_task, limits.per_run, limits.daily] {
        if !value.is_finite() || value < 0.0 {
            bail!("invalid budget limit");
        }
    }
    if limits.per_call > limits.per_task
        || limits.per_task > limits.per_run
        || limits.per_run > limits.daily
    {
        bail!("budget limits must satisfy per_call <= per_task <= per_run <= daily");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_reservations_cannot_oversubscribe() {
        let guard = BudgetGuard::new(BudgetLimits {
            per_call: 5.0,
            per_task: 5.0,
            per_run: 5.0,
            daily: 5.0,
        })
        .unwrap();
        let first = guard.reserve(4.0).await.unwrap();
        assert!(guard.reserve(2.0).await.is_err());
        first.cancel().await;
        assert!(guard.reserve(2.0).await.is_ok());
    }

    #[tokio::test]
    async fn settlement_replaces_reservation_with_actual_usage() {
        let guard = BudgetGuard::new(BudgetLimits {
            per_call: 10.0,
            per_task: 20.0,
            per_run: 30.0,
            daily: 40.0,
        })
        .unwrap();
        let reservation = guard.reserve(5.0).await.unwrap();
        reservation.settle(4.0).await.unwrap();
        let snapshot = guard.detailed_snapshot().await;
        assert_eq!(snapshot.reserved, 0.0);
        assert_eq!(snapshot.run_spent, 4.0);
    }
}
