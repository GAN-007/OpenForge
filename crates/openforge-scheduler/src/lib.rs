use anyhow::{bail, Result};
use openforge_protocol::{TaskNode, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub max_parallel: usize,
    pub max_wave_cost_usd: f64,
    pub retry_failed: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            max_wave_cost_usd: f64::INFINITY,
            retry_failed: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleWave {
    pub tasks: Vec<Uuid>,
    pub reserved_cost_usd: f64,
    pub critical_depth: usize,
}

pub fn validate_dag(tasks: &[TaskNode]) -> Result<()> {
    let ids: HashSet<Uuid> = tasks.iter().map(|task| task.id).collect();

    for task in tasks {
        if task.dependencies.iter().any(|dependency| *dependency == task.id) {
            bail!("task {} depends on itself", task.id);
        }

        let mut unique = HashSet::new();
        for dependency in &task.dependencies {
            if !ids.contains(dependency) {
                bail!(
                    "task {} references unknown dependency {}",
                    task.id,
                    dependency
                );
            }
            if !unique.insert(*dependency) {
                bail!(
                    "task {} repeats dependency {}",
                    task.id,
                    dependency
                );
            }
        }
    }

    let mut indegree: HashMap<Uuid, usize> = tasks
        .iter()
        .map(|task| (task.id, task.dependencies.len()))
        .collect();
    let mut outgoing: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

    for task in tasks {
        for dependency in &task.dependencies {
            outgoing.entry(*dependency).or_default().push(task.id);
        }
    }

    for children in outgoing.values_mut() {
        children.sort();
    }

    let mut queue: VecDeque<Uuid> = {
        let mut roots: Vec<Uuid> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(id, _)| *id)
            .collect();
        roots.sort();
        roots.into()
    };

    let mut visited = 0usize;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        if let Some(children) = outgoing.get(&id) {
            for child in children {
                let degree = indegree.get_mut(child).expect("known child");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(*child);
                }
            }
        }
    }

    if visited != tasks.len() {
        bail!("task graph contains a cycle");
    }

    Ok(())
}

pub fn topological_order(tasks: &[TaskNode]) -> Result<Vec<Uuid>> {
    validate_dag(tasks)?;
    let mut indegree: HashMap<Uuid, usize> = tasks
        .iter()
        .map(|task| (task.id, task.dependencies.len()))
        .collect();
    let mut outgoing: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

    for task in tasks {
        for dependency in &task.dependencies {
            outgoing.entry(*dependency).or_default().push(task.id);
        }
    }

    for children in outgoing.values_mut() {
        children.sort();
    }

    let mut ready: Vec<Uuid> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    ready.sort();

    let mut order = Vec::with_capacity(tasks.len());
    while let Some(id) = ready.first().copied() {
        ready.remove(0);
        order.push(id);

        if let Some(children) = outgoing.get(&id) {
            for child in children {
                let degree = indegree.get_mut(child).expect("known child");
                *degree -= 1;
                if *degree == 0 {
                    ready.push(*child);
                    ready.sort();
                }
            }
        }
    }

    Ok(order)
}

pub fn critical_depths(tasks: &[TaskNode]) -> Result<HashMap<Uuid, usize>> {
    let order = topological_order(tasks)?;
    let by_id: HashMap<Uuid, &TaskNode> =
        tasks.iter().map(|task| (task.id, task)).collect();
    let mut children: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

    for task in tasks {
        for dependency in &task.dependencies {
            children.entry(*dependency).or_default().push(task.id);
        }
    }

    let mut depth = HashMap::new();
    for id in order.into_iter().rev() {
        let child_depth = children
            .get(&id)
            .into_iter()
            .flatten()
            .map(|child| *depth.get(child).unwrap_or(&0usize))
            .max()
            .unwrap_or(0);
        if by_id.contains_key(&id) {
            depth.insert(id, child_depth + 1);
        }
    }

    Ok(depth)
}

pub fn runnable_tasks(tasks: &[TaskNode]) -> Result<Vec<TaskNode>> {
    validate_dag(tasks)?;
    let statuses: HashMap<Uuid, TaskStatus> =
        tasks.iter().map(|task| (task.id, task.status)).collect();
    let depths = critical_depths(tasks)?;

    let mut runnable: Vec<TaskNode> = tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Failed
            ) && task.attempts < task.max_attempts
                && task.dependencies.iter().all(|dependency| {
                    statuses.get(dependency) == Some(&TaskStatus::Completed)
                })
        })
        .cloned()
        .collect();

    runnable.sort_by(|left, right| {
        depths
            .get(&right.id)
            .copied()
            .unwrap_or(0)
            .cmp(&depths.get(&left.id).copied().unwrap_or(0))
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(runnable)
}

pub fn schedule_wave(
    tasks: &[TaskNode],
    config: &SchedulerConfig,
) -> Result<ScheduleWave> {
    if config.max_parallel == 0 {
        bail!("max_parallel must be greater than zero");
    }
    if config.max_wave_cost_usd.is_nan() || config.max_wave_cost_usd < 0.0 {
        bail!("max_wave_cost_usd must be non-negative");
    }

    let depths = critical_depths(tasks)?;
    let runnable = runnable_tasks(tasks)?;
    let mut selected = Vec::new();
    let mut reserved = 0.0;

    for task in runnable {
        if !config.retry_failed && task.status == TaskStatus::Failed {
            continue;
        }
        if selected.len() >= config.max_parallel {
            break;
        }

        let task_cost = task.budget.max_usd.max(0.0);
        if reserved + task_cost > config.max_wave_cost_usd {
            continue;
        }

        reserved += task_cost;
        selected.push(task.id);
    }

    let critical_depth = selected
        .iter()
        .filter_map(|id| depths.get(id).copied())
        .max()
        .unwrap_or(0);

    Ok(ScheduleWave {
        tasks: selected,
        reserved_cost_usd: reserved,
        critical_depth,
    })
}

pub fn blocked_tasks(tasks: &[TaskNode]) -> Vec<Uuid> {
    let statuses: HashMap<Uuid, TaskStatus> =
        tasks.iter().map(|task| (task.id, task.status)).collect();

    tasks
        .iter()
        .filter(|task| {
            matches!(task.status, TaskStatus::Pending | TaskStatus::Ready)
                && task.dependencies.iter().any(|dependency| {
                    matches!(
                        statuses.get(dependency),
                        Some(
                            TaskStatus::Failed
                                | TaskStatus::Cancelled
                                | TaskStatus::Blocked
                        )
                    )
                })
        })
        .map(|task| task.id)
        .collect()
}

pub fn dependency_closure(tasks: &[TaskNode], task_id: Uuid) -> Result<Vec<Uuid>> {
    validate_dag(tasks)?;
    let by_id: HashMap<Uuid, &TaskNode> =
        tasks.iter().map(|task| (task.id, task)).collect();
    if !by_id.contains_key(&task_id) {
        bail!("unknown task {}", task_id);
    }

    let mut stack = vec![task_id];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        let task = by_id.get(&id).expect("validated task");
        for dependency in &task.dependencies {
            if seen.insert(*dependency) {
                stack.push(*dependency);
            }
        }
    }

    let order = topological_order(tasks)?;
    Ok(order
        .into_iter()
        .filter(|id| seen.contains(id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use openforge_protocol::TaskBudget;

    fn task(id: Uuid, run_id: Uuid, deps: Vec<Uuid>, cost: f64) -> TaskNode {
        TaskNode {
            id,
            run_id,
            title: "task".into(),
            description: String::new(),
            role: "backend".into(),
            dependencies: deps,
            required_reviews: vec![],
            acceptance: vec![],
            status: TaskStatus::Pending,
            attempts: 0,
            max_attempts: 2,
            budget: TaskBudget {
                max_usd: cost,
                max_model_calls: 5,
                max_tool_calls: 20,
                max_wall_seconds: 60,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn detects_cycle() {
        let run = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let first = task(a, run, vec![b], 1.0);
        let second = task(b, run, vec![a], 1.0);
        assert!(validate_dag(&[first, second]).is_err());
    }

    #[test]
    fn wave_respects_parallel_and_budget_limits() {
        let run = Uuid::new_v4();
        let tasks = vec![
            task(Uuid::new_v4(), run, vec![], 1.0),
            task(Uuid::new_v4(), run, vec![], 1.0),
            task(Uuid::new_v4(), run, vec![], 1.0),
        ];
        let wave = schedule_wave(
            &tasks,
            &SchedulerConfig {
                max_parallel: 3,
                max_wave_cost_usd: 2.0,
                retry_failed: true,
            },
        )
        .unwrap();
        assert_eq!(wave.tasks.len(), 2);
        assert_eq!(wave.reserved_cost_usd, 2.0);
    }
}
