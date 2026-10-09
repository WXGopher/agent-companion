use super::*;

#[cfg(unix)]
#[test]
fn launch_guard_rejects_redirected_credentials_without_reading_their_contents() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: home.join("sqlite"),
        log_dir: home.join("logs"),
    };
    let mut document = DocumentMut::new();
    for key in ISOLATION_KEYS {
        document[key] = toml_edit::value(isolation_value(key, &paths));
    }
    fs::write(home.join("config.toml"), document.to_string()).unwrap();
    let external = home.join("outside-auth");
    fs::write(&external, b"opaque synthetic credentials, not JSON").unwrap();
    std::os::unix::fs::symlink(&external, home.join("auth.json")).unwrap();
    assert!(validate_isolated_profile(&home, &paths).is_err());
    assert_eq!(
        fs::read(external).unwrap(),
        b"opaque synthetic credentials, not JSON"
    );
}

#[test]
fn launch_validation_checks_defaults_and_profiles_but_not_mcp_parameters() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: root.join("sqlite"),
        log_dir: root.join("logs"),
    };
    let mut document = DocumentMut::new();
    for key in ISOLATION_KEYS {
        document[key] = toml_edit::value(isolation_value(key, &paths));
    }
    let base = document.to_string();
    for extra in [
        "[profiles.work]\nmodel='example'\n",
        "[mcp_servers.helper.env]\nlog_dir='/service/logs'\nsqlite_home='/service/db'\n",
        "[profiles.work.mcp_servers.helper.env]\ncli_auth_credentials_store='service setting'\n",
    ] {
        validate_isolated_config(format!("{base}{extra}").as_bytes(), &paths).unwrap();
    }
    for extra in [
        "[profiles.work]\ncli_auth_credentials_store='keyring'\n",
        "profiles={work={sqlite_home='/primary/sqlite'}}\n",
        "[profiles.work]\nlog_dir='/primary/logs'\n",
    ] {
        assert!(validate_isolated_config(format!("{base}{extra}").as_bytes(), &paths).is_err());
    }
    for key in ISOLATION_KEYS {
        let missing: String = base
            .lines()
            .filter(|line| !line.starts_with(key))
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(
            validate_isolated_config(missing.as_bytes(), &paths)
                .unwrap_err()
                .to_string()
                .contains(key)
        );
    }
}

#[test]
fn storage_binding_rejects_relative_different_and_unresolved_aliases() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: root.join("sqlite"),
        log_dir: root.join("logs"),
    };
    assert!(isolation_setting_matches(
        "log_dir",
        paths.log_dir.to_str(),
        &paths
    ));
    // PathBuf::join on a verbatim Windows root normalizes '..' away. Preserve
    // the actual raw configuration value so this test exercises the guard.
    let separator = std::path::MAIN_SEPARATOR;
    let parent = PathBuf::from(format!(
        "{}{separator}absent{separator}..{separator}logs",
        root.display()
    ));
    assert!(parent.components().any(|part| part == Component::ParentDir));
    for (case, value) in [
        ("relative", PathBuf::from("logs")),
        ("different", root.join("other-logs")),
        ("raw parent", parent),
    ] {
        assert!(
            !isolation_setting_matches("log_dir", value.to_str(), &paths),
            "{case}: actual={value:?}, expected={:?}, components={:?}",
            paths.log_dir,
            value.components().collect::<Vec<_>>()
        );
    }
    fs::write(&paths.log_dir, "not a directory").unwrap();
    assert!(!isolation_setting_matches(
        "log_dir",
        paths.log_dir.to_str(),
        &paths
    ));
}

#[cfg(windows)]
#[test]
fn storage_binding_rejects_raw_mixed_separator_parent_segments() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: root.join("sqlite"),
        log_dir: root.join("logs"),
    };
    for suffix in ["/absent/../logs", r"\absent/..\logs", r"/absent\../logs"] {
        let raw = PathBuf::from(format!("{}{suffix}", root.display()));
        assert!(
            validate_path(&raw).is_err(),
            "raw parent value must be rejected: {raw:?}, components={:?}",
            raw.components().collect::<Vec<_>>()
        );
        assert!(!isolation_setting_matches("log_dir", raw.to_str(), &paths));
        let invalid_binding = IsolationPaths {
            log_dir: raw.clone(),
            ..paths.clone()
        };
        assert!(
            !isolation_setting_matches("log_dir", raw.to_str(), &invalid_binding),
            "matching malformed values cannot establish a binding: {raw:?}"
        );
    }
}

#[cfg(windows)]
#[test]
fn storage_binding_accepts_existing_native_windows_aliases_without_rewriting() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, length: u32) -> u32;
    }
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: root.join("SQLite Storage"),
        log_dir: root.join("Native Log Storage"),
    };
    fs::create_dir_all(&paths.sqlite_home).unwrap();
    fs::create_dir_all(&paths.log_dir).unwrap();
    let normal = paths
        .log_dir
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    let mut aliases = vec![
        normal.replace('\\', "/"),
        normal.to_uppercase(),
        format!(r"\\?\{}", normal.to_uppercase()),
    ];
    let long: Vec<u16> = normal.encode_utf16().chain(Some(0)).collect();
    let mut short = vec![0u16; 32768];
    let length =
        unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), short.len() as u32) };
    if length > 0 && (length as usize) < short.len() {
        aliases.push(
            OsString::from_wide(&short[..length as usize])
                .into_string()
                .unwrap(),
        );
    }
    for alias in aliases {
        let mut document = DocumentMut::new();
        document["cli_auth_credentials_store"] = toml_edit::value("file");
        document["sqlite_home"] = toml_edit::value(paths.sqlite_home.to_string_lossy().as_ref());
        document["log_dir"] = toml_edit::value(alias);
        let original = document.to_string();
        validate_isolated_config(original.as_bytes(), &paths).unwrap();
        assert!(matches_secondary_isolation(&document, &paths));
        assert_eq!(document.to_string(), original);
    }
}

#[cfg(unix)]
#[test]
fn storage_binding_rejects_symlink_aliases_even_when_they_resolve_to_the_assigned_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: root.join("sqlite"),
        log_dir: root.join("logs"),
    };
    fs::create_dir(&paths.log_dir).unwrap();
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&paths.log_dir, &alias).unwrap();
    assert!(!isolation_setting_matches(
        "log_dir",
        alias.to_str(),
        &paths
    ));
    let redirected = IsolationPaths {
        log_dir: alias,
        ..paths
    };
    assert!(!isolation_setting_matches(
        "log_dir",
        redirected.log_dir.to_str(),
        &redirected
    ));
}

#[test]
fn trusted_runtime_defaults_accept_omitted_paths_but_reject_explicit_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let paths = IsolationPaths {
        sqlite_home: home.join("sqlite"),
        log_dir: home.join("desktop/logs"),
    };
    let config = home.join("config.toml");
    let system = home.join("absent-system-config.toml");
    let base = "cli_auth_credentials_store='file'\nmodel='synthetic-model'\n";
    for explicit in [
        vec![],
        vec!["sqlite_home"],
        vec!["log_dir"],
        vec!["sqlite_home", "log_dir"],
    ] {
        let mut document: DocumentMut = base.parse().unwrap();
        for key in explicit {
            document[key] = toml_edit::value(isolation_value(key, &paths));
        }
        let contents = document.to_string();
        fs::write(&config, &contents).unwrap();
        validate_profile(&home, &paths, Some(&system)).unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), contents);
    }
    fs::write(&config, base).unwrap();
    assert!(validate_isolated_profile(&home, &paths).is_err());

    for extra in [
        "sqlite_home='/primary/sqlite'\n",
        "log_dir='/primary/log'\n",
        "sqlite_home=42\n",
        "log_dir=false\n",
        "[profiles.work]\ncli_auth_credentials_store='keyring'\n",
        "profiles={work={sqlite_home='/primary/sqlite'}}\n",
        "[profiles.work]\nlog_dir='/primary/log'\n",
    ] {
        fs::write(&config, format!("{base}{extra}")).unwrap();
        assert!(
            validate_profile(&home, &paths, Some(&system)).is_err(),
            "{extra}"
        );
    }
    for contents in [
        "model='synthetic-model'\n",
        "cli_auth_credentials_store='keyring'\n",
    ] {
        fs::write(&config, contents).unwrap();
        assert!(validate_profile(&home, &paths, Some(&system)).is_err());
    }
}

#[cfg(unix)]
#[test]
fn trusted_runtime_log_default_cannot_redirect_outside_the_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("secondary");
    let outside = root.join("primary-log");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&outside).unwrap();
    let paths = IsolationPaths {
        sqlite_home: home.join("sqlite"),
        log_dir: home.join("desktop/logs"),
    };
    fs::write(
        home.join("config.toml"),
        "cli_auth_credentials_store='file'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, home.join("log")).unwrap();
    assert!(
        validate_profile(&home, &paths, Some(&root.join("absent-system-config.toml"))).is_err()
    );
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn runtime_storage_defaults_account_for_the_effective_system_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("secondary");
    fs::create_dir(&home).unwrap();
    let config = home.join("config.toml");
    let system = root.join("synthetic-system-config.toml");
    let paths = IsolationPaths {
        sqlite_home: home.join("sqlite"),
        log_dir: home.join("desktop/logs"),
    };
    let base = "cli_auth_credentials_store='file'\nmodel='synthetic-model'\n";
    fs::write(&config, base).unwrap();
    // Both layers omit storage settings: the launcher's own home/SQLite apply.
    validate_profile(&home, &paths, Some(&system)).unwrap();

    let mut system_config = DocumentMut::new();
    for key in ["sqlite_home", "log_dir"] {
        system_config[key] = toml_edit::value(isolation_value(key, &paths));
    }
    fs::write(&system, system_config.to_string()).unwrap();
    validate_profile(&home, &paths, Some(&system)).unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), base);

    for key in ["sqlite_home", "log_dir"] {
        let mut conflicting = system_config.clone();
        conflicting[key] = toml_edit::value("/synthetic/primary");
        let contents = conflicting.to_string();
        fs::write(&system, &contents).unwrap();
        let error = validate_profile(&home, &paths, Some(&system))
            .unwrap_err()
            .to_string();
        assert!(error.contains(key));
        assert!(!error.contains("/synthetic/primary"));

        // A correct explicit user value overrides the conflicting system layer.
        let mut local: DocumentMut = base.parse().unwrap();
        local[key] = toml_edit::value(isolation_value(key, &paths));
        let local_contents = local.to_string();
        fs::write(&config, &local_contents).unwrap();
        validate_profile(&home, &paths, Some(&system)).unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), local_contents);
        assert_eq!(fs::read_to_string(&system).unwrap(), contents);
        fs::write(&config, base).unwrap();
    }
}

#[test]
fn launch_validation_parses_inline_and_quoted_profile_override_keys() {
    for raw in [
        "sqlite_home='/wrong'",
        "profiles.work.log_dir='/wrong'",
        "profiles={work={cli_auth_credentials_store='keyring'}}",
        "profiles.work={\"\\u006cog_dir\"='/wrong'}",
    ] {
        assert!(validate_isolated_overrides(&[raw.into()]).is_err(), "{raw}");
    }
    for raw in [
        "model=o3",
        "model='sqlite_home=ordinary text'",
        "profiles={work={model='o3'}}",
        "profiles.work={mcp_servers={helper={env={log_dir='service'}}}}",
        "mcp_servers.helper.env.sqlite_home='/service/db'",
    ] {
        validate_isolated_overrides(&[raw.into()]).unwrap();
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    pair: ProfilePair,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        fs::create_dir(&primary).unwrap();
        fs::create_dir(&secondary).unwrap();
        Self {
            _temp: temp,
            pair: ProfilePair {
                primary: profile_paths(&primary).unwrap(),
                secondary: profile_paths(&secondary).unwrap(),
                secondary_isolation: IsolationPaths {
                    sqlite_home: secondary.join("sqlite"),
                    log_dir: root.join("desktop-data/logs"),
                },
            },
        }
    }

    fn sync(&self, kind: FileKind, direction: Direction) -> SyncOutcome {
        sync_file(&self.pair, kind, direction).unwrap()
    }

    fn secondary_config(&self) -> String {
        let mut doc = DocumentMut::new();
        for key in ISOLATION_KEYS {
            doc[key] = toml_edit::value(isolation_value(key, &self.pair.secondary_isolation));
        }
        doc.to_string()
    }
}

fn config(path: &Path) -> DocumentMut {
    fs::read_to_string(path).unwrap().parse().unwrap()
}

#[test]
fn full_config_copy_includes_embedded_secrets_and_preserves_destination_isolation() {
    let fixture = Fixture::new();
    let source = "# User config\nmodel = 'source-model'\napi_key = 'synthetic-inline-secret'\ncli_auth_credentials_store = 'keyring'\nsqlite_home = '/primary/db'\nlog_dir = '/primary/log'\n[profiles.work]\ncli_auth_credentials_store = 'auto'\nmodel = 'profile-model'\n[mcp_servers.fixture]\nenv = { TOKEN = 'synthetic-mcp-secret' }\n";
    let destination = fixture.secondary_config()
        + "[profiles.work]\ncli_auth_credentials_store = 'file'\nmodel = 'old'\n";
    fs::write(&fixture.pair.primary.config, source).unwrap();
    fs::write(&fixture.pair.secondary.config, &destination).unwrap();
    let auth = fixture.pair.secondary.config.with_file_name("auth.json");
    fs::write(&auth, b"synthetic-auth-not-copied").unwrap();
    let outcome = fixture.sync(FileKind::Config, Direction::ToSecondary);
    assert!(outcome.changed);
    assert_eq!(
        fs::read(outcome.backup_path.unwrap()).unwrap(),
        destination.as_bytes()
    );
    assert_eq!(
        fs::read(&fixture.pair.primary.config).unwrap(),
        source.as_bytes()
    );
    assert_eq!(fs::read(auth).unwrap(), b"synthetic-auth-not-copied");
    let copied = config(&fixture.pair.secondary.config);
    assert_eq!(copied["model"].as_str(), Some("source-model"));
    assert_eq!(copied["api_key"].as_str(), Some("synthetic-inline-secret"));
    assert_eq!(
        copied["mcp_servers"]["fixture"]["env"]["TOKEN"].as_str(),
        Some("synthetic-mcp-secret")
    );
    for key in ISOLATION_KEYS {
        assert_eq!(
            copied[key].as_str(),
            Some(isolation_value(key, &fixture.pair.secondary_isolation).as_str())
        );
    }
    assert_eq!(
        copied["profiles"]["work"]["cli_auth_credentials_store"].as_str(),
        Some("file")
    );
    assert_eq!(
        copied["profiles"]["work"]["model"].as_str(),
        Some("profile-model")
    );
    assert!(
        !fixture
            .sync(FileKind::Config, Direction::ToSecondary)
            .changed
    );
}

#[test]
fn reverse_copy_retains_only_primary_isolation_including_nested_inline_and_array_overrides() {
    let fixture = Fixture::new();
    let destination = "cli_auth_credentials_store = 'keyring'\nsqlite_home = '/primary/db'\nlog_dir = '/primary/log'\nprofiles = { work = { sqlite_home = '/primary/work-db' } }\nentries = [{ log_dir = '/primary/array-log' }]\n[[nested]]\ncli_auth_credentials_store = 'auto'\n[profiles_only_in_target.special]\nlog_dir = '/primary/special-log'\n";
    let source = fixture.secondary_config()
        + "model = 'secondary-model'\nprofiles = { work = { model = 'work-model', sqlite_home = '/secondary/db' }, new = { log_dir = '/secondary/new-log' } }\nentries = [{ log_dir = '/secondary/array-log', enabled = true }]\n[[nested]]\ncli_auth_credentials_store = 'file'\nmodel = 'nested-model'\n";
    fs::write(&fixture.pair.primary.config, destination).unwrap();
    fs::write(&fixture.pair.secondary.config, source).unwrap();
    let outcome = fixture.sync(FileKind::Config, Direction::ToPrimary);
    assert_eq!(
        fs::read_to_string(outcome.backup_path.unwrap()).unwrap(),
        destination
    );
    let result = config(&fixture.pair.primary.config);
    assert_eq!(
        result["cli_auth_credentials_store"].as_str(),
        Some("keyring")
    );
    assert_eq!(result["sqlite_home"].as_str(), Some("/primary/db"));
    assert_eq!(result["log_dir"].as_str(), Some("/primary/log"));
    assert_eq!(
        result["profiles"]["work"]["sqlite_home"].as_str(),
        Some("/primary/work-db")
    );
    assert_eq!(
        result["profiles"]["work"]["model"].as_str(),
        Some("work-model")
    );
    assert_eq!(
        result["entries"]
            .as_array()
            .unwrap()
            .get(0)
            .unwrap()
            .as_inline_table()
            .unwrap()
            .get("log_dir")
            .unwrap()
            .as_str(),
        Some("/primary/array-log")
    );
    assert_eq!(
        result["nested"]
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap()["cli_auth_credentials_store"]
            .as_str(),
        Some("auto")
    );
    assert_eq!(
        result["profiles_only_in_target"]["special"]["log_dir"].as_str(),
        Some("/primary/special-log")
    );
    assert!(!result.to_string().contains("/secondary/"));
    assert!(!fixture.sync(FileKind::Config, Direction::ToPrimary).changed);
}

#[test]
fn missing_primary_config_does_not_inherit_secondary_isolation_defaults() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.pair.secondary.config,
        fixture.secondary_config()
            + "model = 'keep'\n[profiles.work]\ncli_auth_credentials_store = 'file'\n",
    )
    .unwrap();
    let outcome = fixture.sync(FileKind::Config, Direction::ToPrimary);
    assert!(outcome.changed && outcome.backup_path.is_none());
    let result = config(&fixture.pair.primary.config);
    assert_eq!(result["model"].as_str(), Some("keep"));
    assert!(!has_isolation(result.as_item()));
}

#[test]
fn missing_secondary_config_gets_required_destination_isolation() {
    let fixture = Fixture::new();
    fs::write(&fixture.pair.primary.config, "model = 'keep'\n").unwrap();
    let outcome = fixture.sync(FileKind::Config, Direction::ToSecondary);
    assert!(outcome.changed && outcome.backup_path.is_none());
    let result = config(&fixture.pair.secondary.config);
    for key in ISOLATION_KEYS {
        assert_eq!(
            result[key].as_str(),
            Some(isolation_value(key, &fixture.pair.secondary_isolation).as_str())
        );
    }
}

#[test]
fn equal_configuration_bytes_create_no_backup_or_reformatting() {
    let fixture = Fixture::new();
    let same = fixture.secondary_config() + "# Keep formatting\nmodel = 'same'\n";
    fs::write(&fixture.pair.primary.config, &same).unwrap();
    fs::write(&fixture.pair.secondary.config, &same).unwrap();
    for direction in [Direction::ToPrimary, Direction::ToSecondary] {
        let outcome = fixture.sync(FileKind::Config, direction);
        assert!(!outcome.changed && outcome.backup_path.is_none());
    }
    assert_eq!(
        fs::read_to_string(&fixture.pair.secondary.config).unwrap(),
        same
    );
    assert_eq!(
        fs::read_dir(fixture.pair.secondary.config.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn special_files_are_rejected_before_reading() {
    let fixture = Fixture::new();
    let socket =
        std::os::unix::net::UnixListener::bind(&fixture.pair.primary.instructions).unwrap();
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
    drop(socket);
    fs::remove_file(&fixture.pair.primary.instructions).unwrap();
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
    }
    let path =
        std::ffi::CString::new(fixture.pair.primary.instructions.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
}

#[test]
fn instructions_are_byte_exact_separate_from_config_and_override_in_both_directions() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.pair.primary.instructions,
        b"# Primary\r\nsynthetic instructions\r\n",
    )
    .unwrap();
    fs::write(&fixture.pair.secondary.instructions, b"# Secondary\n").unwrap();
    fs::write(
        &fixture.pair.primary.config,
        b"not parsed during instruction sync",
    )
    .unwrap();
    let override_path = fixture
        .pair
        .secondary
        .instructions
        .with_file_name("AGENTS.override.md");
    fs::write(&override_path, b"synthetic override stays").unwrap();
    assert_eq!(
        profile_paths(override_path.parent().unwrap())
            .unwrap()
            .instructions_override,
        Some(override_path.clone())
    );
    let outcome = fixture.sync(FileKind::Instructions, Direction::ToSecondary);
    assert_eq!(
        fs::read(outcome.backup_path.unwrap()).unwrap(),
        b"# Secondary\n"
    );
    assert_eq!(
        fs::read(&fixture.pair.secondary.instructions).unwrap(),
        b"# Primary\r\nsynthetic instructions\r\n"
    );
    assert!(
        !fixture
            .sync(FileKind::Instructions, Direction::ToPrimary)
            .changed
    );
    fs::write(
        &fixture.pair.secondary.instructions,
        b"# Changed secondary\n",
    )
    .unwrap();
    assert!(
        fixture
            .sync(FileKind::Instructions, Direction::ToPrimary)
            .changed
    );
    assert_eq!(
        fs::read(&fixture.pair.primary.instructions).unwrap(),
        b"# Changed secondary\n"
    );
    assert_eq!(
        fs::read(&fixture.pair.primary.config).unwrap(),
        b"not parsed during instruction sync"
    );
    assert_eq!(
        fs::read(override_path).unwrap(),
        b"synthetic override stays"
    );
}

#[test]
fn missing_source_never_erases_destination_and_oversize_is_bounded() {
    let fixture = Fixture::new();
    fs::write(&fixture.pair.secondary.instructions, b"keep").unwrap();
    assert_eq!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::NotFound
    );
    fs::write(
        &fixture.pair.primary.instructions,
        vec![b'x'; MAX_FILE_BYTES as usize + 1],
    )
    .unwrap();
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&fixture.pair.secondary.instructions).unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read_dir(fixture.pair.secondary.instructions.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn malformed_source_or_destination_never_leaks_secrets_or_changes_files() {
    let fixture = Fixture::new();
    for (source, target) in [
        ("token = 'synthetic-secret", "model = 'keep'"),
        ("model = 'keep'", "token = 'synthetic-secret"),
    ] {
        fs::write(&fixture.pair.primary.config, source).unwrap();
        fs::write(&fixture.pair.secondary.config, target).unwrap();
        let error = sync_file(&fixture.pair, FileKind::Config, Direction::ToSecondary).unwrap_err();
        assert!(!error.to_string().contains("synthetic-secret"));
        assert_eq!(
            fs::read_to_string(&fixture.pair.secondary.config).unwrap(),
            target
        );
    }
}

#[test]
fn identical_or_overlapping_homes_and_credential_filename_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.pair.secondary = fixture.pair.primary.clone();
    assert!(sync_file(&fixture.pair, FileKind::Config, Direction::ToPrimary).is_err());
    fixture.pair.secondary =
        profile_paths(&fixture.pair.primary.config.parent().unwrap().join("nested")).unwrap();
    assert!(sync_file(&fixture.pair, FileKind::Config, Direction::ToPrimary).is_err());
    fixture.pair.primary.config = fixture.pair.primary.config.with_file_name("auth.json");
    assert!(sync_file(&fixture.pair, FileKind::Config, Direction::ToPrimary).is_err());
}

#[cfg(unix)]
#[test]
fn symlinks_hardlinks_and_readonly_targets_are_rejected_without_overwrite() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    fs::write(&fixture.pair.primary.instructions, b"new").unwrap();
    let outside = fixture
        .pair
        .secondary
        .instructions
        .with_file_name("outside");
    fs::write(&outside, b"keep").unwrap();
    symlink(&outside, &fixture.pair.secondary.instructions).unwrap();
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
    fs::remove_file(&fixture.pair.secondary.instructions).unwrap();
    fs::hard_link(&outside, &fixture.pair.secondary.instructions).unwrap();
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
    fs::remove_file(&fixture.pair.secondary.instructions).unwrap();
    fs::write(&fixture.pair.secondary.instructions, b"keep").unwrap();
    fs::set_permissions(
        &fixture.pair.secondary.instructions,
        fs::Permissions::from_mode(0o400),
    )
    .unwrap();
    assert!(
        sync_file(
            &fixture.pair,
            FileKind::Instructions,
            Direction::ToSecondary
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&fixture.pair.secondary.instructions).unwrap(),
        b"keep"
    );
    assert_eq!(fs::read(outside).unwrap(), b"keep");
    fs::set_permissions(
        &fixture.pair.secondary.instructions,
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let outcome = fixture.sync(FileKind::Instructions, Direction::ToSecondary);
    assert_eq!(
        fs::metadata(&fixture.pair.secondary.instructions)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(outcome.backup_path.unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}

#[test]
fn a_change_before_atomic_publish_preserves_the_concurrent_edit() {
    let fixture = Fixture::new();
    let path = &fixture.pair.primary.instructions;
    fs::write(path, b"old").unwrap();
    let count = std::cell::Cell::new(0);
    let result = save_atomic(path, Some(b"old"), b"replacement", || {
        count.set(count.get() + 1);
        if count.get() == 2 {
            fs::write(path, b"concurrent edit")?;
        }
        if read_optional(path)?.as_deref() != Some(b"old".as_slice()) {
            return Err(io::Error::other("conflict"));
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(path).unwrap(), b"concurrent edit");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}
