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
