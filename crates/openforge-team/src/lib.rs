use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Viewer,
    Reviewer,
    Developer,
    Maintainer,
    Admin,
    Owner,
}

impl TeamRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Reviewer => "reviewer",
            Self::Developer => "developer",
            Self::Maintainer => "maintainer",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "viewer" => Ok(Self::Viewer),
            "reviewer" => Ok(Self::Reviewer),
            "developer" => Ok(Self::Developer),
            "maintainer" => Ok(Self::Maintainer),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            other => bail!("unknown team role {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Read,
    Comment,
    Execute,
    WriteRepository,
    Approve,
    ManagePolicy,
    ManageSecrets,
    ManageMembers,
    ManageWorkspace,
}

impl TeamRole {
    pub fn permissions(self) -> BTreeSet<Permission> {
        use Permission::*;
        match self {
            Self::Viewer => BTreeSet::from([Read]),
            Self::Reviewer => BTreeSet::from([Read, Comment, Approve]),
            Self::Developer => BTreeSet::from([Read, Comment, Execute, WriteRepository]),
            Self::Maintainer => BTreeSet::from([
                Read,
                Comment,
                Execute,
                WriteRepository,
                Approve,
                ManagePolicy,
            ]),
            Self::Admin => BTreeSet::from([
                Read,
                Comment,
                Execute,
                WriteRepository,
                Approve,
                ManagePolicy,
                ManageSecrets,
                ManageMembers,
            ]),
            Self::Owner => BTreeSet::from([
                Read,
                Comment,
                Execute,
                WriteRepository,
                Approve,
                ManagePolicy,
                ManageSecrets,
                ManageMembers,
                ManageWorkspace,
            ]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid,
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub provider: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRepository {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub canonical_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub identity: Identity,
    pub workspace_id: Uuid,
    pub role: TeamRole,
}

impl Principal {
    pub fn can(&self, permission: Permission) -> bool {
        self.role.permissions().contains(&permission)
    }

    pub fn require(&self, permission: Permission) -> Result<()> {
        if self.can(permission) {
            Ok(())
        } else {
            bail!(
                "principal {} with role {:?} lacks permission {:?}",
                self.identity.subject,
                self.role,
                permission
            )
        }
    }
}

#[derive(Clone)]
pub struct TeamStore {
    conn: Arc<Mutex<Connection>>,
}

impl TeamStore {
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
            CREATE TABLE IF NOT EXISTS identities(
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                email TEXT,
                display_name TEXT,
                provider TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(provider, subject)
            );
            CREATE TABLE IF NOT EXISTS team_workspaces(
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workspace_repositories(
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                name TEXT NOT NULL,
                canonical_path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(workspace_id,canonical_path),
                FOREIGN KEY(workspace_id) REFERENCES team_workspaces(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS memberships(
                workspace_id TEXT NOT NULL,
                identity_id TEXT NOT NULL,
                role TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(workspace_id, identity_id),
                FOREIGN KEY(workspace_id) REFERENCES team_workspaces(id) ON DELETE CASCADE,
                FOREIGN KEY(identity_id) REFERENCES identities(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS access_tokens(
                id TEXT PRIMARY KEY,
                identity_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                label TEXT NOT NULL,
                expires_at TEXT,
                revoked_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(identity_id) REFERENCES identities(id) ON DELETE CASCADE,
                FOREIGN KEY(workspace_id) REFERENCES team_workspaces(id) ON DELETE CASCADE
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn upsert_identity(
        &self,
        provider: &str,
        subject: &str,
        email: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<Identity> {
        if provider.trim().is_empty() || subject.trim().is_empty() {
            bail!("identity provider and subject are required");
        }
        let now = Utc::now();
        let conn = self.conn.lock().expect("team store mutex poisoned");

        if let Some(id) = conn
            .query_row(
                "SELECT id FROM identities WHERE provider=?1 AND subject=?2",
                params![provider, subject],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            conn.execute(
                "UPDATE identities SET email=?2,display_name=?3 WHERE id=?1",
                params![id, email, display_name],
            )?;
            return self.identity_by_id(Uuid::parse_str(&id)?, &conn);
        }

        let identity = Identity {
            id: Uuid::now_v7(),
            subject: subject.to_string(),
            email: email.map(str::to_string),
            display_name: display_name.map(str::to_string),
            provider: provider.to_string(),
            created_at: now,
        };
        conn.execute(
            "INSERT INTO identities(id,subject,email,display_name,provider,created_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                identity.id.to_string(),
                identity.subject,
                identity.email,
                identity.display_name,
                identity.provider,
                identity.created_at.to_rfc3339()
            ],
        )?;
        Ok(identity)
    }

    pub fn create_workspace(&self, slug: &str, name: &str) -> Result<Workspace> {
        if slug.trim().is_empty()
            || !slug
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            bail!("workspace slug must contain only letters, numbers and hyphens");
        }
        if name.trim().is_empty() {
            bail!("workspace name cannot be empty");
        }
        let workspace = Workspace {
            id: Uuid::now_v7(),
            slug: slug.to_ascii_lowercase(),
            name: name.trim().to_string(),
            created_at: Utc::now(),
        };
        let conn = self.conn.lock().expect("team store mutex poisoned");
        conn.execute(
            "INSERT INTO team_workspaces(id,slug,name,created_at) VALUES(?1,?2,?3,?4)",
            params![
                workspace.id.to_string(),
                workspace.slug,
                workspace.name,
                workspace.created_at.to_rfc3339()
            ],
        )?;
        Ok(workspace)
    }

    pub fn register_repository(
        &self,
        workspace_id: Uuid,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<WorkspaceRepository> {
        if name.trim().is_empty() {
            bail!("repository name cannot be empty");
        }
        let canonical = path
            .as_ref()
            .canonicalize()
            .with_context(|| format!("repository {} not found", path.as_ref().display()))?;
        if !canonical.join(".git").exists() {
            bail!("workspace repository must be a Git working tree");
        }
        let repository = WorkspaceRepository {
            id: Uuid::now_v7(),
            workspace_id,
            name: name.trim().to_string(),
            canonical_path: canonical.display().to_string(),
            created_at: Utc::now(),
        };
        let conn = self.conn.lock().expect("team store mutex poisoned");
        conn.execute(
            "INSERT INTO workspace_repositories(
                id,workspace_id,name,canonical_path,created_at
             ) VALUES(?1,?2,?3,?4,?5)",
            params![
                repository.id.to_string(),
                repository.workspace_id.to_string(),
                repository.name,
                repository.canonical_path,
                repository.created_at.to_rfc3339()
            ],
        )?;
        Ok(repository)
    }

    pub fn repositories(&self, workspace_id: Uuid) -> Result<Vec<WorkspaceRepository>> {
        let conn = self.conn.lock().expect("team store mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT id,name,canonical_path,created_at
             FROM workspace_repositories
             WHERE workspace_id=?1
             ORDER BY name,id",
        )?;
        let rows = statement.query_map(params![workspace_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut values = Vec::new();
        for row in rows {
            let (id, name, canonical_path, created_at) = row?;
            values.push(WorkspaceRepository {
                id: Uuid::parse_str(&id)?,
                workspace_id,
                name,
                canonical_path,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            });
        }
        Ok(values)
    }

    pub fn require_repository_access(
        &self,
        workspace_id: Uuid,
        path: impl AsRef<Path>,
    ) -> Result<std::path::PathBuf> {
        let canonical = path
            .as_ref()
            .canonicalize()
            .with_context(|| format!("repository path {} unavailable", path.as_ref().display()))?;
        let allowed = self
            .repositories(workspace_id)?
            .into_iter()
            .any(|repository| {
                let root = std::path::PathBuf::from(repository.canonical_path);
                canonical == root || canonical.starts_with(&root)
            });
        if !allowed {
            bail!(
                "path {} is not assigned to workspace {}",
                canonical.display(),
                workspace_id
            );
        }
        Ok(canonical)
    }

    pub fn set_membership(
        &self,
        workspace_id: Uuid,
        identity_id: Uuid,
        role: TeamRole,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("team store mutex poisoned");
        conn.execute(
            "INSERT INTO memberships(workspace_id,identity_id,role,created_at)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(workspace_id,identity_id) DO UPDATE SET role=excluded.role",
            params![
                workspace_id.to_string(),
                identity_id.to_string(),
                role.as_str(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn issue_token(
        &self,
        workspace_id: Uuid,
        identity_id: Uuid,
        label: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<String> {
        if label.trim().is_empty() {
            bail!("token label cannot be empty");
        }
        let raw = format!(
            "of_team_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let token_hash = hash_token(&raw);
        let conn = self.conn.lock().expect("team store mutex poisoned");
        conn.execute(
            "INSERT INTO access_tokens(
                id,identity_id,workspace_id,token_hash,label,expires_at,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                Uuid::now_v7().to_string(),
                identity_id.to_string(),
                workspace_id.to_string(),
                token_hash,
                label,
                expires_at.map(|value| value.to_rfc3339()),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(raw)
    }

    pub fn authenticate(&self, raw_token: &str) -> Result<Option<Principal>> {
        if raw_token.trim().is_empty() {
            return Ok(None);
        }
        let hash = hash_token(raw_token);
        let conn = self.conn.lock().expect("team store mutex poisoned");
        let row = conn
            .query_row(
                "SELECT t.identity_id,t.workspace_id,m.role,t.expires_at,t.revoked_at
                 FROM access_tokens t
                 JOIN memberships m
                   ON m.identity_id=t.identity_id AND m.workspace_id=t.workspace_id
                 WHERE t.token_hash=?1",
                params![hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;

        let Some((identity_id, workspace_id, role, expires_at, revoked_at)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(expires_at) = expires_at {
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)?.with_timezone(&Utc);
            if Utc::now() >= expires_at {
                return Ok(None);
            }
        }

        Ok(Some(Principal {
            identity: self.identity_by_id(Uuid::parse_str(&identity_id)?, &conn)?,
            workspace_id: Uuid::parse_str(&workspace_id)?,
            role: TeamRole::parse(&role)?,
        }))
    }

    pub fn revoke_token(&self, raw_token: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("team store mutex poisoned");
        Ok(conn.execute(
            "UPDATE access_tokens SET revoked_at=?2
             WHERE token_hash=?1 AND revoked_at IS NULL",
            params![hash_token(raw_token), Utc::now().to_rfc3339()],
        )? == 1)
    }

    fn identity_by_id(&self, id: Uuid, conn: &Connection) -> Result<Identity> {
        conn.query_row(
            "SELECT subject,email,display_name,provider,created_at
             FROM identities WHERE id=?1",
            params![id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(Into::into)
        .and_then(|(subject, email, display_name, provider, created_at)| {
            Ok(Identity {
                id,
                subject,
                email,
                display_name,
                provider,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            })
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcIntrospectionConfig {
    pub provider_name: String,
    pub introspection_url: String,
    pub client_id_env: String,
    pub client_secret_env: String,
    pub expected_issuer: Option<String>,
    pub expected_audience: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: BTreeSet<String>,
    pub issuer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntrospectionResponse {
    active: bool,
    sub: Option<String>,
    email: Option<String>,
    name: Option<String>,
    iss: Option<String>,
    aud: Option<serde_json::Value>,
    groups: Option<serde_json::Value>,
    exp: Option<i64>,
}

pub async fn introspect_oidc_token(
    config: &OidcIntrospectionConfig,
    token: &str,
) -> Result<OidcIdentity> {
    let client_id = std::env::var(&config.client_id_env)
        .with_context(|| format!("missing {}", config.client_id_env))?;
    let client_secret = std::env::var(&config.client_secret_env)
        .with_context(|| format!("missing {}", config.client_secret_env))?;

    let response = reqwest::Client::new()
        .post(&config.introspection_url)
        .basic_auth(client_id, Some(client_secret))
        .form(&[("token", token)])
        .send()
        .await
        .context("OIDC token introspection request failed")?
        .error_for_status()
        .context("OIDC token introspection returned an error status")?
        .json::<IntrospectionResponse>()
        .await
        .context("parse OIDC introspection response")?;

    if !response.active {
        bail!("OIDC token is inactive");
    }
    if let Some(exp) = response.exp {
        if Utc::now().timestamp() >= exp {
            bail!("OIDC token is expired");
        }
    }
    if let Some(expected) = &config.expected_issuer {
        if response.iss.as_deref() != Some(expected.as_str()) {
            bail!("OIDC issuer mismatch");
        }
    }
    if let Some(expected) = &config.expected_audience {
        let matches = match response.aud.as_ref() {
            Some(serde_json::Value::String(value)) => value == expected,
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .any(|value| value.as_str() == Some(expected.as_str())),
            _ => false,
        };
        if !matches {
            bail!("OIDC audience mismatch");
        }
    }

    let subject = response.sub.context("OIDC introspection missing subject")?;
    Ok(OidcIdentity {
        subject,
        email: response.email,
        display_name: response.name,
        groups: string_set(response.groups),
        issuer: response.iss,
    })
}

fn string_set(value: Option<serde_json::Value>) -> BTreeSet<String> {
    match value {
        Some(serde_json::Value::String(value)) => {
            value.split_whitespace().map(str::to_string).collect()
        }
        Some(serde_json::Value::Array(values)) => values
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn hash_token(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_auth_obeys_membership_role() {
        let store = TeamStore::in_memory().unwrap();
        let identity = store
            .upsert_identity("local", "gan", Some("gan@example.test"), None)
            .unwrap();
        let workspace = store.create_workspace("openforge", "OpenForge").unwrap();
        store
            .set_membership(workspace.id, identity.id, TeamRole::Maintainer)
            .unwrap();
        let token = store
            .issue_token(workspace.id, identity.id, "test", None)
            .unwrap();
        let principal = store.authenticate(&token).unwrap().unwrap();
        assert!(principal.can(Permission::Execute));
        assert!(!principal.can(Permission::ManageMembers));
    }
}
