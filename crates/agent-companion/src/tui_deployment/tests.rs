use super::*;

fn fixture() -> (tempfile::TempDir, Layout) {
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
    publish_commands_from(&layout, &record, b"native generation 1").unwrap();
    let mut ownership: Ownership =
        serde_json::from_slice(&fs::read(layout.support.join(OWNER)).unwrap()).unwrap();
    ownership
        .hashes
        .get_mut(&record.dodex.command_path)
        .unwrap()
        .push(digest(b"native generation 2"));
    atomic_json(&layout.support.join(OWNER), &ownership).unwrap();
    atomic_json(&layout.support.join(PENDING), &record).unwrap();
    publish_commands_from(&layout, &record, b"native generation 2").unwrap();
    publish_commands_from(&layout, &record, b"native generation 2").unwrap();
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
    assert!(publish_commands_from(&layout, &record, b"new native command").is_err());
    assert_eq!(
        fs::read_to_string(&instance.command_path).unwrap(),
        "foreign entry"
    );
}
