//! Deployment metadata remains discoverable even when resident monitoring is off.
use agent_companion_core::resume::Environment;
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub struct Discovery {
    pub environments: Vec<Environment>,
    pub managed_root: PathBuf,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct SavedDeployment {
    schema: u32,
    instance: SavedInstance,
    archive_hash: Option<String>,
    desktop: Option<SavedDesktop>,
    tui: Option<SavedTui>,
}

#[derive(Deserialize)]
struct SavedInstance {
    codex_home: PathBuf,
    database_dir: PathBuf,
    cli_path: PathBuf,
    desktop_user_data: Option<PathBuf>,
    runtime_app: Option<PathBuf>,
}

#[derive(Deserialize)]
struct SavedDesktop {
    package: PathBuf,
    version: String,
    archive_hash: String,
}

#[derive(Deserialize)]
struct SavedTui {
    package: PathBuf,
    version: String,
    hash: String,
}

// The console binary deliberately does not link the Windows deployment/UI
// backend. Verify its schema-2 bindings here without launching or modifying it.
fn supported_schema(record: &Path, saved: &SavedDeployment) -> bool {
    if saved.schema == 1 {
        return true;
    }
    if saved.schema != 2 {
        return false;
    }
    let (Some(root), Some(desktop)) = (record.parent(), &saved.desktop) else {
        return false;
    };
    let package_at = |package: &Path, directory: &str, leaf: &str| {
        package.is_absolute()
            && !package.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            && package.file_name().is_some_and(|name| name == leaf)
            && package.parent().and_then(Path::parent) == Some(root.join(directory).as_path())
    };
    let hash = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    if saved.instance.codex_home != root.join("codex-home")
        || saved.instance.database_dir != root.join("codex-home/sqlite")
        || saved.instance.desktop_user_data.as_deref() != Some(root.join("desktop-data").as_path())
        || !package_at(&desktop.package, "desktop-packages", "runtime")
        || saved.instance.runtime_app.as_deref()
            != Some(desktop.package.join("ChatGPT.exe").as_path())
        || desktop.version.is_empty()
        || !hash(&desktop.archive_hash)
        || saved.archive_hash.as_deref() != Some(desktop.archive_hash.as_str())
    {
        return false;
    }
    match &saved.tui {
        Some(tui) => {
            package_at(&tui.package, "tui-packages", "package")
                && semver::Version::parse(&tui.version).is_ok()
                && hash(&tui.hash)
                && saved.instance.cli_path == tui.package.join("bin/codex.exe")
        }
        // Desktop migration may precede TUI migration. The complete old
        // runtime remains in place until the independent TUI package is bound.
        None => saved.instance.cli_path == root.join("runtime/resources/codex.exe"),
    }
}

pub fn discover() -> io::Result<Discovery> {
    let home = user_home()?;
    #[cfg(target_os = "macos")]
    let support = home.join("Library/Application Support/AgentCompanion");
    #[cfg(windows)]
    let support = PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::other("LOCALAPPDATA is unavailable"))?,
    )
    .join("AgentCompanion");
    #[cfg(not(any(target_os = "macos", windows)))]
    let support = home.join(".local/share/agent-companion");

    let primary_home = home.join(".codex");
    let primary_runtime =
        primary_executable(&home).unwrap_or_else(|| standalone_executable(&primary_home));
    let mut environments = vec![Environment {
        id: "codex".into(),
        label: "Codex".into(),
        home: primary_home.clone(),
        executable: primary_runtime,
        database_home: None,
    }];
    let mut warnings = Vec::new();
    #[cfg(target_os = "macos")]
    let record = support.join("dual-instance.json");
    #[cfg(not(target_os = "macos"))]
    let record = support.join("Dodex/companion-deployment.json");
    #[cfg(target_os = "macos")]
    let managed_entry = crate::managed_tui::command(&home, "dodex");
    #[cfg(not(target_os = "macos"))]
    let managed_entry: Option<PathBuf> = None;
    match saved_environment(&record, &primary_home, managed_entry.as_deref()) {
        Ok(Some(environment)) => environments.push(environment),
        Ok(None) => (),
        Err(error) => warnings.push(error.to_string()),
    }
    Ok(Discovery {
        environments,
        managed_root: support.join("Resume"),
        warnings,
    })
}

pub fn user_home() -> io::Result<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("User home directory is unavailable"))?;
    if !home.is_absolute() {
        return Err(io::Error::other("User home directory must be absolute"));
    }
    Ok(home)
}

fn saved_environment(
    record: &Path,
    primary_home: &Path,
    _managed_entry: Option<&Path>,
) -> io::Result<Option<Environment>> {
    let metadata = match fs::symlink_metadata(record) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(io::Error::other(
                "Could not inspect the saved Dodex deployment",
            ));
        }
    };
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(io::Error::other(
            "Saved Dodex deployment is not a regular, supported metadata file",
        ));
    }
    let bytes = fs::read(record)?;
    let saved: SavedDeployment = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Saved Dodex deployment metadata is invalid"))?;
    if !supported_schema(record, &saved)
        || [
            &saved.instance.codex_home,
            &saved.instance.database_dir,
            &saved.instance.cli_path,
        ]
        .iter()
        .any(|path| !path.is_absolute())
    {
        return Err(io::Error::other(
            "Saved Dodex deployment schema or paths are unsupported",
        ));
    }
    let secondary = saved
        .instance
        .codex_home
        .canonicalize()
        .unwrap_or_else(|_| saved.instance.codex_home.clone());
    let primary = primary_home
        .canonicalize()
        .unwrap_or_else(|_| primary_home.to_path_buf());
    if primary.starts_with(&secondary) || secondary.starts_with(&primary) {
        return Err(io::Error::other(
            "Saved Codex and Dodex homes overlap; Dodex was not selected",
        ));
    }
    let executable = saved.instance.cli_path;
    #[cfg(target_os = "macos")]
    let executable = if let Some(entry) = _managed_entry {
        let home = primary_home
            .parent()
            .ok_or_else(|| io::Error::other("Primary home is invalid"))?;
        match crate::managed_tui::read_binding(home, entry).map_err(io::Error::other)? {
            Some(binding) => {
                let desktop = saved.instance.desktop_user_data.as_ref().ok_or_else(|| {
                    io::Error::other(
                        "Saved Dodex desktop profile is missing; managed runtime was not selected",
                    )
                })?;
                binding
                    .validate_profile(
                        &saved.instance.codex_home,
                        &saved.instance.database_dir,
                        desktop,
                    )
                    .map_err(io::Error::other)?;
                binding.executable().map_err(io::Error::other)?
            }
            None => executable,
        }
    } else {
        executable
    };
    Ok(Some(Environment {
        id: "dodex".into(),
        label: "Dodex".into(),
        home: saved.instance.codex_home,
        executable,
        database_home: Some(saved.instance.database_dir),
    }))
}

fn standalone_executable(home: &Path) -> PathBuf {
    home.join("packages/standalone/current/bin")
        .join(if cfg!(windows) { "codex.exe" } else { "codex" })
}

#[cfg(target_os = "macos")]
fn primary_executable(home: &Path) -> Option<PathBuf> {
    primary_executable_in(Path::new("/Applications"), &home.join("Applications"))
}

#[cfg(target_os = "macos")]
fn primary_executable_in(system: &Path, user: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    // The standalone console entry must not link the menu bar's Swift bridge,
    // whose callbacks belong to the GUI binary. plutil is the system parser for
    // both XML and binary plists; only fixed primary application paths are read.
    let regular = |path: &Path, executable: bool| {
        path.canonicalize().is_ok_and(|canonical| canonical == path)
            && fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && (!executable || metadata.permissions().mode() & 0o111 != 0)
            })
    };
    for app in [
        system.join("Codex.app"),
        user.join("Codex.app"),
        system.join("ChatGPT.app"),
        user.join("ChatGPT.app"),
    ] {
        let plist = app.join("Contents/Info.plist");
        let runtime = app.join("Contents/Resources/codex");
        if !regular(&plist, false)
            || !fs::metadata(&plist).is_ok_and(|metadata| metadata.len() <= 1024 * 1024)
            || !regular(&app.join("Contents/MacOS/ChatGPT"), true)
            || !regular(&runtime, true)
        {
            continue;
        }
        let output = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
            .arg(&plist)
            .output()
            .ok()?;
        if output.status.success()
            && output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout) == b"com.openai.codex"
        {
            return Some(runtime);
        }
    }
    None
}

#[cfg(windows)]
fn primary_executable(_home: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    // Read the existing official AppX deployment; never deploy or launch it.
    let powershell = PathBuf::from(std::env::var_os("SystemRoot")?)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !powershell.is_absolute() {
        return None;
    }
    let output = std::process::Command::new(powershell)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $p=@(Get-AppxPackage -Name OpenAI.Codex | Where-Object { $_.PackageFamilyName -eq 'OpenAI.Codex_2p2nqsd0c76g0' -and $_.Status -eq 'Ok' }); if ($p.Count -ne 1) { exit 1 }; Write-Output (Join-Path $p[0].InstallLocation 'app/resources/codex.exe')"])
        .creation_flags(0x08000000).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(text.trim().trim_start_matches('\u{feff}'));
    (path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn primary_executable(_home: &Path) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_monitoring_does_not_hide_saved_history_environment() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let record = root.join("dual-instance.json");
        fs::write(&record, serde_json::to_vec(&serde_json::json!({
            "schema": 1, "enabled": false,
            "instance": { "codex_home": root.join("dodex/home"), "database_dir": root.join("dodex/sqlite"), "cli_path": root.join("dodex/codex") }
        })).unwrap()).unwrap();
        let found = saved_environment(&record, &root.join(".codex"), None)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "dodex");
        assert_eq!(found.home, root.join("dodex/home"));
    }

    fn windows_manifest(root: &Path) -> serde_json::Value {
        let desktop = root.join("desktop-packages/release-desktop/runtime");
        serde_json::json!({
            "schema": 2, "enabled": false, "archive_hash": "A".repeat(64),
            "instance": {
                "codex_home": root.join("codex-home"),
                "database_dir": root.join("codex-home/sqlite"),
                "desktop_user_data": root.join("desktop-data"),
                "runtime_app": desktop.join("ChatGPT.exe"),
                "cli_path": root.join("runtime/resources/codex.exe")
            },
            "desktop": {"package": desktop, "version": "26.9.1", "archive_hash": "A".repeat(64)}
        })
    }

    fn add_windows_tui(root: &Path, manifest: &mut serde_json::Value) -> PathBuf {
        let package = root.join("tui-packages/release-tui/package");
        let cli = package.join("bin/codex.exe");
        manifest["tui"] = serde_json::json!({
            "package": package, "version": "0.160.0", "hash": "a".repeat(64)
        });
        manifest["instance"]["cli_path"] = serde_json::json!(cli);
        cli
    }

    #[test]
    fn windows_schema_two_preserves_history_during_and_after_tui_migration() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let record = root.join("companion-deployment.json");
        let primary = root.join("unrelated-primary");
        let mut manifest = windows_manifest(&root);
        for expected in [
            root.join("runtime/resources/codex.exe"),
            root.join("tui-packages/release-tui/package/bin/codex.exe"),
        ] {
            if expected != root.join("runtime/resources/codex.exe") {
                assert_eq!(add_windows_tui(&root, &mut manifest), expected);
            }
            let bytes = serde_json::to_vec(&manifest).unwrap();
            fs::write(&record, &bytes).unwrap();
            let found = saved_environment(&record, &primary, None).unwrap().unwrap();
            assert_eq!(found.home, root.join("codex-home"));
            assert_eq!(found.database_home, Some(root.join("codex-home/sqlite")));
            assert_eq!(found.executable, expected);
            assert_eq!(fs::read(&record).unwrap(), bytes);
        }
    }

    #[test]
    fn windows_schema_two_rejects_unbound_paths_and_incomplete_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let record = root.join("companion-deployment.json");
        let mut good = windows_manifest(&root);
        add_windows_tui(&root, &mut good);
        for (pointer, value) in [
            ("/schema", serde_json::json!(3)),
            (
                "/instance/codex_home",
                serde_json::json!(root.join("primary")),
            ),
            (
                "/instance/database_dir",
                serde_json::json!(root.join("elsewhere")),
            ),
            (
                "/instance/desktop_user_data",
                serde_json::json!(root.join("elsewhere")),
            ),
            (
                "/instance/runtime_app",
                serde_json::json!(root.join("other.exe")),
            ),
            (
                "/instance/cli_path",
                serde_json::json!(root.join("runtime/resources/codex.exe")),
            ),
            ("/archive_hash", serde_json::json!("B".repeat(64))),
            ("/desktop/archive_hash", serde_json::json!("invalid")),
            (
                "/desktop/package",
                serde_json::json!(root.join("unmanaged/runtime")),
            ),
            (
                "/tui/package",
                serde_json::json!(root.join("desktop-packages/release-tui/package")),
            ),
            ("/tui/version", serde_json::json!("invalid")),
            ("/tui/hash", serde_json::json!("invalid")),
        ] {
            let mut invalid = good.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            fs::write(&record, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(
                saved_environment(&record, &root.join("primary"), None).is_err(),
                "accepted changed field {pointer}"
            );
        }
    }

    #[test]
    fn invalid_deployment_is_reported_without_echoing_its_contents() {
        let temporary = tempfile::tempdir().unwrap();
        let record = temporary.path().join("record.json");
        fs::write(&record, "credential-must-not-be-printed").unwrap();
        let error = saved_environment(&record, &temporary.path().join("primary"), None)
            .err()
            .unwrap();
        assert!(!error.to_string().contains("credential"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn migrated_dodex_resume_uses_validated_native_package_and_original_session_paths() {
        use crate::managed_tui::{Binding, render_wrapper};
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().canonicalize().unwrap();
        let support = home.join("Library/Application Support/AgentCompanion");
        let profile = home.join("existing-second");
        let sqlite = profile.join("sqlite");
        let desktop = home.join("Library/Application Support/Existing-Second");
        let binding = Binding {
            package: support.join("Tui/packages/release/package"),
            entry: home.join(".local/bin/dodex"),
            app: home.join("Applications/Dodex.app"),
            profile_home: profile.clone(),
            sqlite_home: sqlite.clone(),
            desktop_data: desktop.clone(),
            log_dir: desktop.join("logs"),
            original: support.join("Tui/original-dodex"),
            companion: home.join("companion"),
            version: "0.160.0".into(),
        };
        for path in [
            binding.package.join("bin"),
            binding.entry.parent().unwrap().to_path_buf(),
            sqlite.clone(),
            desktop.clone(),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let executable = binding.package.join("bin/codex");
        fs::write(
            &executable,
            "synthetic, must never execute during discovery",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&binding.entry, render_wrapper(&binding).unwrap()).unwrap();
        let record = support.join("dual-instance.json");
        fs::write(&record, serde_json::to_vec(&serde_json::json!({
            "schema": 1, "enabled": false,
            "instance": { "codex_home": profile, "database_dir": sqlite,
                "desktop_user_data": desktop, "cli_path": home.join("missing-legacy-runtime/codex") }
        })).unwrap()).unwrap();
        let before = fs::read(&record).unwrap();
        let read = || saved_environment(&record, &home.join(".codex"), Some(&binding.entry));
        let found = read().unwrap().unwrap();
        assert_eq!(found.executable, executable);
        assert_eq!(found.home, profile);
        assert_eq!(found.database_home, Some(sqlite));
        assert_eq!(fs::read(&record).unwrap(), before);

        // A changed profile or missing new package must not silently fall back
        // to the stale hidden runtime advertised by the preserved old record.
        let mut changed = binding.clone();
        changed.profile_home = home.join("different-second");
        fs::write(&binding.entry, render_wrapper(&changed).unwrap()).unwrap();
        assert!(read().is_err());
        fs::write(&binding.entry, render_wrapper(&binding).unwrap()).unwrap();
        fs::remove_file(&executable).unwrap();
        assert!(read().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn console_primary_discovery_checks_identity_without_gui_bridge() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let app = root.join("Applications/ChatGPT.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        let plist = app.join("Contents/Info.plist");
        let write_identity = |identity| {
            fs::write(&plist, format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{identity}</string></dict></plist>")).unwrap();
        };
        for path in [
            app.join("Contents/MacOS/ChatGPT"),
            app.join("Contents/Resources/codex"),
        ] {
            fs::write(&path, "synthetic, never executed").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        write_identity("unrelated.app");
        assert!(
            primary_executable_in(&root.join("Applications"), &root.join("user/Applications"))
                .is_none()
        );
        write_identity("com.openai.codex");
        assert_eq!(
            primary_executable_in(&root.join("Applications"), &root.join("user/Applications")),
            Some(app.join("Contents/Resources/codex"))
        );
    }
}
