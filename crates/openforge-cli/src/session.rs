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
}
impl Session {
    pub fn new(workspace: &Path) -> Self {
        Self {
            id: Uuid::new_v4(),
            workspace: workspace.into(),
            transcript: vec![],
            last_run: None,
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
        fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
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
                        "{}  {} messages  last run: {}",
                        id,
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_roundtrip_and_reject_cross_workspace_resume() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = Session::new(Path::new("/project"));
        session.transcript.push(("user".into(), "fix tests".into()));
        session.save(temp.path()).unwrap();
        assert_eq!(
            Session::load(temp.path(), session.id, Path::new("/project"))
                .unwrap()
                .transcript,
            session.transcript
        );
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
}
