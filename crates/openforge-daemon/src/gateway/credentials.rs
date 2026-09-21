use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) struct CredentialFile {
    path: PathBuf,
}

impl CredentialFile {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[cfg(unix)]
    fn directory(&self, create: bool) -> Result<bool> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let directory = self
            .path
            .parent()
            .context("credential file has no parent")?;
        if create {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        let metadata = match fs::symlink_metadata(directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        // geteuid reads process identity without accessing any credential data.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
            bail!(
                "Gateway credential directory must be owned by the current user and not a symlink"
            );
        }
        if create {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        } else if metadata.mode() & 0o077 != 0 {
            bail!("Gateway credential directory permissions must be 0700");
        }
        Ok(true)
    }

    #[cfg(not(unix))]
    fn directory(&self, _create: bool) -> Result<bool> {
        bail!("Private gateway credential persistence currently requires Unix file permissions")
    }

    pub(super) fn read(&self) -> Result<Option<String>> {
        if !self.directory(false)? {
            return Ok(None);
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("open private gateway credential file"),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > 16_384 {
            bail!("Invalid gateway credential file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                bail!(
                    "Gateway credential file must be owned by the current user with permissions 0600"
                );
            }
        }
        let mut key = String::new();
        file.take(16_385)
            .read_to_string(&mut key)
            .context("read private gateway credential file")?;
        super::validate_key(&key)?;
        Ok(Some(key))
    }

    pub(super) fn save(&self, key: &str) -> Result<()> {
        super::validate_key(key)?;
        self.directory(true)?;
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&temporary)
                .context("create private gateway credential file")?;
            file.write_all(key.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path).context("save private gateway credential file")?;
            sync_directory(self.path.parent().expect("validated parent"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(super) fn remove(&self) -> Result<()> {
        if !self.directory(false)? {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => sync_directory(self.path.parent().expect("validated parent")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("remove saved gateway credential"),
        }
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    #[test]
    fn private_atomic_round_trip_and_forget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/sevi.key");
        let store = CredentialFile::new(path.clone());
        assert!(store.read().unwrap().is_none());
        store.save("fake-key").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("fake-key"));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        store.save("replacement-key").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("replacement-key"));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        store.remove().unwrap();
        store.remove().unwrap();
        assert!(store.read().unwrap().is_none());
    }

    #[test]
    fn refuses_symlinks_and_world_readable_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/sevi.key");
        let store = CredentialFile::new(path.clone());
        store.save("fake-key").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.read().is_err());
        fs::remove_file(&path).unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "untouched").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(store.read().is_err());
        store.save("new-key").unwrap();
        assert_eq!(fs::read_to_string(outside).unwrap(), "untouched");
        assert_eq!(store.read().unwrap().as_deref(), Some("new-key"));
    }
}
