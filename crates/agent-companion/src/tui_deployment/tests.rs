use super::*;

pub(super) fn fixture() -> (tempfile::TempDir, Layout) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().canonicalize().unwrap().join("user");
    fs::create_dir_all(&home).unwrap();
    let layout = Layout {
        support: home.join("Library/Application Support/AgentCompanion"),
        user_home: home,
    };
    private_directory(&layout.support).unwrap();
    (temporary, layout)
}

#[test]
fn legacy_account_database_and_logs_stay_in_place_without_a_desktop() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("DodexApp/codex-home");
    let database = home.join("sqlite");
    let desktop = layout.support.join("DodexApp/desktop-data");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("auth.json"), "synthetic account must remain").unwrap();
    atomic_json(
        &layout.support.join("dual-instance.json"),
        &serde_json::json!({
            "schema":1, "enabled":false, "instance":{
                "id":"dodex", "codex_home":home, "database_dir":database,
                "desktop_user_data":desktop,
                "runtime_app":layout.user_home.join("Applications/Dodex.app"),
                "cli_path":layout.user_home.join("Applications/Dodex.app/Contents/Resources/codex")
            }
        }),
    )
    .unwrap();
    let (record, source) = migrate_legacy(&layout).unwrap();
    assert_eq!(record.dodex.codex_home, home);
    assert_eq!(record.dodex.database_dir, database);
    assert_eq!(record.dodex.log_dir, desktop.join("logs"));
    assert_eq!(record.dodex.cli_path, tui_instance::standalone_entry(&home));
    assert!(source.is_none());
    assert!(
        !record.dodex_enabled,
        "Repair must retain the monitoring preference"
    );
    assert_eq!(
        fs::read_to_string(home.join("auth.json")).unwrap(),
        "synthetic account must remain"
    );
    assert!(!layout.user_home.join("Applications").exists());
    backup_metadata(&layout).unwrap();
    write_registry(&layout, &record).unwrap();
    retire_legacy_records(&layout).unwrap();
    retire_legacy_records(&layout).unwrap();
    assert!(!layout.support.join("dual-instance.json").exists());
    assert!(
        layout
            .support
            .join("TuiMigration/dual-instance.json.before-tui")
            .is_file()
    );
}

#[test]
fn interrupted_publication_accepts_both_owned_generations_without_touching_accounts() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    let record = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: layout.secondary(home.clone(), home.join("sqlite"), home.join("log")),
        dodex_enabled: false,
    };
    publish_commands_from(&layout, &record, b"native generation 1", &[]).unwrap();
    let mut ownership: Ownership =
        serde_json::from_slice(&fs::read(layout.support.join(OWNER)).unwrap()).unwrap();
    ownership
        .hashes
        .get_mut(&record.dodex.command_path)
        .unwrap()
        .push(digest(b"native generation 2"));
    atomic_json(&layout.support.join(OWNER), &ownership).unwrap();
    atomic_json(&layout.support.join(PENDING), &record).unwrap();
    publish_commands_from(&layout, &record, b"native generation 2", &[]).unwrap();
    publish_commands_from(&layout, &record, b"native generation 2", &[]).unwrap();
    assert_eq!(
        fs::read(&record.dodex.command_path).unwrap(),
        b"native generation 2"
    );
    assert_eq!(
        fs::read(record.dodex.command_path.with_extension("before-tui")).unwrap(),
        b"native generation 1"
    );
    assert!(!home.exists());
}

#[test]
fn repair_adds_only_isolation_defaults_and_is_idempotent() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    private_directory(&home).unwrap();
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("log"));
    let original =
        "# local preferences\nmodel=\"synthetic-model\"\n[tui]\nstatus_line=[\"model\"]\n";
    fs::write(home.join("config.toml"), original).unwrap();
    configure_profile(&instance).unwrap();
    let first = fs::read(home.join("config.toml")).unwrap();
    configure_profile(&instance).unwrap();
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), first);
    let updated = String::from_utf8(first).unwrap();
    assert!(
        updated.contains("# local preferences")
            && updated.contains("synthetic-model")
            && updated.contains("status_line=[")
    );
    assert_eq!(
        fs::read_to_string(home.join("config.toml.before-tui")).unwrap(),
        original
    );
    assert!(!home.join("auth.json").exists());
}

#[cfg(windows)]
#[test]
fn repair_preserves_equivalent_windows_storage_path_spellings() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("log"));
    for path in [&home, &instance.database_dir, &instance.log_dir] {
        private_directory(path).unwrap();
    }
    let database = instance.database_dir.canonicalize().unwrap();
    let logs = instance.log_dir.canonicalize().unwrap();
    let spelling = |path: &Path, variant: usize| {
        let native = path.to_string_lossy();
        let plain = native.trim_start_matches(r"\\?\");
        match variant {
            0 => plain.replace('\\', "/"),
            1 => plain.to_ascii_uppercase(),
            _ => native.to_ascii_uppercase(),
        }
    };
    for variant in 0..3 {
        let original = format!(
            "# Keep this user's original path spelling.\ncli_auth_credentials_store = 'file'\nsqlite_home = {}\nlog_dir = {}\n",
            serde_json::to_string(&spelling(&database, variant)).unwrap(),
            serde_json::to_string(&spelling(&logs, variant)).unwrap()
        );
        fs::write(home.join("config.toml"), &original).unwrap();
        configure_profile(&instance).unwrap();
        configure_profile(&instance).unwrap();
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
        assert!(!home.join("config.toml.before-tui").exists());
    }
}

#[test]
fn repair_rejects_relative_or_different_storage_without_rewriting_config() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    private_directory(&home).unwrap();
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("log"));
    for configured_log in [
        "log".to_owned(),
        home.join("different-log").to_string_lossy().into_owned(),
    ] {
        let original = format!(
            "cli_auth_credentials_store = 'file'\nsqlite_home = {}\nlog_dir = {}\n",
            serde_json::to_string(&instance.database_dir.to_string_lossy()).unwrap(),
            serde_json::to_string(&configured_log).unwrap()
        );
        fs::write(home.join("config.toml"), &original).unwrap();
        assert!(configure_profile(&instance).is_err());
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
        assert!(!home.join("config.toml.before-tui").exists());
    }
}

#[test]
fn mismatched_isolation_or_foreign_entry_is_preserved() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    private_directory(&home).unwrap();
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("log"));
    fs::write(
        home.join("config.toml"),
        "cli_auth_credentials_store=\"keyring\"\n",
    )
    .unwrap();
    assert!(configure_profile(&instance).is_err());
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        "cli_auth_credentials_store=\"keyring\"\n"
    );
    fs::create_dir_all(layout.public_bin()).unwrap();
    fs::write(&instance.command_path, "foreign entry").unwrap();
    let record = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: instance.clone(),
        dodex_enabled: false,
    };
    assert!(publish_commands_from(&layout, &record, b"new native command", &[]).is_err());
    assert_eq!(
        fs::read_to_string(&instance.command_path).unwrap(),
        "foreign entry"
    );
}

#[test]
fn repair_accepts_a_previous_console_installed_by_install_cli() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    let record = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: layout.secondary(home.clone(), home.join("sqlite"), home.join("log")),
        dodex_enabled: true,
    };
    fs::create_dir_all(layout.public_bin()).unwrap();
    let name = record
        .dodex
        .command_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    fs::write(&record.dodex.command_path, b"previous packaged console").unwrap();
    atomic_json(
        &layout.public_bin().join(".agent-companion-cli.json"),
        &serde_json::json!({
            "schema": 1,
            "owner": "agent-companion/terminal-cli",
            "entries": { name: [digest(b"previous packaged console")] }
        }),
    )
    .unwrap();
    #[cfg(not(windows))]
    assert!(migrate_legacy(&layout).is_ok());
    publish_commands_from(&layout, &record, b"new packaged console", &[]).unwrap();
    assert_eq!(
        fs::read(&record.dodex.command_path).unwrap(),
        b"new packaged console"
    );
    assert_eq!(
        fs::read(record.dodex.command_path.with_extension("before-tui")).unwrap(),
        b"previous packaged console"
    );
    assert!(!home.exists());
}

#[cfg(windows)]
#[test]
fn repair_updates_legacy_path_aliases_on_every_generation() {
    let (_temp, layout) = fixture();
    let home = layout.support.join("Dodex/codex-home");
    let record = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: layout.secondary(home.clone(), home.join("sqlite"), home.join("log")),
        dodex_enabled: true,
    };
    // Exclude the real roaming directory: every modified entry is disposable.
    let directories: Vec<_> = windows_alias_directories(&layout)
        .into_iter()
        .filter(|path| path.starts_with(&layout.user_home))
        .collect();
    assert!(
        directories.contains(
            &layout
                .support
                .parent()
                .unwrap()
                .join("Microsoft/WindowsApps")
        )
    );
    let unrelated = layout.user_home.join(".cargo/bin");
    for directory in &directories {
        private_directory(directory).unwrap();
        let bytes: &[u8] = if *directory == unrelated {
            b"unrelated console"
        } else {
            b"legacy console"
        };
        fs::write(directory.join("dodex.exe"), bytes).unwrap();
        if *directory != unrelated {
            atomic_json(
                &directory.join("dodex.agent-companion.json"),
                &serde_json::json!({
                    "schema": 2, "owner": "agent-companion/dodex",
                    "hashes": [digest(bytes)], "entry_revision": 2, "update_route": "companion"
                }),
            )
            .unwrap();
        }
    }
    for bytes in [b"native generation 1", b"native generation 2"] {
        publish_commands_from(&layout, &record, bytes, &directories).unwrap();
        for directory in &directories {
            let expected: &[u8] = if *directory == unrelated {
                b"unrelated console"
            } else {
                bytes
            };
            assert_eq!(fs::read(directory.join("dodex.exe")).unwrap(), expected);
        }
    }
    for directory in directories.iter().filter(|path| **path != unrelated) {
        assert_eq!(
            fs::read(directory.join("dodex.before-tui")).unwrap(),
            b"legacy console"
        );
    }
    assert!(!home.exists());
}

#[cfg(windows)]
#[test]
fn primary_discovery_keeps_an_earlier_npm_command_ahead_of_native_executables() {
    let (_temp, layout) = fixture();
    let npm_bin = layout.user_home.join("npm");
    let native_bin = layout.user_home.join("other-bin");
    let package = npm_bin.join("node_modules/@openai/codex");
    let triple = if cfg!(target_arch = "aarch64") {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    };
    let native = package.join("vendor").join(triple).join("codex/codex.exe");
    fs::create_dir_all(native.parent().unwrap()).unwrap();
    fs::create_dir_all(&native_bin).unwrap();
    fs::write(&native, b"MZ\0\0").unwrap();
    fs::write(native_bin.join("codex.exe"), b"MZ\0\0").unwrap();
    fs::write(npm_bin.join("codex.cmd"), b"synthetic npm entry").unwrap();
    fs::write(npm_bin.join("npm.cmd"), b"synthetic npm updater").unwrap();
    atomic_json(
        &package.join("package.json"),
        &serde_json::json!({"name": "@openai/codex", "bin": {"codex": "bin/codex.js"}}),
    )
    .unwrap();
    let path = std::env::join_paths([&npm_bin, &native_bin]).unwrap();
    let primary = discover_primary_in(&layout, &path).unwrap().unwrap();
    assert_eq!(primary.channel, Channel::Npm);
    assert_eq!(primary.cli_path, npm_bin.join("codex.cmd"));
    assert_eq!(primary.updater, Some(npm_bin.join("npm.cmd")));
    assert_eq!(primary.runtime().unwrap(), native.canonicalize().unwrap());
}

#[cfg(windows)]
#[test]
fn maintenance_powershell_ignores_inherited_modules_without_changing_terminal_environment() {
    const CHILD: &str = "ACOMP_TUI_POWERSHELL_TEST_CHILD";
    const TEST: &str = "tui_deployment::tests::maintenance_powershell_ignores_inherited_modules_without_changing_terminal_environment";
    let inherited = std::env::var_os("PSModulePath");
    if std::env::var_os(CHILD).is_some() {
        let (temporary, layout) = fixture();
        let root = temporary.path();
        let home = layout.user_home.join("profile");
        let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("logs"));
        let mut signature = maintenance_powershell_command().unwrap();
        signature.args(["-Command", r#"
$ErrorActionPreference = 'Stop'
$signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $PSHOME 'powershell.exe')
$source = (Get-Command Get-AuthenticodeSignature).Module.Path
if ($signature.Status -ne 'Valid' -or -not $source.StartsWith((Join-Path $PSHOME 'Modules\'), [StringComparison]::OrdinalIgnoreCase)) { throw 'Wrong signature module' }
if ($env:ACOMP_TUI_TEST_INHERITED -ne 'preserved') { throw 'Lost inherited environment' }
[Console]::Out.Write('verified')
"#]);
        assert_eq!(
            run_bounded(&mut signature, Duration::from_secs(30)).unwrap(),
            b"verified"
        );

        // Exercise the installer's -File/argument/environment contract and the
        // private entry's junction cmdlet, using only disposable fixture data.
        let target = root.join("target");
        let link = root.join("entry");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("marker"), b"synthetic").unwrap();
        let script = root.join("installer-probe.ps1");
        fs::write(&script, r#"
param([string]$Release)
$ErrorActionPreference = 'Stop'
if ($Release -ne 'synthetic-release' -or $env:CODEX_HOME -ne $env:ACOMP_TUI_EXPECTED_HOME -or $env:CODEX_SQLITE_HOME -ne $env:ACOMP_TUI_EXPECTED_SQLITE -or $env:CODEX_INSTALL_DIR -ne $env:ACOMP_TUI_EXPECTED_INSTALL) { throw 'Lost installer arguments or environment' }
$null = Get-Command Invoke-RestMethod, Invoke-WebRequest, Expand-Archive, Start-Process
$null = ConvertFrom-Json '{"synthetic":true}'
New-Item -ItemType Junction -Path $env:ACOMP_TUI_TEST_LINK -Target $env:ACOMP_TUI_TEST_TARGET | Out-Null
[Console]::Out.Write('installed')
"#).unwrap();
        let mut installer = maintenance_powershell_command().unwrap();
        instance.environment(&mut installer);
        installer
            .args(["-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .args(["-Release", "synthetic-release"])
            .env("ACOMP_TUI_EXPECTED_HOME", &instance.codex_home)
            .env("ACOMP_TUI_EXPECTED_SQLITE", &instance.database_dir)
            .env("ACOMP_TUI_EXPECTED_INSTALL", &instance.install_dir)
            .env("ACOMP_TUI_TEST_LINK", &link)
            .env("ACOMP_TUI_TEST_TARGET", &target);
        assert_eq!(
            run_bounded(&mut installer, Duration::from_secs(30)).unwrap(),
            b"installed"
        );
        assert_eq!(fs::read(link.join("marker")).unwrap(), b"synthetic");

        // The terminal launcher must still pass user modules to its native
        // child. WinPS may add its default paths during startup; compare with
        // that shell's inherited environment, including every supplied path.
        let output = root.join("terminal-environment.txt");
        let expected = root.join("shell-environment.txt");
        let mut terminal = powershell_command().unwrap();
        terminal.args(["-Command", r#"
$ErrorActionPreference = 'Stop'
[IO.File]::WriteAllText($env:ACOMP_TUI_TEST_EXPECTED, $env:PSModulePath)
$process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\cmd.exe') -ArgumentList @('/d', '/u', '/c', 'set PSModulePath') -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput $env:ACOMP_TUI_TEST_OUTPUT
if ($process.ExitCode -ne 0) { throw 'Native child failed' }
"#])
            .env("ACOMP_TUI_TEST_OUTPUT", &output)
            .env("ACOMP_TUI_TEST_EXPECTED", &expected);
        run_bounded(&mut terminal, Duration::from_secs(30)).unwrap();
        let output = fs::read(output).unwrap();
        let (code_units, remainder) = output.as_chunks::<2>();
        assert!(remainder.is_empty(), "Incomplete UTF-16 code unit");
        let output = String::from_utf16(
            &code_units
                .iter()
                .copied()
                .map(u16::from_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let (name, value) = output.trim().split_once('=').unwrap();
        assert!(name.eq_ignore_ascii_case("PSModulePath"));
        assert_eq!(value, fs::read_to_string(expected).unwrap());
        let paths: Vec<_> = std::env::split_paths(value).collect();
        for path in std::env::split_paths(inherited.as_ref().unwrap()) {
            assert!(
                paths.contains(&path),
                "Terminal lost an inherited module path"
            );
        }
    } else {
        // Set the parent environment in a separate test process, never with
        // process-global set_var while the remaining Rust tests run in parallel.
        let temporary = tempfile::tempdir().unwrap();
        let modules = temporary.path().join("modules-模块");
        let security = modules.join("Microsoft.PowerShell.Security");
        fs::create_dir_all(&security).unwrap();
        fs::write(
            security.join("Microsoft.PowerShell.Security.psd1"),
            "@{ RootModule='Security.psm1'; ModuleVersion='99.0'; FunctionsToExport=@('Get-AuthenticodeSignature') }",
        )
        .unwrap();
        fs::write(
            security.join("Security.psm1"),
            "function Get-AuthenticodeSignature { throw 'Inherited signature module was loaded' }; Export-ModuleMember -Function Get-AuthenticodeSignature",
        )
        .unwrap();
        let mut paths = vec![modules];
        let mut pwsh = Command::new("pwsh.exe");
        pwsh.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write($PSHOME)",
        ]);
        match run_bounded(&mut pwsh, Duration::from_secs(30)) {
            Ok(bytes) => {
                let path = PathBuf::from(String::from_utf8(bytes).unwrap()).join("Modules");
                assert!(path.join("Microsoft.PowerShell.Security").is_dir());
                paths.insert(0, path);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => panic!("PowerShell 7 module discovery failed: {error}"),
        }
        if let Some(value) = &inherited {
            paths.extend(std::env::split_paths(value));
        }
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "1")
            .env("PSModulePath", std::env::join_paths(paths).unwrap())
            .env("ACOMP_TUI_TEST_INHERITED", "preserved");
        run_bounded(&mut child, Duration::from_secs(120)).unwrap();
    }
    assert_eq!(std::env::var_os("PSModulePath"), inherited);
}

#[cfg(windows)]
#[test]
fn repair_replaces_running_windows_alias_without_stopping_its_process() {
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    use windows::Win32::{
        Foundation::{FILETIME, HANDLE},
        System::Threading::GetProcessTimes,
    };

    struct LiveFixture(std::process::Child);
    impl LiveFixture {
        fn creation_time(&self) -> u64 {
            let mut created = FILETIME::default();
            let mut exited = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            unsafe {
                GetProcessTimes(
                    HANDLE(self.0.as_raw_handle()),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
                .unwrap();
            }
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
        }
    }
    impl Drop for LiveFixture {
        fn drop(&mut self) {
            if let Some(mut input) = self.0.stdin.take() {
                let _ = input.write_all(b"exit\r\n");
            }
            for _ in 0..500 {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // This handle belongs solely to the fixture child created below.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let (_temporary, layout) = fixture();
    let home = layout.user_home.join("profile");
    let record = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: layout.secondary(home.clone(), home.join("sqlite"), home.join("logs")),
        dodex_enabled: true,
    };
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    let old = fs::read(system.join("cmd.exe")).unwrap();
    let new = fs::read(system.join("hostname.exe")).unwrap();
    publish_commands_from(&layout, &record, &new, &[]).unwrap();
    let aliases = layout.user_home.join(".cargo/bin");
    fs::create_dir_all(&aliases).unwrap();
    let alias = aliases.join("dodex.exe");
    fs::write(&alias, &old).unwrap();
    atomic_json(
        &aliases.join("dodex.agent-companion.json"),
        &serde_json::json!({"schema":2,"owner":"agent-companion/dodex","hashes":[digest(&old)]}),
    )
    .unwrap();
    let backup = alias.with_extension("before-tui");
    fs::write(&backup, &old).unwrap();
    let mut process = LiveFixture(
        Command::new(&alias)
            .args(["/d", "/q"])
            .current_dir(&layout.user_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap(),
    );
    let pid = process.0.id();
    let started = process.creation_time();
    assert!(process.0.try_wait().unwrap().is_none());
    assert_eq!(
        atomic_write(&alias, &new, 0o755)
            .unwrap_err()
            .raw_os_error(),
        Some(5)
    );
    assert_eq!(digest(&fs::read(&alias).unwrap()), digest(&old));

    for _ in 0..2 {
        publish_commands_from(&layout, &record, &new, std::slice::from_ref(&aliases)).unwrap();
        assert!(process.0.try_wait().unwrap().is_none());
        assert_eq!(process.0.id(), pid);
        assert_eq!(process.creation_time(), started);
        assert_eq!(digest(&fs::read(&alias).unwrap()), digest(&new));
        assert_eq!(
            digest(&fs::read(&record.dodex.command_path).unwrap()),
            digest(&new)
        );
        assert_eq!(digest(&fs::read(&backup).unwrap()), digest(&old));
        let retired: Vec<_> = fs::read_dir(&aliases)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".dodex.exe.retired-")
            })
            .collect();
        assert_eq!(
            retired.len(),
            1,
            "Same-generation repair must not retire another file"
        );
        assert_eq!(digest(&fs::read(&retired[0]).unwrap()), digest(&old));
        let owner: Ownership =
            serde_json::from_slice(&fs::read(layout.support.join(OWNER)).unwrap()).unwrap();
        assert!(owner.hashes[&alias].contains(&digest(&old)));
        assert!(owner.hashes[&alias].contains(&digest(&new)));
        assert!(!owner.hashes.contains_key(&retired[0]));
    }
    assert!(!home.exists());
}
