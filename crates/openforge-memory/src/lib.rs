use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: Uuid,
    pub scope: String,
    pub key: String,
    pub value: Value,
    pub provenance: String,
    pub confidence: f32,
    pub expires_at: Option<DateTime<Utc>>,
    pub repository_fingerprint: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInput {
    pub scope: String,
    pub key: String,
    pub value: Value,
    pub provenance: String,
    pub confidence: f32,
    pub expires_at: Option<DateTime<Utc>>,
    pub repository_fingerprint: Option<String>,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub lexical_score: f32,
    pub semantic_score: Option<f32>,
    pub stale: bool,
    pub combined_score: f32,
}

#[derive(Clone)]
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
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
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS rich_memory(
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL,
                key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                provenance TEXT NOT NULL,
                confidence REAL NOT NULL,
                expires_at TEXT,
                repository_fingerprint TEXT,
                embedding_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL,
                UNIQUE(scope,key)
            );
            CREATE INDEX IF NOT EXISTS idx_rich_memory_scope ON rich_memory(scope);
            CREATE INDEX IF NOT EXISTS idx_rich_memory_expiry ON rich_memory(expires_at);
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn put(&self, input: MemoryInput) -> Result<MemoryRecord> {
        validate_input(&input)?;
        let now = Utc::now();
        let conn = self.conn.lock().expect("memory store mutex poisoned");
        let existing = conn
            .query_row(
                "SELECT id,created_at FROM rich_memory WHERE scope=?1 AND key=?2",
                params![input.scope, input.key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let (id, created_at) = match existing {
            Some((id, created_at)) => (
                Uuid::parse_str(&id)?,
                DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            ),
            None => (Uuid::now_v7(), now),
        };

        conn.execute(
            "INSERT INTO rich_memory(
                id,scope,key,value_json,provenance,confidence,expires_at,
                repository_fingerprint,embedding_json,created_at,updated_at,last_accessed_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)
             ON CONFLICT(scope,key) DO UPDATE SET
                value_json=excluded.value_json,
                provenance=excluded.provenance,
                confidence=excluded.confidence,
                expires_at=excluded.expires_at,
                repository_fingerprint=excluded.repository_fingerprint,
                embedding_json=excluded.embedding_json,
                updated_at=excluded.updated_at",
            params![
                id.to_string(),
                input.scope,
                input.key,
                serde_json::to_string(&input.value)?,
                input.provenance,
                input.confidence,
                input.expires_at.map(|value| value.to_rfc3339()),
                input.repository_fingerprint,
                input
                    .embedding
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                created_at.to_rfc3339(),
                now.to_rfc3339()
            ],
        )?;

        Ok(MemoryRecord {
            id,
            scope: input.scope,
            key: input.key,
            value: input.value,
            provenance: input.provenance,
            confidence: input.confidence,
            expires_at: input.expires_at,
            repository_fingerprint: input.repository_fingerprint,
            embedding: input.embedding,
            created_at,
            updated_at: now,
            last_accessed_at: now,
        })
    }

    pub fn search(
        &self,
        scope: Option<&str>,
        query: &str,
        query_embedding: Option<&[f32]>,
        current_repository_fingerprint: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryHit>> {
        let conn = self.conn.lock().expect("memory store mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT id,scope,key,value_json,provenance,confidence,expires_at,
                    repository_fingerprint,embedding_json,created_at,updated_at,last_accessed_at
             FROM rich_memory
             WHERE (?1 IS NULL OR scope=?1)
               AND (expires_at IS NULL OR expires_at > ?2)
             ORDER BY updated_at DESC
             LIMIT 5000",
        )?;
        let records = statement
            .query_map(params![scope, Utc::now().to_rfc3339()], parse_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let terms = query
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|term| term.len() > 1)
            .map(|term| term.to_lowercase())
            .collect::<Vec<_>>();

        let mut hits = records
            .into_iter()
            .filter_map(|record| {
                let haystack =
                    format!("{} {} {}", record.key, record.provenance, record.value).to_lowercase();
                let lexical_score = if terms.is_empty() {
                    0.0
                } else {
                    terms
                        .iter()
                        .map(|term| if haystack.contains(term) { 1.0 } else { 0.0 })
                        .sum::<f32>()
                        / terms.len() as f32
                };

                let semantic_score = match (query_embedding, record.embedding.as_deref()) {
                    (Some(left), Some(right)) => cosine(left, right),
                    _ => None,
                };
                let stale = match (
                    current_repository_fingerprint,
                    record.repository_fingerprint.as_deref(),
                ) {
                    (Some(current), Some(stored)) => current != stored,
                    _ => false,
                };
                let semantic = semantic_score.unwrap_or(0.0).max(0.0);
                let freshness = if stale { 0.25 } else { 1.0 };
                let combined_score =
                    (0.45 * lexical_score + 0.45 * semantic + 0.10 * record.confidence) * freshness;
                (combined_score > 0.0 || query.is_empty()).then_some(MemoryHit {
                    record,
                    lexical_score,
                    semantic_score,
                    stale,
                    combined_score,
                })
            })
            .collect::<Vec<_>>();

        hits.sort_by(|left, right| {
            right
                .combined_score
                .partial_cmp(&left.combined_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| right.record.updated_at.cmp(&left.record.updated_at))
        });
        hits.truncate(limit.min(1000));

        let now = Utc::now().to_rfc3339();
        for hit in &hits {
            conn.execute(
                "UPDATE rich_memory SET last_accessed_at=?2 WHERE id=?1",
                params![hit.record.id.to_string(), now],
            )?;
        }
        Ok(hits)
    }

    pub fn delete(&self, scope: &str, key: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("memory store mutex poisoned");
        Ok(conn.execute(
            "DELETE FROM rich_memory WHERE scope=?1 AND key=?2",
            params![scope, key],
        )? == 1)
    }

    pub fn purge_expired(&self) -> Result<usize> {
        let conn = self.conn.lock().expect("memory store mutex poisoned");
        Ok(conn.execute(
            "DELETE FROM rich_memory WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            params![Utc::now().to_rfc3339()],
        )?)
    }

    pub fn invalidate_repository_fingerprint(&self, current: &str) -> Result<usize> {
        if current.trim().is_empty() {
            bail!("repository fingerprint cannot be empty");
        }
        let conn = self.conn.lock().expect("memory store mutex poisoned");
        Ok(conn.execute(
            "UPDATE rich_memory
             SET confidence=confidence*0.5
             WHERE repository_fingerprint IS NOT NULL
               AND repository_fingerprint != ?1",
            params![current],
        )?)
    }
}

fn validate_input(input: &MemoryInput) -> Result<()> {
    if input.scope.trim().is_empty()
        || input.key.trim().is_empty()
        || input.provenance.trim().is_empty()
    {
        bail!("memory scope, key and provenance are required");
    }
    if !input.confidence.is_finite() || !(0.0..=1.0).contains(&input.confidence) {
        bail!("memory confidence must be between zero and one");
    }
    if let Some(embedding) = &input.embedding {
        if embedding.is_empty() || embedding.iter().any(|value| !value.is_finite()) {
            bail!("memory embedding must be finite and non-empty");
        }
    }
    Ok(())
}

fn parse_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let id: String = row.get(0)?;
    let value_json: String = row.get(3)?;
    let expires_at: Option<String> = row.get(6)?;
    let embedding_json: Option<String> = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;
    let last_accessed_at: String = row.get(11)?;

    Ok(MemoryRecord {
        id: Uuid::parse_str(&id).map_err(sql_error)?,
        scope: row.get(1)?,
        key: row.get(2)?,
        value: serde_json::from_str(&value_json).map_err(sql_error)?,
        provenance: row.get(4)?,
        confidence: row.get(5)?,
        expires_at: expires_at
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(sql_error)
            })
            .transpose()?,
        repository_fingerprint: row.get(7)?,
        embedding: embedding_json
            .map(|value| serde_json::from_str(&value).map_err(sql_error))
            .transpose()?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map_err(sql_error)?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map_err(sql_error)?
            .with_timezone(&Utc),
        last_accessed_at: DateTime::parse_from_rfc3339(&last_accessed_at)
            .map_err(sql_error)?
            .with_timezone(&Utc),
    })
}

fn sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return None;
    }
    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_repository_memory_is_downranked() {
        let store = MemoryStore::in_memory().unwrap();
        store
            .put(MemoryInput {
                scope: "project".into(),
                key: "architecture".into(),
                value: serde_json::json!({"decision": "daemon-first"}),
                provenance: "docs/adr".into(),
                confidence: 1.0,
                expires_at: None,
                repository_fingerprint: Some("old".into()),
                embedding: Some(vec![1.0, 0.0]),
            })
            .unwrap();
        let hits = store
            .search(
                Some("project"),
                "architecture",
                Some(&[1.0, 0.0]),
                Some("new"),
                10,
            )
            .unwrap();
        assert!(hits[0].stale);
    }
}
