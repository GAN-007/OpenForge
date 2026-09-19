use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Online,
    Draining,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerDescriptor {
    pub id: Uuid,
    pub name: String,
    pub endpoint: Option<String>,
    pub capabilities: BTreeSet<String>,
    pub labels: BTreeSet<String>,
    pub status: WorkerStatus,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Leased,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "leased" => Ok(Self::Leased),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => bail!("unknown worker job status {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableJob {
    pub id: Uuid,
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub status: JobStatus,
    pub payload: Value,
    pub required_capabilities: BTreeSet<String>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLease {
    pub job: DurableJob,
    pub worker_id: Uuid,
    pub lease_token: String,
    pub leased_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCheckpoint {
    pub job_id: Uuid,
    pub sequence: i64,
    pub state: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct WorkerStore {
    conn: Arc<Mutex<Connection>>,
}

impl WorkerStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workers(
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                endpoint TEXT,
                capabilities_json TEXT NOT NULL,
                labels_json TEXT NOT NULL,
                status TEXT NOT NULL,
                registered_at TEXT NOT NULL,
                last_heartbeat_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS jobs(
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                task_id TEXT,
                status TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                required_capabilities_json TEXT NOT NULL,
                attempts INTEGER NOT NULL,
                max_attempts INTEGER NOT NULL,
                lease_worker_id TEXT,
                lease_token TEXT,
                leased_at TEXT,
                lease_expires_at TEXT,
                last_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_jobs_claim
                ON jobs(status, lease_expires_at, created_at);
            CREATE TABLE IF NOT EXISTS job_checkpoints(
                job_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(job_id, sequence)
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn register_worker(
        &self,
        name: &str,
        endpoint: Option<&str>,
        capabilities: BTreeSet<String>,
        labels: BTreeSet<String>,
    ) -> Result<WorkerDescriptor> {
        if name.trim().is_empty() {
            bail!("worker name cannot be empty");
        }

        let worker = WorkerDescriptor {
            id: Uuid::now_v7(),
            name: name.trim().to_string(),
            endpoint: endpoint.map(str::to_string),
            capabilities,
            labels,
            status: WorkerStatus::Online,
            registered_at: Utc::now(),
            last_heartbeat_at: Utc::now(),
        };

        let conn = self.conn.lock().expect("worker store mutex poisoned");
        conn.execute(
            "INSERT INTO workers(
                id,name,endpoint,capabilities_json,labels_json,status,
                registered_at,last_heartbeat_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                worker.id.to_string(),
                worker.name,
                worker.endpoint,
                serde_json::to_string(&worker.capabilities)?,
                serde_json::to_string(&worker.labels)?,
                "online",
                worker.registered_at.to_rfc3339(),
                worker.last_heartbeat_at.to_rfc3339()
            ],
        )?;
        Ok(worker)
    }

    pub fn get_worker(&self, worker_id: Uuid) -> Result<Option<WorkerDescriptor>> {
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        conn.query_row(
            "SELECT name,endpoint,capabilities_json,labels_json,status,
                    registered_at,last_heartbeat_at
             FROM workers WHERE id=?1",
            params![worker_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?
        .map(|(name, endpoint, capabilities, labels, status, registered_at, last_heartbeat_at)| {
            Ok(WorkerDescriptor {
                id: worker_id,
                name,
                endpoint,
                capabilities: serde_json::from_str(&capabilities)?,
                labels: serde_json::from_str(&labels)?,
                status: match status.as_str() {
                    "online" => WorkerStatus::Online,
                    "draining" => WorkerStatus::Draining,
                    "offline" => WorkerStatus::Offline,
                    other => bail!("unknown worker status {other}"),
                },
                registered_at: DateTime::parse_from_rfc3339(&registered_at)?.with_timezone(&Utc),
                last_heartbeat_at: DateTime::parse_from_rfc3339(&last_heartbeat_at)?.with_timezone(&Utc),
            })
        })
        .transpose()
    }

    pub fn heartbeat_worker(&self, worker_id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        Ok(conn.execute(
            "UPDATE workers
             SET last_heartbeat_at=?2,status='online'
             WHERE id=?1 AND status!='offline'",
            params![worker_id.to_string(), Utc::now().to_rfc3339()],
        )? == 1)
    }

    pub fn submit_job(
        &self,
        run_id: Uuid,
        task_id: Option<Uuid>,
        payload: Value,
        required_capabilities: BTreeSet<String>,
        max_attempts: u32,
    ) -> Result<DurableJob> {
        if max_attempts == 0 {
            bail!("job max_attempts must be greater than zero");
        }
        let now = Utc::now();
        let job = DurableJob {
            id: Uuid::now_v7(),
            run_id,
            task_id,
            status: JobStatus::Queued,
            payload,
            required_capabilities,
            attempts: 0,
            max_attempts,
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        conn.execute(
            "INSERT INTO jobs(
                id,run_id,task_id,status,payload_json,required_capabilities_json,
                attempts,max_attempts,created_at,updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,0,?7,?8,?8)",
            params![
                job.id.to_string(),
                job.run_id.to_string(),
                job.task_id.map(|value| value.to_string()),
                job.status.as_str(),
                serde_json::to_string(&job.payload)?,
                serde_json::to_string(&job.required_capabilities)?,
                job.max_attempts,
                now.to_rfc3339()
            ],
        )?;
        Ok(job)
    }

    pub fn claim(
        &self,
        worker_id: Uuid,
        worker_capabilities: &BTreeSet<String>,
        lease_seconds: u64,
    ) -> Result<Option<JobLease>> {
        let now = Utc::now();
        let expires = now
            .checked_add_signed(Duration::seconds(lease_seconds.clamp(5, 86_400) as i64))
            .context("worker lease expiry overflow")?;
        let mut conn = self.conn.lock().expect("worker store mutex poisoned");
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let mut statement = tx.prepare(
            "SELECT id,run_id,task_id,status,payload_json,required_capabilities_json,
                    attempts,max_attempts,created_at,updated_at
             FROM jobs
             WHERE (
                 status='queued'
                 OR (status='leased' AND lease_expires_at IS NOT NULL AND lease_expires_at < ?1)
             )
             AND attempts < max_attempts
             ORDER BY created_at
             LIMIT 128",
        )?;

        let candidates = statement
            .query_map(params![now.to_rfc3339()], parse_job_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let Some(job) = candidates
            .into_iter()
            .find(|job| job.required_capabilities.is_subset(worker_capabilities))
        else {
            tx.commit()?;
            return Ok(None);
        };

        let lease_token = format!(
            "of_lease_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let changed = tx.execute(
            "UPDATE jobs SET
                status='leased',
                attempts=attempts+1,
                lease_worker_id=?2,
                lease_token=?3,
                leased_at=?4,
                lease_expires_at=?5,
                updated_at=?4
             WHERE id=?1
               AND (
                   status='queued'
                   OR (status='leased' AND lease_expires_at IS NOT NULL AND lease_expires_at < ?4)
               )",
            params![
                job.id.to_string(),
                worker_id.to_string(),
                lease_token,
                now.to_rfc3339(),
                expires.to_rfc3339()
            ],
        )?;
        if changed != 1 {
            tx.rollback()?;
            return Ok(None);
        }

        let mut leased_job = job;
        leased_job.status = JobStatus::Leased;
        leased_job.attempts += 1;
        leased_job.updated_at = now;
        tx.commit()?;

        Ok(Some(JobLease {
            job: leased_job,
            worker_id,
            lease_token,
            leased_at: now,
            expires_at: expires,
        }))
    }

    pub fn heartbeat_lease(
        &self,
        job_id: Uuid,
        lease_token: &str,
        lease_seconds: u64,
    ) -> Result<bool> {
        let now = Utc::now();
        let expires = now
            .checked_add_signed(Duration::seconds(lease_seconds.clamp(5, 86_400) as i64))
            .context("worker lease expiry overflow")?;
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        Ok(conn.execute(
            "UPDATE jobs SET lease_expires_at=?3,updated_at=?4
             WHERE id=?1 AND lease_token=?2 AND status='leased'",
            params![
                job_id.to_string(),
                lease_token,
                expires.to_rfc3339(),
                now.to_rfc3339()
            ],
        )? == 1)
    }

    pub fn checkpoint(
        &self,
        job_id: Uuid,
        lease_token: &str,
        sequence: i64,
        state: &Value,
    ) -> Result<JobCheckpoint> {
        if sequence < 0 {
            bail!("checkpoint sequence cannot be negative");
        }
        let now = Utc::now();
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        let active = conn
            .query_row(
                "SELECT 1 FROM jobs WHERE id=?1 AND lease_token=?2 AND status='leased'",
                params![job_id.to_string(), lease_token],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !active {
            bail!("cannot checkpoint without an active matching lease");
        }
        conn.execute(
            "INSERT INTO job_checkpoints(job_id,sequence,state_json,created_at)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(job_id,sequence) DO UPDATE SET
                state_json=excluded.state_json,
                created_at=excluded.created_at",
            params![
                job_id.to_string(),
                sequence,
                serde_json::to_string(state)?,
                now.to_rfc3339()
            ],
        )?;

        Ok(JobCheckpoint {
            job_id,
            sequence,
            state: state.clone(),
            created_at: now,
        })
    }

    pub fn latest_checkpoint(&self, job_id: Uuid) -> Result<Option<JobCheckpoint>> {
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        conn.query_row(
            "SELECT sequence,state_json,created_at
             FROM job_checkpoints
             WHERE job_id=?1
             ORDER BY sequence DESC
             LIMIT 1",
            params![job_id.to_string()],
            |row| {
                let sequence: i64 = row.get(0)?;
                let state_json: String = row.get(1)?;
                let created_at: String = row.get(2)?;
                Ok((sequence, state_json, created_at))
            },
        )
        .optional()?
        .map(|(sequence, state_json, created_at)| {
            Ok(JobCheckpoint {
                job_id,
                sequence,
                state: serde_json::from_str(&state_json)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            })
        })
        .transpose()
    }

    pub fn complete(&self, job_id: Uuid, lease_token: &str, result: &Value) -> Result<bool> {
        self.finish(job_id, lease_token, JobStatus::Completed, Some(result), None)
    }

    pub fn fail(&self, job_id: Uuid, lease_token: &str, error: &str) -> Result<bool> {
        self.finish(job_id, lease_token, JobStatus::Failed, None, Some(error))
    }

    pub fn cancel(&self, job_id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        Ok(conn.execute(
            "UPDATE jobs SET status='cancelled',updated_at=?2
             WHERE id=?1 AND status IN ('queued','leased')",
            params![job_id.to_string(), Utc::now().to_rfc3339()],
        )? == 1)
    }

    pub fn requeue_expired(&self) -> Result<usize> {
        let now = Utc::now();
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        Ok(conn.execute(
            "UPDATE jobs SET
                status=CASE WHEN attempts < max_attempts THEN 'queued' ELSE 'failed' END,
                lease_worker_id=NULL,
                lease_token=NULL,
                leased_at=NULL,
                lease_expires_at=NULL,
                updated_at=?1,
                last_error=CASE WHEN attempts < max_attempts
                    THEN last_error
                    ELSE COALESCE(last_error,'lease expired after final attempt')
                END
             WHERE status='leased'
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < ?1",
            params![now.to_rfc3339()],
        )?)
    }

    fn finish(
        &self,
        job_id: Uuid,
        lease_token: &str,
        status: JobStatus,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<bool> {
        if !matches!(status, JobStatus::Completed | JobStatus::Failed) {
            bail!("finish status must be completed or failed");
        }
        let now = Utc::now();
        let conn = self.conn.lock().expect("worker store mutex poisoned");
        let payload = result.map(serde_json::to_string).transpose()?;
        Ok(conn.execute(
            "UPDATE jobs SET
                status=?3,
                payload_json=COALESCE(?4,payload_json),
                last_error=?5,
                lease_worker_id=NULL,
                lease_token=NULL,
                leased_at=NULL,
                lease_expires_at=NULL,
                updated_at=?6
             WHERE id=?1 AND lease_token=?2 AND status='leased'",
            params![
                job_id.to_string(),
                lease_token,
                status.as_str(),
                payload,
                error,
                now.to_rfc3339()
            ],
        )? == 1)
    }
}

fn parse_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableJob> {
    let id: String = row.get(0)?;
    let run_id: String = row.get(1)?;
    let task_id: Option<String> = row.get(2)?;
    let status: String = row.get(3)?;
    let payload: String = row.get(4)?;
    let capabilities: String = row.get(5)?;
    let attempts: u32 = row.get(6)?;
    let max_attempts: u32 = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    Ok(DurableJob {
        id: Uuid::parse_str(&id).map_err(to_sql_error)?,
        run_id: Uuid::parse_str(&run_id).map_err(to_sql_error)?,
        task_id: task_id
            .map(|value| Uuid::parse_str(&value).map_err(to_sql_error))
            .transpose()?,
        status: JobStatus::parse(&status).map_err(to_sql_error)?,
        payload: serde_json::from_str(&payload).map_err(to_sql_error)?,
        required_capabilities: serde_json::from_str(&capabilities).map_err(to_sql_error)?,
        attempts,
        max_attempts,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map_err(to_sql_error)?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map_err(to_sql_error)?
            .with_timezone(&Utc),
    })
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(error),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_job_can_be_claimed_checkpointed_and_completed() {
        let store = WorkerStore::in_memory().unwrap();
        let worker = store
            .register_worker(
                "worker-a",
                None,
                BTreeSet::from(["rust".into(), "docker".into()]),
                BTreeSet::new(),
            )
            .unwrap();
        let job = store
            .submit_job(
                Uuid::new_v4(),
                None,
                serde_json::json!({"objective": "test"}),
                BTreeSet::from(["rust".into()]),
                2,
            )
            .unwrap();
        let lease = store
            .claim(worker.id, &worker.capabilities, 60)
            .unwrap()
            .unwrap();
        assert_eq!(lease.job.id, job.id);
        store
            .checkpoint(
                job.id,
                &lease.lease_token,
                1,
                &serde_json::json!({"phase": "compile"}),
            )
            .unwrap();
        assert_eq!(
            store.latest_checkpoint(job.id).unwrap().unwrap().sequence,
            1
        );
        assert!(store
            .complete(
                job.id,
                &lease.lease_token,
                &serde_json::json!({"ok": true})
            )
            .unwrap());
    }
}


use async_trait::async_trait;
use std::process::Stdio;
use tokio::{
    process::Command,
    time::{timeout, Duration as TokioDuration},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerCommand {
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[async_trait]
pub trait WorkerExecutor: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn execute(
        &self,
        workspace: &std::path::Path,
        command: &WorkerCommand,
    ) -> Result<WorkerCommandResult>;
}

#[derive(Default)]
pub struct LocalWorkerExecutor;

#[async_trait]
impl WorkerExecutor for LocalWorkerExecutor {
    fn kind(&self) -> &'static str {
        "local"
    }

    async fn execute(
        &self,
        workspace: &std::path::Path,
        command: &WorkerCommand,
    ) -> Result<WorkerCommandResult> {
        execute_local(workspace, command).await
    }
}

#[derive(Debug, Clone)]
pub struct OciWorkerExecutor {
    pub program: String,
    pub image: String,
    pub cpus: f32,
    pub memory_mb: u64,
    pub pids: u32,
    pub network: String,
}

impl OciWorkerExecutor {
    pub fn docker(image: impl Into<String>) -> Self {
        Self {
            program: "docker".into(),
            image: image.into(),
            cpus: 2.0,
            memory_mb: 4096,
            pids: 256,
            network: "none".into(),
        }
    }

    pub fn podman(image: impl Into<String>) -> Self {
        Self {
            program: "podman".into(),
            image: image.into(),
            cpus: 2.0,
            memory_mb: 4096,
            pids: 256,
            network: "none".into(),
        }
    }
}

#[async_trait]
impl WorkerExecutor for OciWorkerExecutor {
    fn kind(&self) -> &'static str {
        "oci"
    }

    async fn execute(
        &self,
        workspace: &std::path::Path,
        command: &WorkerCommand,
    ) -> Result<WorkerCommandResult> {
        validate_command(command)?;
        let workspace = workspace
            .canonicalize()
            .context("worker workspace not found")?;
        let cwd = safe_relative(&command.cwd)?;
        if !self.cpus.is_finite() || self.cpus <= 0.0 || self.memory_mb < 128 || self.pids == 0 {
            bail!("invalid OCI worker resource limits");
        }
        if !matches!(self.network.as_str(), "none" | "bridge") {
            bail!("OCI worker network must be none or bridge");
        }

        let mut args = vec![
            "run".to_string(),
            "--rm".into(),
            "--init".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--user".into(),
            "10001:10001".into(),
            "--cpus".into(),
            self.cpus.to_string(),
            "--memory".into(),
            format!("{}m", self.memory_mb),
            "--pids-limit".into(),
            self.pids.to_string(),
            "--network".into(),
            self.network.clone(),
            "-v".into(),
            format!("{}:/workspace:rw", workspace.display()),
            "-w".into(),
            if cwd.as_os_str().is_empty() {
                "/workspace".into()
            } else {
                format!("/workspace/{}", cwd.display())
            },
        ];

        for (key, value) in &command.environment {
            validate_env_key(key)?;
            args.extend(["-e".into(), format!("{key}={value}")]);
        }
        args.push(self.image.clone());
        args.extend(command.argv.clone());

        run_command(
            &self.program,
            &args,
            None,
            command.timeout_seconds,
            &std::collections::BTreeMap::new(),
        )
        .await
    }
}

#[derive(Debug, Clone)]
pub struct KubernetesWorkerExecutor {
    pub namespace: String,
    pub image: String,
    pub service_account: Option<String>,
}

#[async_trait]
impl WorkerExecutor for KubernetesWorkerExecutor {
    fn kind(&self) -> &'static str {
        "kubernetes"
    }

    async fn execute(
        &self,
        _workspace: &std::path::Path,
        command: &WorkerCommand,
    ) -> Result<WorkerCommandResult> {
        validate_command(command)?;
        validate_resource_name(&self.namespace)?;
        if self.image.trim().is_empty() {
            bail!("Kubernetes worker image cannot be empty");
        }

        let name = format!("openforge-{}", Uuid::new_v4().simple());
        let mut overrides = serde_json::json!({
            "apiVersion": "v1",
            "spec": {
                "restartPolicy": "Never",
                "containers": [{
                    "name": "worker",
                    "image": self.image,
                    "command": command.argv,
                    "env": command.environment.iter().map(|(name,value)| {
                        serde_json::json!({"name": name, "value": value})
                    }).collect::<Vec<_>>(),
                    "securityContext": {
                        "allowPrivilegeEscalation": false,
                        "runAsNonRoot": true,
                        "runAsUser": 10001,
                        "capabilities": {"drop": ["ALL"]},
                        "seccompProfile": {"type": "RuntimeDefault"}
                    }
                }]
            }
        });
        if let Some(account) = &self.service_account {
            validate_resource_name(account)?;
            overrides["spec"]["serviceAccountName"] = serde_json::Value::String(account.clone());
        }

        let args = vec![
            "run".into(),
            name.clone(),
            "-n".into(),
            self.namespace.clone(),
            "--restart=Never".into(),
            "--attach".into(),
            "--rm".into(),
            "--quiet".into(),
            "--image".into(),
            self.image.clone(),
            "--overrides".into(),
            serde_json::to_string(&overrides)?,
            "--command".into(),
            "--".into(),
        ];

        run_command(
            "kubectl",
            &args,
            None,
            command.timeout_seconds,
            &std::collections::BTreeMap::new(),
        )
        .await
    }
}

#[derive(Debug, Clone)]
pub struct SshVmWorkerExecutor {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<std::path::PathBuf>,
    pub remote_workspace: String,
}

#[async_trait]
impl WorkerExecutor for SshVmWorkerExecutor {
    fn kind(&self) -> &'static str {
        "ssh-vm"
    }

    async fn execute(
        &self,
        _workspace: &std::path::Path,
        command: &WorkerCommand,
    ) -> Result<WorkerCommandResult> {
        validate_command(command)?;
        validate_host(&self.host)?;
        validate_resource_name(&self.user)?;
        if self.remote_workspace.trim().is_empty() {
            bail!("remote workspace cannot be empty");
        }

        let mut args = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-p".into(),
            self.port.to_string(),
        ];
        if let Some(identity) = &self.identity_file {
            let identity = identity
                .canonicalize()
                .context("SSH identity file not found")?;
            args.extend(["-i".into(), identity.display().to_string()]);
        }
        args.push(format!("{}@{}", self.user, self.host));

        let env_prefix = command
            .environment
            .iter()
            .map(|(key, value)| {
                validate_env_key(key)?;
                Ok(format!("{}={}", shell_escape(key), shell_escape(value)))
            })
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let remote_cwd = format!(
            "{}/{}",
            self.remote_workspace.trim_end_matches('/'),
            safe_relative(&command.cwd)?.display()
        );
        let remote_argv = command
            .argv
            .iter()
            .map(|value| shell_escape(value))
            .collect::<Vec<_>>()
            .join(" ");
        args.push(format!(
            "cd {} && {} {}",
            shell_escape(&remote_cwd),
            env_prefix,
            remote_argv
        ));

        run_command(
            "ssh",
            &args,
            None,
            command.timeout_seconds,
            &std::collections::BTreeMap::new(),
        )
        .await
    }
}

async fn execute_local(
    workspace: &std::path::Path,
    command: &WorkerCommand,
) -> Result<WorkerCommandResult> {
    validate_command(command)?;
    let workspace = workspace
        .canonicalize()
        .context("worker workspace not found")?;
    let cwd = workspace.join(safe_relative(&command.cwd)?);
    let canonical_cwd = cwd.canonicalize().context("worker cwd not found")?;
    if !canonical_cwd.starts_with(&workspace) {
        bail!("worker cwd escapes workspace");
    }
    run_command(
        &command.argv[0],
        &command.argv[1..],
        Some(&canonical_cwd),
        command.timeout_seconds,
        &command.environment,
    )
    .await
}

async fn run_command(
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    timeout_seconds: u64,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<WorkerCommandResult> {
    let mut process = Command::new(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    if let Some(path) = std::env::var_os("PATH") {
        process.env("PATH", path);
    }
    for (key, value) in environment {
        validate_env_key(key)?;
        process.env(key, value);
    }

    let child = process.spawn().with_context(|| format!("spawn worker command {program}"))?;
    match timeout(
        TokioDuration::from_secs(timeout_seconds.clamp(1, 86_400)),
        child.wait_with_output(),
    )
    .await
    {
        Ok(output) => {
            let output = output?;
            Ok(WorkerCommandResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            })
        }
        Err(_) => Ok(WorkerCommandResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: "worker command timed out".into(),
            timed_out: true,
        }),
    }
}

fn validate_command(command: &WorkerCommand) -> Result<()> {
    if command.argv.is_empty() || command.argv[0].trim().is_empty() {
        bail!("worker command argv cannot be empty");
    }
    safe_relative(&command.cwd)?;
    for key in command.environment.keys() {
        validate_env_key(key)?;
    }
    Ok(())
}

fn safe_relative(value: &str) -> Result<std::path::PathBuf> {
    let path = std::path::Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        bail!("worker path must remain relative to workspace");
    }
    Ok(path.to_path_buf())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid worker environment variable name");
    }
    Ok(())
}

fn validate_resource_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        bail!("invalid worker resource name");
    }
    Ok(())
}

fn validate_host(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || "-_.:".contains(character)
        })
    {
        bail!("invalid worker host");
    }
    Ok(())
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace(''', "'\\''"))
}
