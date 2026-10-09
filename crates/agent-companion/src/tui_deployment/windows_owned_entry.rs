//! Replace only verified Companion commands, including images still in use.
use super::{digest, no_redirects};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::{offset_of, size_of},
    os::windows::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE},
    Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, FileRenameInfo,
        SetFileInformationByHandle,
    },
};

pub(super) struct Generation {
    pub hash: String,
    pub length: u64,
}

struct PinnedDirectory {
    path: PathBuf,
    _handles: Vec<File>,
}

impl PinnedDirectory {
    fn allow_child_renames(&mut self) -> io::Result<()> {
        // The exclusively held stage now keeps the leaf nonempty, so it cannot
        // become a junction. Retain read access and deny delete sharing on all
        // ancestors, while allowing the directory writes needed for renames.
        let handles = self
            .path
            .ancestors()
            .map(|path| {
                OpenOptions::new()
                    .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                    .open(path)
            })
            .collect::<io::Result<Vec<_>>>()?;
        self._handles = handles;
        Ok(())
    }
}

// Pin every ancestor without write/delete sharing. LIST_DIRECTORY is needed
// too: an attributes-only handle still permits turning an empty directory
// into a junction. Child files can still be created under these pinned parents.
fn pin_parent(path: &Path) -> io::Result<PinnedDirectory> {
    no_redirects(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid command directory"))?;
    let mut directories = Vec::new();
    for directory in parent.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let open = || {
            OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(directory)
        };
        let handle = match open() {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(directory) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                open()?
            }
            Err(error) => return Err(error),
        };
        let metadata = handle.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::other("Command directory contains a redirect"));
        }
        directories.push(handle);
    }
    Ok(PinnedDirectory {
        path: parent.to_path_buf(),
        _handles: directories,
    })
}

struct Stage {
    file: File,
    published: bool,
}

impl Stage {
    fn publish(&mut self, directory: &PinnedDirectory, path: &Path) -> io::Result<()> {
        rename_no_replace(&self.file, directory, path)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if !self.published {
            // Delete only this held, unpublished file; a pathname cleanup could
            // remove a different file if a concurrent writer replaced the name.
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                let _ = SetFileInformationByHandle(
                    HANDLE(self.file.as_raw_handle()),
                    FileDispositionInfo,
                    (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                );
            }
        }
    }
}

fn stage(path: &Path, bytes: &[u8], directory: &mut PinnedDirectory) -> io::Result<Stage> {
    let temporary = tempfile::Builder::new()
        .prefix(".tui-command-")
        .disable_cleanup(true)
        .make_in(path.parent().unwrap(), |candidate| {
            OpenOptions::new()
                .write(true)
                .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
                .share_mode(FILE_SHARE_READ.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .create_new(true)
                .open(candidate)
        })?;
    let mut stage = Stage {
        file: temporary.into_file(),
        published: false,
    };
    stage.file.write_all(bytes)?;
    stage.file.sync_all()?;
    directory.allow_child_renames()?;
    Ok(stage)
}

fn backup(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut directory = pin_parent(path)?;
    match stage(path, bytes, &mut directory)?.publish(&directory, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn publish(path: &Path, bytes: &[u8], previous: Option<&Generation>) -> io::Result<()> {
    publish_inner(path, bytes, previous).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot publish owned TUI command {}: {error}",
                path.display()
            ),
        )
    })
}

fn publish_inner(path: &Path, bytes: &[u8], previous: Option<&Generation>) -> io::Result<()> {
    let mut directory = pin_parent(path)?;
    let mut temporary = stage(path, bytes, &mut directory)?;
    let Some(previous) = previous else {
        return temporary.publish(&directory, path);
    };
    let mut old = match OpenOptions::new()
        .access_mode(GENERIC_READ.0 | DELETE.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return temporary.publish(&directory, path);
        }
        Err(error) => return Err(error),
    };
    // This handle denies writes/deletes until publication ends. Hash the held
    // file, not a second path lookup that could point to a different file.
    let metadata = old.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || metadata.len() != previous.length
    {
        return Err(io::Error::other("Owned command changed before publication"));
    }
    let mut current = Vec::new();
    old.read_to_end(&mut current)?;
    if digest(&current) != previous.hash {
        return Err(io::Error::other("Owned command changed before publication"));
    }
    if current == bytes {
        return Ok(());
    }
    // Backup the same held, verified generation. A second pathname read could
    // copy an unrelated file swapped in after the initial ownership check.
    backup(&path.with_extension("before-tui"), &current)?;
    let retired = retire(&old, &directory, path, &previous.hash)?;
    publish_or_restore(temporary, &old, &directory, path, &retired)
}

fn retire(old: &File, directory: &PinnedDirectory, path: &Path, hash: &str) -> io::Result<PathBuf> {
    let prefix = format!(
        ".{}.retired-{hash}-",
        path.file_name().unwrap().to_string_lossy()
    );
    let mut attempts = 0;
    // The rename itself creates the random name exclusively; there is no
    // placeholder to unlink and no existing backup is replaced. Retain this
    // file across success, failure and crashes while its image may be mapped.
    let retired = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".exe")
        .disable_cleanup(true)
        .make_in(path.parent().unwrap(), |candidate| {
            attempts += 1;
            if attempts > 8 {
                return Err(io::Error::other("Could not reserve a retired command name"));
            }
            rename_no_replace(old, directory, candidate)?;
            Ok(())
        })?;
    Ok(retired.path().to_path_buf())
}

fn publish_or_restore(
    mut temporary: Stage,
    old: &File,
    directory: &PinnedDirectory,
    path: &Path,
    retired: &Path,
) -> io::Result<()> {
    match temporary.publish(directory, path) {
        Ok(()) => Ok(()),
        Err(error) => Err(restore_after_failure(old, directory, path, retired, error)),
    }
}

fn restore_after_failure(
    old: &File,
    directory: &PinnedDirectory,
    path: &Path,
    retired: &Path,
    error: io::Error,
) -> io::Error {
    // A concurrent file at the original name must survive both the attempted
    // publication and rollback. One bounded rollback is sufficient; the pending
    // registry permits another repair.
    let recovery = match rename_no_replace(old, directory, path) {
        Ok(()) => "Previous command restored; repeat repair.".to_owned(),
        Err(rollback) => format!(
            "Previous command retained at {} (rollback: {rollback}); repeat repair after resolving the entry conflict.",
            retired.display()
        ),
    };
    io::Error::new(error.kind(), format!("{error}; {recovery}"))
}

fn rename_no_replace(file: &File, directory: &PinnedDirectory, target: &Path) -> io::Result<()> {
    if target.parent() != Some(directory.path.as_path()) || !target.is_absolute() {
        return Err(io::Error::other("Retired command escaped its directory"));
    }
    let name: Vec<u16> = target.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::other("Invalid command filename"));
    }
    let filename_bytes = (name.len() * size_of::<u16>()) as u32;
    let length =
        offset_of!(FILE_RENAME_INFO, FileName) + filename_bytes as usize + size_of::<u16>();
    // usize supplies the alignment required by the HANDLE in FILE_RENAME_INFO.
    let mut buffer = vec![0usize; length.div_ceil(size_of::<usize>())];
    unsafe {
        let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = HANDLE::default();
        (*information).FileNameLength = filename_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfo,
            information.cast(),
            length as u32,
        )
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_parent_cannot_be_changed_to_a_junction_before_staging() {
        use windows::Win32::{
            Foundation::GENERIC_WRITE,
            Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_WRITE},
        };
        let temporary = tempfile::tempdir().unwrap();
        let entry = temporary.path().join("empty/dodex.exe");
        let mut directory = pin_parent(&entry).unwrap();
        // Setting a directory's reparse data needs a write handle. A pin that
        // requests only attributes would incorrectly allow this open.
        let error = OpenOptions::new()
            .access_mode(GENERIC_WRITE.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(entry.parent().unwrap())
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32));
        let mut staged = stage(&entry, b"new", &mut directory).unwrap();
        let staged_path = fs::read_dir(entry.parent().unwrap())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(fs::remove_file(&staged_path).is_err());
        assert!(fs::rename(entry.parent().unwrap(), temporary.path().join("moved")).is_err());
        staged.publish(&directory, &entry).unwrap();
        assert_eq!(fs::read(entry).unwrap(), b"new");
    }

    fn held(path: &Path) -> File {
        OpenOptions::new()
            .access_mode(GENERIC_READ.0 | DELETE.0)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .unwrap()
    }

    #[test]
    fn publication_preserves_changed_or_new_user_entries_and_existing_backup() {
        let temporary = tempfile::tempdir().unwrap();
        let entry = temporary.path().join("dodex.exe");
        let previous = Generation {
            hash: digest(b"owned-old"),
            length: 9,
        };
        fs::write(&entry, b"user-data").unwrap();
        assert!(
            publish(&entry, b"new", Some(&previous))
                .unwrap_err()
                .to_string()
                .contains("changed before publication")
        );
        assert!(publish(&entry, b"new", None).is_err());
        assert_eq!(fs::read(&entry).unwrap(), b"user-data");
        let saved = entry.with_extension("before-tui");
        assert!(!saved.exists(), "Unverified content must not be backed up");
        fs::write(&saved, b"existing backup").unwrap();
        backup(&saved, b"replacement").unwrap();
        assert_eq!(fs::read(saved).unwrap(), b"existing backup");
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_publication_restores_old_entry_or_preserves_competing_file() {
        for competing in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let entry = temporary.path().join("dodex.exe");
            fs::write(&entry, b"old").unwrap();
            let mut directory = pin_parent(&entry).unwrap();
            let file = held(&entry);
            let staged = stage(&entry, b"new", &mut directory).unwrap();
            let retired = retire(&file, &directory, &entry, &digest(b"old")).unwrap();
            let error = if competing {
                fs::write(&entry, b"unrelated new file").unwrap();
                publish_or_restore(staged, &file, &directory, &entry, &retired).unwrap_err()
            } else {
                let error = restore_after_failure(
                    &file,
                    &directory,
                    &entry,
                    &retired,
                    io::Error::other("synthetic publish failure"),
                );
                drop(staged);
                error
            };
            if competing {
                assert_eq!(fs::read(&entry).unwrap(), b"unrelated new file");
                assert_eq!(fs::read(&retired).unwrap(), b"old");
                assert!(error.to_string().contains(&retired.display().to_string()));
            } else {
                assert_eq!(fs::read(&entry).unwrap(), b"old");
                assert!(!retired.exists());
                assert!(error.to_string().contains("Previous command restored"));
            }
        }
    }

    #[test]
    fn held_identity_and_parent_cannot_be_changed_and_retirement_never_replaces() {
        let temporary = tempfile::tempdir().unwrap();
        let bin = temporary.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let entry = bin.join("dodex.exe");
        fs::write(&entry, b"old").unwrap();
        let mut directory = pin_parent(&entry).unwrap();
        let file = held(&entry);
        let _staged = stage(&entry, b"new", &mut directory).unwrap();
        assert!(fs::write(&entry, b"changed").is_err());
        assert!(fs::rename(&entry, bin.join("swapped.exe")).is_err());
        assert!(fs::rename(&bin, temporary.path().join("moved")).is_err());
        let occupied = bin.join("retired.exe");
        fs::write(&occupied, b"preexisting").unwrap();
        assert!(rename_no_replace(&file, &directory, &occupied).is_err());
        assert!(
            rename_no_replace(&file, &directory, &temporary.path().join("outside.exe")).is_err()
        );
        assert!(pin_parent(&bin.join("../outside.exe")).is_err());
        assert!(pin_parent(Path::new("relative.exe")).is_err());
        assert_eq!(fs::read(&occupied).unwrap(), b"preexisting");
        assert_eq!(fs::read(&entry).unwrap(), b"old");
        assert!(!temporary.path().join("outside.exe").exists());
        let retired = retire(&file, &directory, &entry, &digest(b"old")).unwrap();
        assert_eq!(fs::read(retired).unwrap(), b"old");
    }

    #[test]
    fn publication_rejects_redirected_parent_without_touching_target() {
        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("original");
        let linked = temporary.path().join("linked");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("dodex.exe"), b"user-data").unwrap();
        let mut command = super::super::maintenance_powershell_command().unwrap();
        command.args(["-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:AC_TUI_TEST_LINK -Target $env:AC_TUI_TEST_TARGET | Out-Null"])
            .env("AC_TUI_TEST_LINK", &linked)
            .env("AC_TUI_TEST_TARGET", &original);
        super::super::run_bounded(&mut command, std::time::Duration::from_secs(30)).unwrap();
        let previous = Generation {
            hash: digest(b"user-data"),
            length: 9,
        };
        assert!(publish(&linked.join("dodex.exe"), b"new", Some(&previous)).is_err());
        assert_eq!(fs::read(original.join("dodex.exe")).unwrap(), b"user-data");
        assert_eq!(fs::read_dir(&original).unwrap().count(), 1);
    }
}
