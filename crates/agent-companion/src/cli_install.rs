//! User-level entry points with ownership checks. No shell profile is edited.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const OWNER: &str = "agent-companion/terminal-cli";
const MARKER: &str = ".agent-companion-cli.json";

struct InstallLock {
    file: fs::File,
}

impl InstallLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        if is_link(path)? {
            return Err(io::Error::other("CLI install lock is redirected"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()
            .map_err(|_| io::Error::other("Another CLI installation is in progress"))?;
        Ok(Self { file })
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // A concurrent fork can retain the open-file description until exec.
        // Closing only our handle leaves its lock alive in that interval.
        let _ = self.file.unlock();
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Ownership {
    schema: u32,
    owner: String,
    // Both generations survive an interrupted install. An unrelated entry is
    // never adopted merely because its filename is familiar.
    entries: BTreeMap<String, Vec<String>>,
}

pub fn run() -> io::Result<()> {
    let home = agent_companion_core::tui_instance::Layout::current()?.user_home;
    let executable = std::env::current_exe()?.canonicalize()?;
    let source_dir = executable
        .parent()
        .ok_or_else(|| io::Error::other("Executable has no parent directory"))?;
    #[cfg(unix)]
    let (directory, sources) = (
        home.join(".local/bin"),
        vec![
            (
                "agent-companion".to_owned(),
                source_dir.join("agent-companion"),
            ),
            ("acomp".to_owned(), source_dir.join("acomp")),
            ("dodex".to_owned(), source_dir.join("dodex")),
        ],
    );
    #[cfg(windows)]
    let (directory, sources) = {
        let _ = home;
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("LOCALAPPDATA is unavailable"))?;
        // Both installed aliases use a console subsystem. The distributed
        // full GUI executable remains available in its application directory.
        (
            local.join("AgentCompanion/bin"),
            vec![
                (
                    "agent-companion.exe".to_owned(),
                    source_dir.join("acomp.exe"),
                ),
                ("acomp.exe".to_owned(), source_dir.join("acomp.exe")),
                ("dodex.exe".to_owned(), source_dir.join("dodex.exe")),
            ],
        )
    };
    install(&directory, &sources)?;
    #[cfg(windows)]
    register_user_path(&directory)?;
    writeln!(
        io::stdout(),
        "Installed acomp, agent-companion, and dodex in {}",
        directory.display()
    )?;
    #[cfg(unix)]
    if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|entry| entry == directory)
    {
        writeln!(
            io::stdout(),
            "Add this directory to your shell PATH: {}",
            directory.display()
        )?;
    }
    #[cfg(windows)]
    writeln!(
        io::stdout(),
        "Open a new terminal to use the commands. Installed Companion console entries provide resume, install-cli, dodex-tui, help, and version; the full application keeps its existing commands."
    )?;
    Ok(())
}

fn install(directory: &Path, sources: &[(String, PathBuf)]) -> io::Result<()> {
    if !directory.is_absolute() {
        return Err(io::Error::other("CLI install directory must be absolute"));
    }
    if is_link(directory)? {
        return Err(io::Error::other("CLI install directory is redirected"));
    }
    fs::create_dir_all(directory)?;
    let lock_path = directory.join(".agent-companion-cli.lock");
    let _lock = InstallLock::acquire(&lock_path)?;
    let marker = directory.join(MARKER);
    let mut ownership = read_ownership(&marker)?;
    let mut desired = Vec::new();
    // Validate every destination before writing anything.
    for (name, source) in sources {
        if Path::new(name).file_name().and_then(|part| part.to_str()) != Some(name.as_str()) {
            return Err(io::Error::other("Invalid CLI entry name"));
        }
        let source = source.canonicalize().map_err(|_| {
            io::Error::other(format!(
                "Missing packaged CLI executable: {}",
                source.display()
            ))
        })?;
        if !source.is_file() {
            return Err(io::Error::other("Packaged CLI entry is not a regular file"));
        }
        let destination = directory.join(name);
        let expected = source_fingerprint(&source, name)?;
        if fs::symlink_metadata(&destination).is_ok() {
            let actual = fingerprint(&destination)?;
            if !ownership
                .entries
                .get(name)
                .is_some_and(|known| known.contains(&actual))
                && actual != expected
                && !(name.starts_with("dodex")
                    && crate::tui_deployment::command_is_owned(
                        &destination,
                        &fs::read(&destination)?,
                    ))
            {
                return Err(io::Error::other(format!(
                    "Refusing to replace an unrelated command: {}",
                    destination.display()
                )));
            }
        }
        ownership
            .entries
            .entry(name.clone())
            .or_default()
            .push(expected.clone());
        ownership.entries.get_mut(name).unwrap().sort();
        ownership.entries.get_mut(name).unwrap().dedup();
        desired.push((destination, source, expected));
    }
    write_ownership(&marker, &ownership)?;
    for (destination, source, expected) in desired {
        if fingerprint(&destination).ok().as_ref() == Some(&expected) {
            continue;
        }
        replace_entry(&source, &destination)?;
        if fingerprint(&destination)? != expected {
            return Err(io::Error::other("Installed CLI verification failed"));
        }
    }
    Ok(())
}

fn read_ownership(marker: &Path) -> io::Result<Ownership> {
    let metadata = match fs::symlink_metadata(marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Ownership {
                schema: 1,
                owner: OWNER.into(),
                entries: BTreeMap::new(),
            });
        }
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || is_link(marker)? || metadata.len() > 64 * 1024 {
        return Err(io::Error::other(
            "CLI ownership marker is not a regular supported file",
        ));
    }
    let ownership: Ownership = serde_json::from_slice(&fs::read(marker)?)
        .map_err(|_| io::Error::other("CLI ownership marker is invalid"))?;
    if ownership.schema != 1 || ownership.owner != OWNER {
        return Err(io::Error::other(
            "CLI destination belongs to another installer",
        ));
    }
    Ok(ownership)
}

pub(crate) fn command_is_owned(path: &Path) -> bool {
    let Some(directory) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    read_ownership(&directory.join(MARKER)).is_ok_and(|ownership| {
        fingerprint(path).is_ok_and(|actual| {
            ownership
                .entries
                .get(name)
                .is_some_and(|known| known.contains(&actual))
        })
    })
}

fn write_ownership(marker: &Path, ownership: &Ownership) -> io::Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(marker.parent().unwrap())?;
    serde_json::to_writer(&mut temporary, ownership)?;
    temporary.flush()?;
    temporary.persist(marker).map_err(|error| error.error)?;
    Ok(())
}

fn is_link(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                Ok(metadata.file_attributes() & 0x400 != 0)
            }
            #[cfg(not(windows))]
            {
                Ok(metadata.file_type().is_symlink())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn fingerprint(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        Ok(fs::read_link(path)?.to_string_lossy().into_owned())
    } else if metadata.is_file() {
        file_fingerprint(path)
    } else {
        Err(io::Error::other(
            "Existing command is not a supported CLI entry",
        ))
    }
}
#[cfg(unix)]
fn source_fingerprint(source: &Path, name: &str) -> io::Result<String> {
    if name == "dodex" {
        file_fingerprint(source)
    } else {
        Ok(source.to_string_lossy().into_owned())
    }
}
#[cfg(unix)]
fn replace_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let stage = tempfile::tempdir_in(destination.parent().unwrap())?;
    let entry = stage.path().join("entry");
    // TUI repair publishes regular, hash-owned console entries. Keep Dodex
    // independent of the Companion bundle and compatible with repeat repair.
    if destination.file_name().is_some_and(|name| name == "dodex") {
        fs::copy(source, &entry)?;
        File::open(&entry)?.sync_all()?;
    } else {
        std::os::unix::fs::symlink(source, &entry)?;
    }
    fs::rename(entry, destination)
}

fn file_fingerprint(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    if is_link(path)? {
        return Err(io::Error::other("Existing command is redirected"));
    }
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
#[cfg(windows)]
fn fingerprint(path: &Path) -> io::Result<String> {
    file_fingerprint(path)
}
#[cfg(windows)]
fn source_fingerprint(source: &Path, _name: &str) -> io::Result<String> {
    fingerprint(source)
}
#[cfg(windows)]
fn replace_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let mut stage = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
    io::copy(&mut File::open(source)?, &mut stage)?;
    stage.flush()?;
    stage.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn register_user_path(directory: &Path) -> io::Result<()> {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt, OsStringExt},
    };
    use windows::{
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, LPARAM, WPARAM},
            System::Registry::{
                HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ,
                RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
            },
            UI::WindowsAndMessaging::{
                HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
            },
        },
        core::w,
    };
    let invalid = || {
        io::Error::other(
            "Could not register the CLI directory in the user PATH; installed files are intact",
        )
    };
    let mut value = OsString::new();
    let mut kind = REG_EXPAND_SZ;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut read = false;
    for _ in 0..3 {
        let mut size = 0;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            read = true;
            break;
        }
        if result.is_err() || size > 128 * 1024 || size % 2 != 0 {
            return Err(invalid());
        }
        let mut data = vec![0u16; size as usize / 2];
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if result == ERROR_MORE_DATA {
            continue;
        }
        if result.is_err() {
            return Err(invalid());
        }
        data.truncate(size as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        if data.contains(&0) {
            return Err(invalid());
        }
        value = OsString::from_wide(&data);
        read = true;
        break;
    }
    if !read {
        return Err(invalid());
    }
    if std::env::split_paths(&value).any(|entry| {
        entry
            .to_string_lossy()
            .eq_ignore_ascii_case(&directory.to_string_lossy())
            || (kind == REG_EXPAND_SZ
                && entry
                    .to_string_lossy()
                    .eq_ignore_ascii_case(r"%LOCALAPPDATA%\AgentCompanion\bin"))
    }) {
        return Ok(());
    }
    if !value.is_empty() && value.encode_wide().last() != Some(b';' as u16) {
        value.push(";");
    }
    // REG_SZ preserves a literal % in unusual user directory names. For an
    // existing expandable PATH use LOCALAPPDATA rather than interpolating it.
    if kind == REG_SZ {
        value.push(directory);
    } else {
        value.push(r"%LOCALAPPDATA%\AgentCompanion\bin");
    }
    let data: Vec<u16> = value.encode_wide().chain(Some(0)).collect();
    if data.len() > 32767 {
        return Err(invalid());
    }
    let result = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Environment"),
            w!("Path"),
            kind.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if result.is_err() {
        return Err(invalid());
    }
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1000,
            None,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn installation_lock_releases_while_an_inherited_descriptor_remains_open() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("install.lock");
        let lock = InstallLock::acquire(&path).unwrap();
        // dup and fork retain the same open-file description. A concurrent
        // spawn can hold this description until exec, even with CLOEXEC set.
        let inherited = lock.file.try_clone().unwrap();
        assert!(InstallLock::acquire(&path).is_err());
        drop(lock);
        let next = InstallLock::acquire(&path)
            .expect("completed installation must release its lock before a forked child execs");
        drop(inherited);
        assert!(InstallLock::acquire(&path).is_err());
        drop(next);
        InstallLock::acquire(&path).unwrap();
    }

    #[test]
    fn installation_preserves_unrelated_entries_and_can_be_repeated() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join("bin");
        let source = root.join("source");
        fs::write(&source, "test executable").unwrap();
        fs::create_dir(&directory).unwrap();
        let entry = directory.join("acomp");
        fs::write(&entry, "unrelated command").unwrap();
        assert!(install(&directory, &[("acomp".into(), source.clone())]).is_err());
        assert_eq!(fs::read_to_string(&entry).unwrap(), "unrelated command");
        assert!(!directory.join(MARKER).exists());
        fs::remove_file(&entry).unwrap();
        install(&directory, &[("acomp".into(), source.clone())]).unwrap();
        install(&directory, &[("acomp".into(), source)]).unwrap();
        assert_eq!(fs::read_to_string(&entry).unwrap(), "test executable");
    }

    #[cfg(unix)]
    #[test]
    fn dodex_entry_is_regular_and_survives_a_companion_bundle_move() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join("bin");
        let source = root.join("packaged-dodex");
        fs::write(&source, "console generation 1").unwrap();
        install(&directory, &[("dodex".into(), source.clone())]).unwrap();
        let entry = directory.join("dodex");
        assert!(
            !fs::symlink_metadata(&entry)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::write(&source, "console generation 2").unwrap();
        install(&directory, &[("dodex".into(), source.clone())]).unwrap();
        install(&directory, &[("dodex".into(), source.clone())]).unwrap();
        fs::rename(&source, root.join("moved-package")).unwrap();
        assert_eq!(fs::read_to_string(&entry).unwrap(), "console generation 2");
    }
}
