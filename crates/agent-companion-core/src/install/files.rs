//! Backups and atomic configuration replacement shared by the editor and installers.
use std::io::Write;
#[cfg(feature = "server")]
use std::path::PathBuf;
use std::{fs, io, path::Path};

pub(super) fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Prepare every backup before changing any user file. On failure restore all
/// earlier writes, so a half-installed hook never loses its rollback record.
#[cfg(feature = "server")]
pub(super) fn commit_files(
    writes: Vec<(PathBuf, Option<Vec<u8>>)>,
) -> io::Result<(bool, Vec<PathBuf>)> {
    let mut changes = Vec::new();
    let mut backups = Vec::new();
    for (path, bytes) in writes {
        let original = read_optional(&path)?;
        if bytes == original {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if original.is_some() {
            let backup = super::backup_path_for(&path);
            fs::copy(&path, &backup)?;
            backups.push(backup);
        }
        changes.push((path, bytes, original));
    }
    for (index, (path, bytes, _)) in changes.iter().enumerate() {
        if let Err(error) = replace(path, bytes.as_deref()) {
            let mut rollback_errors = Vec::new();
            for (path, _, original) in changes[..=index].iter().rev() {
                if let Err(rollback) = replace(path, original.as_deref()) {
                    rollback_errors.push(rollback.to_string());
                }
            }
            return Err(io::Error::other(format!(
                "{error}; rollback errors: {rollback_errors:?}"
            )));
        }
    }
    Ok((!changes.is_empty(), backups))
}

#[cfg(feature = "server")]
fn replace(path: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    if let Some(bytes) = bytes {
        let mut temporary =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        temporary.write_all(bytes)?;
        if let Ok(metadata) = fs::metadata(path) {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    } else {
        match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

/// Compare against the exact document read by the editor, including unrelated
/// fields, again just before replacement. Never truncate the original in place.
pub(super) fn save_editor(path: &Path, original: Option<&[u8]>, bytes: &[u8]) -> io::Result<()> {
    let conflict = || {
        io::Error::other(
            "Codex configuration changed while saving. Close and reopen the editor to reload it.",
        )
    };
    if read_optional(path)?.as_deref() != original {
        return Err(conflict());
    }
    let permissions = if original.is_some() {
        // A rename can replace a read-only file on Unix: explicitly respect it.
        let file = fs::OpenOptions::new().write(true).open(path)?;
        let permissions = file.metadata()?.permissions();
        if permissions.readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Codex configuration is read-only",
            ));
        }
        Some(permissions)
    } else {
        None
    };
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    if let Some(permissions) = &permissions {
        temporary.as_file().set_permissions(permissions.clone())?;
    }
    temporary.as_file().sync_all()?;
    if read_optional(path)?.as_deref() != original {
        return Err(conflict());
    }
    if let Some(original) = original {
        let backup = super::backup_path_for(path);
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(backup)?;
        if let Some(permissions) = &permissions {
            file.set_permissions(permissions.clone())?;
        }
        file.write_all(original)?;
        file.sync_all()?;
    }
    if read_optional(path)?.as_deref() != original {
        return Err(conflict());
    }
    if original.is_some() {
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_document_cannot_replace_a_concurrent_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "new").unwrap();
        assert!(save_editor(&path, Some(b"old"), b"replacement").is_err());
        assert!(save_editor(&path, None, b"replacement").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
