use anyhow::{bail, Result};
use openforge_protocol::{TaskNode, TaskStatus};
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

pub fn validate_dag(tasks: &[TaskNode]) -> Result<()> {
    let ids: HashSet<Uuid> = tasks.iter().map(|t| t.id).collect();
    for task in tasks {
        if task.dependencies.iter().any(|d| *d == task.id) { bail!("task {} depends on itself",task.id); }
        for dep in &task.dependencies {
            if !ids.contains(dep) { bail!("task {} references unknown dependency {}",task.id,dep); }
        }
    }
    let mut indegree: HashMap<Uuid, usize> = tasks.iter().map(|t|(t.id,t.dependencies.len())).collect();
    let mut outgoing: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for t in tasks { for d in &t.dependencies { outgoing.entry(*d).or_default().push(t.id); } }
    let mut q: VecDeque<Uuid> = indegree.iter().filter(|(_,d)|**d==0).map(|(id,_)|*id).collect();
    let mut visited=0usize;
    while let Some(id)=q.pop_front() {
        visited += 1;
        if let Some(children)=outgoing.get(&id) {
            for child in children {
                let d=indegree.get_mut(child).expect("known child");
                *d-=1;
                if *d==0 { q.push_back(*child); }
            }
        }
    }
    if visited != tasks.len() { bail!("task graph contains a cycle"); }
    Ok(())
}

pub fn runnable_tasks(tasks: &[TaskNode]) -> Result<Vec<TaskNode>> {
    validate_dag(tasks)?;
    let statuses: HashMap<Uuid,TaskStatus> = tasks.iter().map(|t|(t.id,t.status)).collect();
    Ok(tasks.iter().filter(|t| {
        matches!(t.status,TaskStatus::Pending|TaskStatus::Ready|TaskStatus::Failed)
        && t.attempts < t.max_attempts
        && t.dependencies.iter().all(|d| statuses.get(d)==Some(&TaskStatus::Completed))
    }).cloned().collect())
}

pub fn blocked_tasks(tasks:&[TaskNode])->Vec<Uuid>{
    let statuses:HashMap<Uuid,TaskStatus>=tasks.iter().map(|t|(t.id,t.status)).collect();
    tasks.iter().filter(|t| {
        matches!(t.status,TaskStatus::Pending|TaskStatus::Ready)
        && t.dependencies.iter().any(|d| matches!(statuses.get(d),Some(TaskStatus::Failed|TaskStatus::Cancelled|TaskStatus::Blocked)))
    }).map(|t|t.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use openforge_protocol::{TaskBudget};
    fn task(id:Uuid,deps:Vec<Uuid>)->TaskNode{TaskNode{
        id,run_id:Uuid::new_v4(),title:"t".into(),description:"".into(),role:"backend".into(),
        dependencies:deps,required_reviews:vec![],acceptance:vec![],status:TaskStatus::Pending,
        attempts:0,max_attempts:2,budget:TaskBudget{max_usd:1.0,max_model_calls:5,max_tool_calls:20,max_wall_seconds:60},
        created_at:Utc::now(),updated_at:Utc::now()
    }}
    #[test]
    fn detects_cycle(){let a=Uuid::new_v4();let b=Uuid::new_v4();let mut ta=task(a,vec![b]);let tb=task(b,vec![a]);ta.run_id=tb.run_id;assert!(validate_dag(&[ta,tb]).is_err());}
}
