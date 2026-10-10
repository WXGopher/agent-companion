//! Codex hook configuration and stable executable publication.
//!
//! Hooks point at copies outside the build directory so rebuilding Companion
//! does not replace executables used by live sessions. Configuration writes
//! preserve user hooks and create backups; see the Codex installer.

use crate::home_dir;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(feature = "server")]
mod codex;
#[cfg(feature = "config-edit")]
pub mod codex_tui;
#[cfg(feature = "config-edit")]
mod files;
#[cfg(feature = "config-edit")]
pub mod profile_sync;
#[cfg(feature = "server")]
pub use codex::{CODEX_HOOKS, CodexReport, install_codex, status_codex, uninstall_codex};

/// Resolve only the process environment; Finder launches do not source shell files.
pub fn codex_home() -> io::Result<PathBuf> {
    let path = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not determine Codex home"))?;
    std::path::absolute(path)
}

/// Whether one event's hook is currently wired up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryStatus {
    pub event: String,
    pub installed: bool,
    /// The managed `command` found, when installed.
    pub command: Option<String>,
}

/// The `agent-companion-hook` executable that ships beside the running `agent-companion`.
pub fn hook_binary_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the running executable has no parent directory",
        )
    })?;
    Ok(dir.join(format!(
        "agent-companion-hook{}",
        std::env::consts::EXE_SUFFIX
    )))
}

/// Where Codex hooks find the installed executables:
/// `%LOCALAPPDATA%\AgentCompanion\bin`.
pub fn stable_bin_dir() -> io::Result<PathBuf> {
    Ok(crate::usage::agent_companion_data_dir()?.join("bin"))
}

/// Stable paths used by Codex hook installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableBinaries {
    pub agent_companion: PathBuf,
    pub hook: PathBuf,
    /// Whether anything was actually copied. False when Agent Companion is already
    /// running from the stable directory.
    pub copied: bool,
}

/// Copy the running Companion, its hook and its Codex launcher outside the
/// build directory. Running from the stable directory copies nothing.
pub fn install_binaries() -> io::Result<StableBinaries> {
    let stable_dir = stable_bin_dir()?;
    let running = std::env::current_exe()?;
    let agent_companion = stable_dir.join(binary_name("agent-companion"));
    let hook = stable_dir.join(binary_name("agent-companion-hook"));

    if same_file(&running, &agent_companion) {
        return Ok(StableBinaries {
            agent_companion,
            hook,
            copied: false,
        });
    }

    fs::create_dir_all(&stable_dir)?;
    let source_hook = hook_binary_path()?;
    let source_launcher = running.with_file_name(binary_name("agent-companion-codex"));
    if !source_hook.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is missing; Agent Companion installs the hook that ships beside it",
                source_hook.display()
            ),
        ));
    }

    if !source_launcher.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is missing; build or extract all Agent Companion binaries before installing",
                source_launcher.display()
            ),
        ));
    }

    replace_file(&running, &agent_companion)?;
    replace_file(&source_hook, &hook)?;
    replace_file(
        &source_launcher,
        &stable_dir.join(binary_name("agent-companion-codex")),
    )?;
    sweep_displaced(&stable_dir);

    Ok(StableBinaries {
        agent_companion,
        hook,
        copied: true,
    })
}

fn binary_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// Copy `source` over `target`, even when `target` is a running program.
///
/// Windows will not let a running executable be overwritten, but it will let one
/// be *renamed*: the process keeps its open handle to the file under its new
/// name and carries on. So the old copy is moved aside, the new one written in
/// its place, and the leftovers swept up on a later install once nothing holds
/// them.
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    match fs::copy(source, target) {
        Ok(_) => return Ok(()),
        Err(error)
            if error.kind() != io::ErrorKind::PermissionDenied
                && !(cfg!(windows) && error.raw_os_error() == Some(32)) =>
        {
            return Err(error);
        }
        Err(_) => {}
    }

    let displaced = target.with_extension(format!("old-{}", crate::now_unix_secs()));
    fs::rename(target, &displaced)?;
    match fs::copy(source, target) {
        Ok(_) => Ok(()),
        Err(error) => {
            // Put it back rather than leaving the user with no binary at all.
            let _ = fs::rename(&displaced, target);
            Err(error)
        }
    }
}

/// Delete the copies displaced by earlier installs, ignoring the ones still
/// running.
fn sweep_displaced(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.starts_with("old-"))
        {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Whether two paths name the same file, comparing case-insensitively because
/// Windows paths do.
fn same_file(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

#[cfg(feature = "config-edit")]
fn backup_path_for(settings_path: &Path) -> PathBuf {
    let stamp = compact_utc(crate::now_unix_secs());
    let base = settings_path.as_os_str().to_string_lossy().to_string();
    let candidate = PathBuf::from(format!("{base}.backup.{stamp}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 1..1000 {
        let candidate = PathBuf::from(format!("{base}.backup.{stamp}.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}

#[cfg(any(feature = "config-edit", test))]
fn compact_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch to a
/// proleptic-Gregorian date. Cheaper than taking on a date-time dependency for
/// the one timestamp this crate formats.
#[cfg(any(feature = "config-edit", test))]
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn replacement_handles_a_running_binary_sharing_violation() {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("new.exe");
        let target = dir.path().join("agent-companion.exe");
        fs::write(&source, b"new version").unwrap();
        fs::write(&target, b"old version").unwrap();
        // A running image permits rename/delete, but refuses writes to its file.
        let mut running = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 4)
            .open(&target)
            .unwrap();
        assert_eq!(
            fs::copy(&source, &target).unwrap_err().raw_os_error(),
            Some(32)
        );
        replace_file(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new version");
        let mut old = String::new();
        running.read_to_string(&mut old).unwrap();
        assert_eq!(old, "old version");
    }

    #[test]
    fn the_stable_directory_lives_beside_the_other_agent_companion_state() {
        // Not next to the executable, and not in the build tree: somewhere no
        // build ever writes.
        let dir = stable_bin_dir().unwrap();
        assert!(dir.ends_with("bin"));
        assert_eq!(
            dir.parent().unwrap(),
            crate::usage::agent_companion_data_dir().unwrap()
        );
    }
    #[test]
    fn compact_utc_formats_a_known_instant() {
        // 2026-08-23T11:42:33Z
        assert_eq!(compact_utc(1_787_485_353), "2026-08-23T114233Z");
        assert_eq!(compact_utc(0), "1970-01-01T000000Z");
        // A leap day, to exercise civil_from_days.
        assert_eq!(compact_utc(1_709_164_800), "2024-02-29T000000Z");
    }
}
