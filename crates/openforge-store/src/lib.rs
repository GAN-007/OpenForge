use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use openforge_protocol::{Actor, EventEnvelope, Run, RunStatus, TaskNode};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path).context("open SQLite state database")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS runs (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              objective TEXT NOT NULL,
              base_sha TEXT NOT NULL,
              status TEXT NOT NULL,
              autonomy TEXT NOT NULL,
              budget_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
              id TEXT PRIMARY KEY,
              run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
              task_json TEXT NOT NULL,
              status TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_run ON tasks(run_id);

            CREATE TABLE IF NOT EXISTS events (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,
              event_id TEXT NOT NULL UNIQUE,
              run_id TEXT,
              task_id TEXT,
              timestamp TEXT NOT NULL,
              actor_json TEXT NOT NULL,
              event_type TEXT NOT NULL,
              payload_json TEXT NOT NULL,
              previous_event_hash TEXT,
              event_hash TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_events_run_sequence
              ON events(run_id, sequence);

            CREATE TABLE IF NOT EXISTS cost_ledger (
              id TEXT PRIMARY KEY,
              run_id TEXT NOT NULL,
              task_id TEXT,
              agent_id TEXT,
              provider TEXT NOT NULL,
              model TEXT NOT NULL,
              amount_usd REAL NOT NULL CHECK(amount_usd >= 0),
              input_tokens INTEGER NOT NULL DEFAULT 0,
              output_tokens INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cost_run ON cost_ledger(run_id);

            CREATE TABLE IF NOT EXISTS memory (
              id TEXT PRIMARY KEY,
              scope TEXT NOT NULL,
              project_id TEXT,
              repository_id TEXT,
              key TEXT NOT NULL,
              value_json TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              UNIQUE(scope, project_id, repository_id, key)
            );
            "#,
        )?;
        Ok(())
    }

    pub fn create_run(&self, run: &Run) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO runs(
                id, project_id, objective, base_sha, status, autonomy,
                budget_json, created_at, updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                run.id.to_string(),
                run.project_id.to_string(),
                &run.objective,
                &run.base_sha,
                serde_json::to_string(&run.status)?,
                serde_json::to_string(&run.autonomy)?,
                serde_json::to_string(&run.budget)?,
                run.created_at.to_rfc3339(),
                run.updated_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn update_run_status(&self, id: Uuid, status: RunStatus) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "UPDATE runs SET status=?2, updated_at=?3 WHERE id=?1",
            params![
                id.to_string(),
                serde_json::to_string(&status)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_run(&self, id: Uuid) -> Result<Option<Run>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let row = conn
            .query_row(
                "SELECT project_id,objective,base_sha,status,autonomy,
                        budget_json,created_at,updated_at
                 FROM runs WHERE id=?1",
                [id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;

        row.map(
            |(
                project_id,
                objective,
                base_sha,
                status,
                autonomy,
                budget,
                created,
                updated,
            )|
             -> Result<Run> {
                Ok(Run {
                    id,
                    project_id: Uuid::parse_str(&project_id)?,
                    objective,
                    base_sha,
                    status: serde_json::from_str(&status)?,
                    autonomy: serde_json::from_str(&autonomy)?,
                    budget: serde_json::from_str(&budget)?,
                    created_at: DateTime::parse_from_rfc3339(&created)?
                        .with_timezone(&Utc),
                    updated_at: DateTime::parse_from_rfc3339(&updated)?
                        .with_timezone(&Utc),
                })
            },
        )
        .transpose()
    }

    pub fn upsert_task(&self, task: &TaskNode) -> Result<()> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO tasks(id,run_id,task_json,status,updated_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET
               task_json=excluded.task_json,
               status=excluded.status,
               updated_at=excluded.updated_at",
            params![
                task.id.to_string(),
                task.run_id.to_string(),
                serde_json::to_string(task)?,
                serde_json::to_string(&task.status)?,
                task.updated_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn list_tasks(&self, run_id: Uuid) -> Result<Vec<TaskNode>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt =
            conn.prepare("SELECT task_json FROM tasks WHERE run_id=?1 ORDER BY rowid")?;
        let rows =
            stmt.query_map([run_id.to_string()], |row| row.get::<_, String>(0))?;

        rows.map(|row| Ok(serde_json::from_str::<TaskNode>(&row?)?))
            .collect()
    }

    pub fn append_event(
        &self,
        run_id: Option<Uuid>,
        task_id: Option<Uuid>,
        actor: Actor,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Result<EventEnvelope> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;

        let previous: Option<String> = tx
            .query_row(
                "SELECT event_hash FROM events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        let timestamp = Utc::now();
        let event_id = Uuid::now_v7();
        let event_type = event_type.into();

        let canonical = serde_json::json!({
            "event_id": event_id,
            "run_id": run_id,
            "task_id": task_id,
            "timestamp": timestamp,
            "actor": &actor,
            "event_type": &event_type,
            "payload": &payload,
            "previous_event_hash": &previous
        });
        let event_hash =
            hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?));

        tx.execute(
            "INSERT INTO events(
                event_id,run_id,task_id,timestamp,actor_json,event_type,
                payload_json,previous_event_hash,event_hash
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                event_id.to_string(),
                run_id.map(|value| value.to_string()),
                task_id.map(|value| value.to_string()),
                timestamp.to_rfc3339(),
                serde_json::to_string(&actor)?,
                &event_type,
                serde_json::to_string(&payload)?,
                &previous,
                &event_hash
            ],
        )?;

        let sequence = tx.last_insert_rowid();
        tx.commit()?;

        Ok(EventEnvelope {
            event_id,
            sequence,
            run_id,
            task_id,
            timestamp,
            actor,
            event_type,
            payload,
            previous_event_hash: previous,
            event_hash,
        })
    }

    pub fn list_events(
        &self,
        run_id: Uuid,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT sequence,event_id,task_id,timestamp,actor_json,event_type,
                    payload_json,previous_event_hash,event_hash
             FROM events
             WHERE run_id=?1 AND sequence>?2
             ORDER BY sequence
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(
            params![run_id.to_string(), after_sequence, limit as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )?;

        let mut events = Vec::new();
        for row in rows {
            let (
                sequence,
                event_id,
                task_id,
                timestamp,
                actor,
                event_type,
                payload,
                previous,
                event_hash,
            ) = row?;

            events.push(EventEnvelope {
                event_id: Uuid::parse_str(&event_id)?,
                sequence,
                run_id: Some(run_id),
                task_id: task_id
                    .map(|value| Uuid::parse_str(&value))
                    .transpose()?,
                timestamp: DateTime::parse_from_rfc3339(&timestamp)?
                    .with_timezone(&Utc),
                actor: serde_json::from_str(&actor)?,
                event_type,
                payload: serde_json::from_str(&payload)?,
                previous_event_hash: previous,
                event_hash,
            });
        }
        Ok(events)
    }

    pub fn list_all_events(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT sequence,event_id,run_id,task_id,timestamp,actor_json,event_type,
                    payload_json,previous_event_hash,event_hash
             FROM events
             WHERE sequence>?1
             ORDER BY sequence
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(
            params![after_sequence, limit.min(100_000) as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )?;

        let mut events = Vec::new();
        for row in rows {
            let (
                sequence,
                event_id,
                run_id,
                task_id,
                timestamp,
                actor,
                event_type,
                payload,
                previous,
                event_hash,
            ) = row?;

            events.push(EventEnvelope {
                event_id: Uuid::parse_str(&event_id)?,
                sequence,
                run_id: run_id.map(|value| Uuid::parse_str(&value)).transpose()?,
                task_id: task_id.map(|value| Uuid::parse_str(&value)).transpose()?,
                timestamp: DateTime::parse_from_rfc3339(&timestamp)?
                    .with_timezone(&Utc),
                actor: serde_json::from_str(&actor)?,
                event_type,
                payload: serde_json::from_str(&payload)?,
                previous_event_hash: previous,
                event_hash,
            });
        }
        Ok(events)
    }

    pub fn record_cost(
        &self,
        run_id: Uuid,
        task_id: Option<Uuid>,
        agent_id: Option<&str>,
        provider: &str,
        model: &str,
        amount_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<()> {
        if !amount_usd.is_finite() || amount_usd < 0.0 {
            anyhow::bail!("invalid cost");
        }

        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO cost_ledger(
                id,run_id,task_id,agent_id,provider,model,amount_usd,
                input_tokens,output_tokens,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                Uuid::now_v7().to_string(),
                run_id.to_string(),
                task_id.map(|value| value.to_string()),
                agent_id,
                provider,
                model,
                amount_usd,
                input_tokens as i64,
                output_tokens as i64,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn run_cost(&self, run_id: Uuid) -> Result<f64> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(amount_usd),0)
             FROM cost_ledger
             WHERE run_id=?1",
            [run_id.to_string()],
            |row| row.get(0),
        )?)
    }

    pub fn memory_put(
        &self,
        scope: &str,
        project_id: Option<Uuid>,
        repository_id: Option<&str>,
        key: &str,
        value: &Value,
    ) -> Result<()> {
        validate_memory_scope(scope)?;
        if key.trim().is_empty() {
            anyhow::bail!("memory key cannot be empty");
        }

        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO memory(
                id,scope,project_id,repository_id,key,value_json,created_at,updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(scope,project_id,repository_id,key)
             DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at",
            params![
                Uuid::new_v4().to_string(),
                scope,
                project_id.map(|value| value.to_string()),
                repository_id,
                key,
                serde_json::to_string(value)?,
                &now,
                &now
            ],
        )?;
        Ok(())
    }

    pub fn memory_search(
        &self,
        scope: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Value>> {
        if let Some(scope) = scope {
            validate_memory_scope(scope)?;
        }

        let pattern = format!("%{}%", escape_like(query));
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id,scope,project_id,repository_id,key,value_json,created_at,updated_at
             FROM memory
             WHERE (?1 IS NULL OR scope=?1)
               AND (key LIKE ?2 ESCAPE '\' OR value_json LIKE ?2 ESCAPE '\')
             ORDER BY updated_at DESC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(
            params![scope, pattern, limit.min(1000) as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )?;

        let mut result = Vec::new();
        for row in rows {
            let (
                id,
                scope,
                project_id,
                repository_id,
                key,
                value_json,
                created_at,
                updated_at,
            ) = row?;
            result.push(serde_json::json!({
                "id": id,
                "scope": scope,
                "project_id": project_id,
                "repository_id": repository_id,
                "key": key,
                "value": serde_json::from_str::<Value>(&value_json)?,
                "created_at": created_at,
                "updated_at": updated_at
            }));
        }
        Ok(result)
    }

    pub fn memory_delete(
        &self,
        scope: Option<&str>,
        key: &str,
    ) -> Result<usize> {
        if let Some(scope) = scope {
            validate_memory_scope(scope)?;
        }
        let conn = self.conn.lock().expect("store mutex poisoned");
        let changed = conn.execute(
            "DELETE FROM memory WHERE (?1 IS NULL OR scope=?1) AND key=?2",
            params![scope, key],
        )?;
        Ok(changed)
    }
}

fn validate_memory_scope(scope: &str) -> Result<()> {
    if matches!(
        scope,
        "session" | "project" | "repository" | "user_rules" | "agent_experience"
    ) {
        Ok(())
    } else {
        anyhow::bail!("unsupported memory scope {scope}");
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\', "\\")
        .replace('%', "\%")
        .replace('_', "\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_chain_is_hash_linked() {
        let store = Store::in_memory().unwrap();

        let first = store
            .append_event(
                None,
                None,
                Actor {
                    kind: "system".into(),
                    id: "test".into(),
                },
                "first",
                serde_json::json!({"x": 1}),
            )
            .unwrap();

        let second = store
            .append_event(
                None,
                None,
                Actor {
                    kind: "system".into(),
                    id: "test".into(),
                },
                "second",
                serde_json::json!({"x": 2}),
            )
            .unwrap();

        assert_eq!(
            second.previous_event_hash.as_deref(),
            Some(first.event_hash.as_str())
        );
        assert_ne!(first.event_hash, second.event_hash);
    }

    #[test]
    fn memory_is_explicit_and_deletable() {
        let store = Store::in_memory().unwrap();
        store
            .memory_put(
                "project",
                None,
                Some("repo"),
                "architecture",
                &serde_json::json!({"decision": "daemon-first"}),
            )
            .unwrap();

        let found = store.memory_search(Some("project"), "daemon", 10).unwrap();
        assert_eq!(found.len(), 1);

        assert_eq!(
            store
                .memory_delete(Some("project"), "architecture")
                .unwrap(),
            1
        );
    }
}
