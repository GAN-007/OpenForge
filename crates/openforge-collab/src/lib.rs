use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Active,
    Paused,
    TakenOver,
    Completed,
    Cancelled,
}

impl ThreadStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::TakenOver => "taken_over",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentThread {
    pub id: Uuid,
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub title: String,
    pub status: ThreadStatus,
    pub owner_subject: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub sequence: i64,
    pub author_kind: String,
    pub author_id: String,
    pub message_type: String,
    pub content: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedInstruction {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub content: String,
    pub submitted_by: String,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    ApprovedOnce,
    ApprovedAlways,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub capability: String,
    pub subject: String,
    pub reason: String,
    pub status: ApprovalStatus,
    pub decided_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub workspace_id: Uuid,
    pub subject: String,
    pub surface: String,
    pub resource: Option<String>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub run_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub author: String,
    pub body: String,
    pub resolved: bool,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct CollaborationStore {
    conn: Arc<Mutex<Connection>>,
}

impl CollaborationStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS agent_threads(
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                task_id TEXT,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                owner_subject TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS thread_messages(
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                author_kind TEXT NOT NULL,
                author_id TEXT NOT NULL,
                message_type TEXT NOT NULL,
                content_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(thread_id,sequence),
                FOREIGN KEY(thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS queued_instructions(
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                content TEXT NOT NULL,
                submitted_by TEXT NOT NULL,
                consumed_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS approval_requests(
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                capability TEXT NOT NULL,
                subject TEXT NOT NULL,
                reason TEXT NOT NULL,
                status TEXT NOT NULL,
                decided_by TEXT,
                created_at TEXT NOT NULL,
                decided_at TEXT,
                FOREIGN KEY(thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS presence(
                workspace_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                surface TEXT NOT NULL,
                resource TEXT,
                last_seen_at TEXT NOT NULL,
                PRIMARY KEY(workspace_id,subject,surface)
            );
            CREATE TABLE IF NOT EXISTS review_comments(
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                run_id TEXT,
                task_id TEXT,
                file_path TEXT,
                line INTEGER,
                author TEXT NOT NULL,
                body TEXT NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                resolved_at TEXT
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn create_thread(
        &self,
        run_id: Uuid,
        task_id: Option<Uuid>,
        title: &str,
        owner_subject: Option<&str>,
    ) -> Result<AgentThread> {
        if title.trim().is_empty() {
            bail!("thread title cannot be empty");
        }
        let now = Utc::now();
        let thread = AgentThread {
            id: Uuid::now_v7(),
            run_id,
            task_id,
            title: title.trim().to_string(),
            status: ThreadStatus::Active,
            owner_subject: owner_subject.map(str::to_string),
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        conn.execute(
            "INSERT INTO agent_threads(
                id,run_id,task_id,title,status,owner_subject,created_at,updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
            params![
                thread.id.to_string(),
                thread.run_id.to_string(),
                thread.task_id.map(|value| value.to_string()),
                thread.title,
                thread.status.as_str(),
                thread.owner_subject,
                now.to_rfc3339()
            ],
        )?;
        Ok(thread)
    }

    pub fn set_thread_status(&self, id: Uuid, status: ThreadStatus) -> Result<bool> {
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        Ok(conn.execute(
            "UPDATE agent_threads SET status=?2,updated_at=?3 WHERE id=?1",
            params![id.to_string(), status.as_str(), Utc::now().to_rfc3339()],
        )? == 1)
    }

    pub fn append_message(
        &self,
        thread_id: Uuid,
        author_kind: &str,
        author_id: &str,
        message_type: &str,
        content: &Value,
    ) -> Result<ThreadMessage> {
        if author_kind.trim().is_empty()
            || author_id.trim().is_empty()
            || message_type.trim().is_empty()
        {
            bail!("message author and type are required");
        }
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        let sequence = conn.query_row(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM thread_messages WHERE thread_id=?1",
            params![thread_id.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        let message = ThreadMessage {
            id: Uuid::now_v7(),
            thread_id,
            sequence,
            author_kind: author_kind.to_string(),
            author_id: author_id.to_string(),
            message_type: message_type.to_string(),
            content: content.clone(),
            created_at: Utc::now(),
        };
        conn.execute(
            "INSERT INTO thread_messages(
                id,thread_id,sequence,author_kind,author_id,message_type,content_json,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                message.id.to_string(),
                message.thread_id.to_string(),
                message.sequence,
                message.author_kind,
                message.author_id,
                message.message_type,
                serde_json::to_string(&message.content)?,
                message.created_at.to_rfc3339()
            ],
        )?;
        Ok(message)
    }

    pub fn messages(&self, thread_id: Uuid, after_sequence: i64, limit: usize) -> Result<Vec<ThreadMessage>> {
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT id,sequence,author_kind,author_id,message_type,content_json,created_at
             FROM thread_messages
             WHERE thread_id=?1 AND sequence>?2
             ORDER BY sequence
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                thread_id.to_string(),
                after_sequence,
                limit.clamp(1, 5000) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )?;
        let mut values = Vec::new();
        for row in rows {
            let (id, sequence, author_kind, author_id, message_type, content, created_at) = row?;
            values.push(ThreadMessage {
                id: Uuid::parse_str(&id)?,
                thread_id,
                sequence,
                author_kind,
                author_id,
                message_type,
                content: serde_json::from_str(&content)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            });
        }
        Ok(values)
    }

    pub fn queue_instruction(
        &self,
        thread_id: Uuid,
        content: &str,
        submitted_by: &str,
    ) -> Result<QueuedInstruction> {
        if content.trim().is_empty() || submitted_by.trim().is_empty() {
            bail!("instruction content and submitter are required");
        }
        let instruction = QueuedInstruction {
            id: Uuid::now_v7(),
            thread_id,
            content: content.trim().to_string(),
            submitted_by: submitted_by.to_string(),
            consumed_at: None,
            created_at: Utc::now(),
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        conn.execute(
            "INSERT INTO queued_instructions(
                id,thread_id,content,submitted_by,created_at
             ) VALUES(?1,?2,?3,?4,?5)",
            params![
                instruction.id.to_string(),
                instruction.thread_id.to_string(),
                instruction.content,
                instruction.submitted_by,
                instruction.created_at.to_rfc3339()
            ],
        )?;
        Ok(instruction)
    }

    pub fn consume_instructions(&self, thread_id: Uuid, maximum: usize) -> Result<Vec<QueuedInstruction>> {
        let mut conn = self.conn.lock().expect("collaboration mutex poisoned");
        let tx = conn.transaction()?;
        let mut statement = tx.prepare(
            "SELECT id,content,submitted_by,created_at
             FROM queued_instructions
             WHERE thread_id=?1 AND consumed_at IS NULL
             ORDER BY created_at
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![thread_id.to_string(), maximum.clamp(1, 1000) as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let now = Utc::now();
        let mut values = Vec::new();
        for (id, content, submitted_by, created_at) in rows {
            tx.execute(
                "UPDATE queued_instructions SET consumed_at=?2 WHERE id=?1 AND consumed_at IS NULL",
                params![id, now.to_rfc3339()],
            )?;
            values.push(QueuedInstruction {
                id: Uuid::parse_str(&id)?,
                thread_id,
                content,
                submitted_by,
                consumed_at: Some(now),
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            });
        }
        tx.commit()?;
        Ok(values)
    }

    pub fn request_approval(
        &self,
        thread_id: Uuid,
        capability: &str,
        subject: &str,
        reason: &str,
    ) -> Result<ApprovalRequest> {
        if capability.trim().is_empty() || reason.trim().is_empty() {
            bail!("approval capability and reason are required");
        }
        let request = ApprovalRequest {
            id: Uuid::now_v7(),
            thread_id,
            capability: capability.to_string(),
            subject: subject.to_string(),
            reason: reason.to_string(),
            status: ApprovalStatus::Pending,
            decided_by: None,
            created_at: Utc::now(),
            decided_at: None,
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        conn.execute(
            "INSERT INTO approval_requests(
                id,thread_id,capability,subject,reason,status,created_at
             ) VALUES(?1,?2,?3,?4,?5,'pending',?6)",
            params![
                request.id.to_string(),
                request.thread_id.to_string(),
                request.capability,
                request.subject,
                request.reason,
                request.created_at.to_rfc3339()
            ],
        )?;
        Ok(request)
    }

    pub fn decide_approval(
        &self,
        id: Uuid,
        status: ApprovalStatus,
        decided_by: &str,
    ) -> Result<bool> {
        if status == ApprovalStatus::Pending {
            bail!("approval decision cannot remain pending");
        }
        if decided_by.trim().is_empty() {
            bail!("approval decision actor is required");
        }
        let status_text = match status {
            ApprovalStatus::ApprovedOnce => "approved_once",
            ApprovalStatus::ApprovedAlways => "approved_always",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Pending => unreachable!(),
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        Ok(conn.execute(
            "UPDATE approval_requests
             SET status=?2,decided_by=?3,decided_at=?4
             WHERE id=?1 AND status='pending'",
            params![
                id.to_string(),
                status_text,
                decided_by,
                Utc::now().to_rfc3339()
            ],
        )? == 1)
    }

    pub fn heartbeat_presence(
        &self,
        workspace_id: Uuid,
        subject: &str,
        surface: &str,
        resource: Option<&str>,
    ) -> Result<Presence> {
        if subject.trim().is_empty() || surface.trim().is_empty() {
            bail!("presence subject and surface are required");
        }
        let presence = Presence {
            workspace_id,
            subject: subject.to_string(),
            surface: surface.to_string(),
            resource: resource.map(str::to_string),
            last_seen_at: Utc::now(),
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        conn.execute(
            "INSERT INTO presence(workspace_id,subject,surface,resource,last_seen_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(workspace_id,subject,surface) DO UPDATE SET
                resource=excluded.resource,
                last_seen_at=excluded.last_seen_at",
            params![
                workspace_id.to_string(),
                presence.subject,
                presence.surface,
                presence.resource,
                presence.last_seen_at.to_rfc3339()
            ],
        )?;
        Ok(presence)
    }

    pub fn active_presence(&self, workspace_id: Uuid, within_seconds: i64) -> Result<Vec<Presence>> {
        let cutoff = Utc::now()
            .checked_sub_signed(Duration::seconds(within_seconds.clamp(5, 3600)))
            .expect("valid presence cutoff");
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT subject,surface,resource,last_seen_at
             FROM presence
             WHERE workspace_id=?1 AND last_seen_at>=?2
             ORDER BY subject,surface",
        )?;
        let rows = statement.query_map(
            params![workspace_id.to_string(), cutoff.to_rfc3339()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut values = Vec::new();
        for row in rows {
            let (subject, surface, resource, last_seen_at) = row?;
            values.push(Presence {
                workspace_id,
                subject,
                surface,
                resource,
                last_seen_at: DateTime::parse_from_rfc3339(&last_seen_at)?.with_timezone(&Utc),
            });
        }
        Ok(values)
    }

    pub fn add_comment(
        &self,
        workspace_id: Uuid,
        run_id: Option<Uuid>,
        task_id: Option<Uuid>,
        file_path: Option<&str>,
        line: Option<u32>,
        author: &str,
        body: &str,
    ) -> Result<ReviewComment> {
        if author.trim().is_empty() || body.trim().is_empty() {
            bail!("comment author and body are required");
        }
        let comment = ReviewComment {
            id: Uuid::now_v7(),
            workspace_id,
            run_id,
            task_id,
            file_path: file_path.map(str::to_string),
            line,
            author: author.to_string(),
            body: body.to_string(),
            resolved: false,
            created_at: Utc::now(),
            resolved_at: None,
        };
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        conn.execute(
            "INSERT INTO review_comments(
                id,workspace_id,run_id,task_id,file_path,line,author,body,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                comment.id.to_string(),
                comment.workspace_id.to_string(),
                comment.run_id.map(|value| value.to_string()),
                comment.task_id.map(|value| value.to_string()),
                comment.file_path,
                comment.line,
                comment.author,
                comment.body,
                comment.created_at.to_rfc3339()
            ],
        )?;
        Ok(comment)
    }

    pub fn resolve_comment(&self, id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().expect("collaboration mutex poisoned");
        Ok(conn.execute(
            "UPDATE review_comments SET resolved=1,resolved_at=?2
             WHERE id=?1 AND resolved=0",
            params![id.to_string(), Utc::now().to_rfc3339()],
        )? == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_supports_queued_instruction_and_approval() {
        let store = CollaborationStore::in_memory().unwrap();
        let thread = store
            .create_thread(Uuid::new_v4(), None, "Fix auth", Some("gan"))
            .unwrap();
        store
            .queue_instruction(thread.id, "also add regression coverage", "gan")
            .unwrap();
        assert_eq!(store.consume_instructions(thread.id, 10).unwrap().len(), 1);
        let approval = store
            .request_approval(thread.id, "deployment", "prod", "ship release")
            .unwrap();
        assert!(store
            .decide_approval(approval.id, ApprovalStatus::ApprovedOnce, "gan")
            .unwrap());
    }
}
