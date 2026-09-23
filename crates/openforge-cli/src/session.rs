use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub workspace: PathBuf,
    pub transcript: Vec<(String, String)>,
    pub last_run: Option<Uuid>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub goal_paused: bool,
    #[serde(default)]
    pub personality: Option<String>,
    #[serde(default)]
    pub mentions: Vec<PathBuf>,
}

impl Session {
    pub fn new(workspace: &Path) -> Self {
        Self {
            id: Uuid::new_v4(),
            workspace: workspace.into(),
            transcript: vec![],
            last_run: None,
            title: None,
            goal: None,
            goal_paused: false,
            personality: None,
            mentions: vec![],
        }
    }

    pub fn root() -> Result<PathBuf> {
        let root = if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path)
        } else {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not configured")?)
                .join(".local/state")
        };
        Ok(root.join("openforge/sessions"))
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        ensure_private_dir(root)?;
        let tmp = root.join(format!("{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        fs::rename(tmp, root.join(format!("{}.json", self.id)))?;
        Ok(())
    }

    pub fn load(root: &Path, id: Uuid, workspace: &Path) -> Result<Self> {
        let session: Self = serde_json::from_slice(&fs::read(root.join(format!("{id}.json")))?)?;
        if session.workspace != workspace {
            bail!(
                "session belongs to a different workspace: {}",
                session.workspace.display()
            );
        }
        Ok(session)
    }

    pub fn delete(&self, root: &Path) -> Result<bool> {
        let path = root.join(format!("{}.json", self.id));
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(path)?;
        Ok(true)
    }

    pub fn archive(&self, root: &Path) -> Result<PathBuf> {
        self.save(root)?;
        let archived = root.join("archived");
        ensure_private_dir(&archived)?;
        let source = root.join(format!("{}.json", self.id));
        let destination = archived.join(format!("{}.json", self.id));
        fs::rename(source, &destination)?;
        Ok(destination)
    }

    pub fn latest_output(&self) -> Option<&str> {
        self.transcript
            .iter()
            .rev()
            .find(|(role, _)| role == "openforge")
            .map(|(_, text)| text.as_str())
    }

    pub fn list(root: &Path, workspace: &Path) -> Result<()> {
        if !root.exists() {
            println!("No saved sessions for this workspace");
            return Ok(());
        }
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                if let Ok(session) = Self::load(root, id, workspace) {
                    println!(
                        "{}  {}  {} messages  last run: {}",
                        id,
                        session.title.as_deref().unwrap_or("untitled"),
                        session.transcript.len(),
                        session
                            .last_run
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".into())
                    );
                }
            }
        }
        Ok(())
    }
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_roundtrip_and_reject_cross_workspace_resume() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = Session::new(Path::new("/project"));
        session.title = Some("terminal parity".into());
        session.goal = Some("keep tests green".into());
        session.personality = Some("pragmatic".into());
        session.mentions.push(PathBuf::from("README.md"));
        session.transcript.push(("user".into(), "fix tests".into()));
        session.save(temp.path()).unwrap();
        let restored = Session::load(temp.path(), session.id, Path::new("/project")).unwrap();
        assert_eq!(restored.transcript, session.transcript);
        assert_eq!(restored.title.as_deref(), Some("terminal parity"));
        assert_eq!(restored.goal.as_deref(), Some("keep tests green"));
        assert_eq!(restored.personality.as_deref(), Some("pragmatic"));
        assert_eq!(restored.mentions, vec![PathBuf::from("README.md")]);
        assert!(Session::load(temp.path(), session.id, Path::new("/other")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join(format!("{}.json", session.id)))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn archive_and_delete_move_session_state_without_data_loss() {
        let temp = tempfile::tempdir().unwrap();
        let session = Session::new(Path::new("/project"));
        session.save(temp.path()).unwrap();
        let archived = session.archive(temp.path()).unwrap();
        assert!(archived.exists());
        assert!(!temp.path().join(format!("{}.json", session.id)).exists());

        let replacement = Session::new(Path::new("/project"));
        replacement.save(temp.path()).unwrap();
        assert!(replacement.delete(temp.path()).unwrap());
        assert!(
            !temp
                .path()
                .join(format!("{}.json", replacement.id))
                .exists()
        );
    }
}
