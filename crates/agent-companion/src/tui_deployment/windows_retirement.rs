//! Remove only hash-owned legacy desktop entry points after TUI publication.
//! Account storage and running processes are never removed or terminated.
use super::*;
use windows::Win32::{
    System::Com::CoTaskMemFree,
    UI::Shell::{FOLDERID_Programs, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
};

#[derive(Serialize, Deserialize)]
struct Entry {
    source: PathBuf,
    backup: PathBuf,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    schema: u32,
    complete: bool,
    entries: Vec<Entry>,
}

pub(super) fn retire(layout: &Layout, record: &Registry) -> io::Result<()> {
    let recovery = layout.support.join("TuiMigration");
    if !recovery
        .join("Dodex-companion-deployment.json.before-tui")
        .is_file()
    {
        return Ok(());
    }
    let root = record
        .dodex
        .codex_home
        .parent()
        .ok_or_else(|| io::Error::other("Invalid legacy desktop root"))?;
    let desktop = root.join("dodex.exe");
    // Match the native known folder used by the old desktop installer; the
    // Programs folder may have been redirected independently of APPDATA.
    let programs = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
        .map_err(io::Error::other)?;
    let path = unsafe { programs.to_string() };
    unsafe { CoTaskMemFree(Some(programs.0.cast())) };
    let shortcut = PathBuf::from(path.map_err(io::Error::other)?).join("Dodex.lnk");
    retire_entries(layout, &desktop, &shortcut)
}

fn retire_entries(layout: &Layout, desktop: &Path, shortcut: &Path) -> io::Result<()> {
    let recovery = layout.support.join("TuiMigration");
    let path = recovery.join("windows-desktop-retirement.json");
    let journal: Journal = if path.exists() {
        serde_json::from_slice(&read_limited(&path, 64 * 1024)?)?
    } else {
        if !desktop.is_file()
            || !legacy_entry(layout, desktop, &read_limited(desktop, 512 * 1024 * 1024)?)?
        {
            return Ok(());
        }
        let mut sources = Vec::new();
        if shortcut.is_file() && owned_shortcut(shortcut, desktop)? {
            sources.push((shortcut, "Dodex.lnk.before-tui"));
        }
        sources.push((desktop, "Dodex-desktop.exe.before-tui"));
        let entries = sources
            .into_iter()
            .map(|(source, name)| {
                Ok(Entry {
                    source: source.to_path_buf(),
                    backup: recovery.join(name),
                    sha256: digest(&read_limited(source, 512 * 1024 * 1024)?),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let journal = Journal {
            schema: 1,
            complete: false,
            entries,
        };
        atomic_json(&path, &journal)?;
        journal
    };
    if journal.schema != 1 || journal.entries.is_empty() {
        return Err(io::Error::other("Invalid desktop recovery record"));
    }
    for entry in &journal.entries {
        let name = if entry.source == desktop {
            "Dodex-desktop.exe.before-tui"
        } else if entry.source == shortcut {
            "Dodex.lnk.before-tui"
        } else {
            return Err(io::Error::other("Desktop recovery source changed"));
        };
        if entry.backup != recovery.join(name) {
            return Err(io::Error::other("Desktop recovery destination changed"));
        }
        if entry.source.exists() {
            let bytes = read_limited(&entry.source, 512 * 1024 * 1024)?;
            if digest(&bytes) != entry.sha256 {
                return Err(io::Error::other(
                    "Legacy desktop entry changed; it was preserved",
                ));
            }
            if !entry.backup.exists() {
                atomic_write(&entry.backup, &bytes, 0o600)?;
            }
        }
        if digest(&read_limited(&entry.backup, 512 * 1024 * 1024)?) != entry.sha256 {
            return Err(io::Error::other("Desktop recovery backup does not match"));
        }
        if entry.source.exists() {
            fs::remove_file(&entry.source).map_err(|e| io::Error::new(e.kind(), format!(
                "Close the old Dodex desktop and repeat repair; its recovery copy is preserved: {e}")))?;
        }
    }
    atomic_json(
        &path,
        &Journal {
            complete: true,
            ..journal
        },
    )
}

fn owned_shortcut(shortcut: &Path, desktop: &Path) -> io::Result<bool> {
    no_redirects(shortcut)?;
    let mut command = powershell_command()?;
    command.args(["-Command", "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); $l=(New-Object -ComObject WScript.Shell).CreateShortcut($env:AC_TUI_OLD_LINK); if ([string]::IsNullOrEmpty($l.Arguments)) { [Console]::Out.Write($l.TargetPath) }"])
        .env("AC_TUI_OLD_LINK", shortcut.to_string_lossy().trim_start_matches(r"\\?\"));
    let bytes = run_bounded(&mut command, Duration::from_secs(30))?;
    let target = PathBuf::from(String::from_utf8(bytes).map_err(io::Error::other)?);
    if !target.is_absolute() {
        return Ok(false);
    }
    no_redirects(&target)?;
    Ok(match (target.canonicalize(), desktop.canonicalize()) {
        (Ok(target), Ok(desktop)) => target == desktop,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_desktop_retirement_is_recoverable_and_does_not_touch_accounts() {
        let (_temporary, layout) = super::super::tests::fixture();
        let root = layout.support.join("Dodex");
        private_directory(&root).unwrap();
        let desktop = root.join("dodex.exe");
        let bytes = b"owned legacy desktop fixture";
        fs::write(&desktop, bytes).unwrap();
        atomic_json(
            &root.join("dodex.agent-companion.json"),
            &serde_json::json!({
                "schema": 1, "owner": "agent-companion/dodex", "hashes": [digest(bytes)]
            }),
        )
        .unwrap();
        let account = root.join("codex-home/auth.json");
        fs::create_dir_all(account.parent().unwrap()).unwrap();
        fs::write(&account, b"synthetic account remains").unwrap();
        let shortcut = root.join("Dodex.lnk");
        let mut command = powershell_command().unwrap();
        command.args(["-Command", "$ErrorActionPreference='Stop'; $l=(New-Object -ComObject WScript.Shell).CreateShortcut($env:AC_TUI_TEST_LINK); $l.TargetPath=$env:AC_TUI_TEST_DESKTOP; $l.Save()"])
            .env("AC_TUI_TEST_LINK", shortcut.to_string_lossy().trim_start_matches(r"\\?\"))
            .env("AC_TUI_TEST_DESKTOP", desktop.to_string_lossy().trim_start_matches(r"\\?\"));
        run_bounded(&mut command, Duration::from_secs(30)).unwrap();
        retire_entries(&layout, &desktop, &shortcut).unwrap();
        retire_entries(&layout, &desktop, &shortcut).unwrap();
        assert!(!desktop.exists());
        assert!(!shortcut.exists());
        assert!(
            layout
                .support
                .join("TuiMigration/Dodex.lnk.before-tui")
                .is_file()
        );
        assert_eq!(
            fs::read(
                layout
                    .support
                    .join("TuiMigration/Dodex-desktop.exe.before-tui")
            )
            .unwrap(),
            bytes
        );
        assert_eq!(fs::read(account).unwrap(), b"synthetic account remains");
    }

    #[test]
    fn unrelated_desktop_entries_are_preserved() {
        let (_temporary, layout) = super::super::tests::fixture();
        let desktop = layout.support.join("Dodex/dodex.exe");
        fs::create_dir_all(desktop.parent().unwrap()).unwrap();
        fs::write(&desktop, b"unrelated program").unwrap();
        let shortcut = layout.support.join("Dodex.lnk");
        fs::write(&shortcut, b"unrelated shortcut").unwrap();
        retire_entries(&layout, &desktop, &shortcut).unwrap();
        assert_eq!(fs::read(desktop).unwrap(), b"unrelated program");
        assert_eq!(fs::read(shortcut).unwrap(), b"unrelated shortcut");
        assert!(
            !layout
                .support
                .join("TuiMigration/windows-desktop-retirement.json")
                .exists()
        );
    }
}
