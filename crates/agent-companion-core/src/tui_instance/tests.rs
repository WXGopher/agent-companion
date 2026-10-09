use super::*;

fn fixture() -> (tempfile::TempDir, Layout, Registry) {
    let temporary = tempfile::tempdir().unwrap();
    let user_home = temporary.path().canonicalize().unwrap().join("user");
    fs::create_dir_all(&user_home).unwrap();
    let layout = Layout {
        support: user_home.join("support"),
        user_home,
    };
    let home = layout.support.join("secondary");
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("logs"));
    let registry = Registry {
        schema: SCHEMA,
        primary: None,
        dodex: instance,
        dodex_enabled: false,
    };
    (temporary, layout, registry)
}

#[test]
fn records_do_not_depend_on_desktop_paths_or_monitoring() {
    let (_temporary, layout, registry) = fixture();
    layout.validate(&registry).unwrap();
    fs::create_dir_all(&layout.support).unwrap();
    fs::write(layout.record(), serde_json::to_vec(&registry).unwrap()).unwrap();
    assert_eq!(layout.read().unwrap(), Some(registry));
    let text = fs::read_to_string(layout.record()).unwrap();
    assert!(!text.contains("desktop") && !text.contains("runtime_app"));
}

#[test]
fn each_instance_has_its_own_maintenance_lock() {
    let (_temporary, layout, _) = fixture();
    let _codex = InstanceLock::acquire(&layout, "codex").unwrap();
    let dodex = InstanceLock::acquire(&layout, "dodex").unwrap();
    assert!(InstanceLock::acquire(&layout, "codex").is_err());
    assert!(InstanceLock::acquire(&layout, "dodex").is_err());
    drop(dodex);
    assert!(InstanceLock::acquire(&layout, "dodex").is_ok());
}

#[test]
fn overlapping_or_redirected_storage_is_rejected_without_rewriting_it() {
    let (_temporary, layout, mut registry) = fixture();
    registry.dodex.database_dir = layout.user_home.join(".codex/sqlite");
    assert!(layout.validate(&registry).is_err());
    registry.dodex.database_dir = registry.dodex.codex_home.join("sqlite");
    registry.dodex.cli_path = layout
        .user_home
        .join("Applications/Dodex.app/Contents/Resources/codex");
    assert!(layout.validate(&registry).is_err());
}

#[test]
fn redirected_local_appdata_keeps_owned_legacy_storage_without_accepting_other_roots() {
    let (temporary, mut layout, mut registry) = fixture();
    // Windows can place LOCALAPPDATA on another drive outside USERPROFILE.
    layout.support = temporary.path().canonicalize().unwrap().join("local-data");
    let home = layout.support.join("Dodex/codex-home");
    registry.dodex = layout.secondary(home.clone(), home.join("sqlite"), home.join("logs"));
    layout.validate(&registry).unwrap();
    registry.dodex.database_dir = temporary.path().canonicalize().unwrap().join("foreign");
    assert!(layout.validate(&registry).is_err());
    registry.dodex.database_dir = layout.user_home.join(".codex/sqlite");
    assert!(layout.validate(&registry).is_err());
}

fn launch_fixture() -> (tempfile::TempDir, InstanceConfig) {
    let (temporary, _layout, registry) = fixture();
    let mut instance = registry.dodex;
    let package = instance
        .codex_home
        .join("packages/standalone/releases/fixture");
    for directory in ["bin", "codex-resources", "codex-path"] {
        fs::create_dir_all(package.join(directory)).unwrap();
    }
    let native = package.join("bin").join(executable_name());
    fs::write(&native, b"MZ\0\0fixture, never executed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        package.join("bin").join(if cfg!(windows) {
            "codex-code-mode-host.exe"
        } else {
            "codex-code-mode-host"
        }),
        "fixture",
    )
    .unwrap();
    fs::write(
        package
            .join("codex-path")
            .join(if cfg!(windows) { "rg.exe" } else { "rg" }),
        "fixture",
    )
    .unwrap();
    fs::write(
        package.join("codex-package.json"),
        serde_json::to_vec(&serde_json::json!({
            "layoutVersion": 1, "version": "fixture",
            "entrypoint": format!("bin/{}", executable_name()),
            "resourcesDir": "codex-resources", "pathDir": "codex-path",
        }))
        .unwrap(),
    )
    .unwrap();
    // Test command construction with the resolved release entry; no native
    // execution or platform-specific symlink privilege is required.
    instance.cli_path = native;
    let mut config = toml_edit::DocumentMut::new();
    config["cli_auth_credentials_store"] = toml_edit::value("file");
    config["sqlite_home"] = toml_edit::value(instance.database_dir.to_string_lossy().into_owned());
    config["log_dir"] = toml_edit::value(instance.log_dir.to_string_lossy().into_owned());
    fs::write(instance.codex_home.join("config.toml"), config.to_string()).unwrap();
    (temporary, instance)
}

#[test]
fn named_profiles_cannot_override_secondary_storage_or_credentials() {
    let (_temporary, instance) = launch_fixture();
    for (key, value) in [
        (
            "sqlite_home",
            instance
                .codex_home
                .join("other-sqlite")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "log_dir",
            instance
                .codex_home
                .join("other-log")
                .to_string_lossy()
                .into_owned(),
        ),
        ("cli_auth_credentials_store", "keyring".to_owned()),
    ] {
        let mut profile = toml_edit::DocumentMut::new();
        profile[key] = toml_edit::value(value);
        fs::write(
            instance.codex_home.join("work.config.toml"),
            profile.to_string(),
        )
        .unwrap();
        for arguments in [
            vec!["--profile", "work", "resume", "fixture"],
            vec!["exec", "-pwork", "fixture"],
        ] {
            let arguments: Vec<_> = arguments.into_iter().map(OsString::from).collect();
            let error = instance.command(&arguments).unwrap_err();
            assert!(error.to_string().contains(key), "{error}");
        }
    }
}

#[test]
fn safe_missing_and_help_profiles_keep_native_arguments() {
    let (_temporary, instance) = launch_fixture();
    fs::write(
        instance.codex_home.join("work.config.toml"),
        "model='fixture'\n[mcp_servers.fixture.env]\nsqlite_home='service-value'\n",
    )
    .unwrap();
    fs::write(
        instance.codex_home.join("unsafe.config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    for arguments in [
        vec!["--profile=work", "resume", "fixture"],
        vec!["--profile", "missing", "resume", "fixture"],
        vec!["--profile", "unsafe", "--help"],
        vec!["--profile", "unsafe", "help", "resume"],
        vec!["exec", "--", "-punsafe"],
    ] {
        let arguments: Vec<_> = arguments.into_iter().map(OsString::from).collect();
        let command = instance.command(&arguments).unwrap();
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_owned())
                .collect::<Vec<_>>(),
            arguments
        );
    }
}

fn configure_project_trust(
    instance: &InstanceConfig,
    entries: &[(&Path, &str)],
    markers: Option<&[&str]>,
) {
    let path = instance.codex_home.join("config.toml");
    let mut config = read_configuration(&path).unwrap().unwrap();
    let mut projects = toml_edit::Table::new();
    for (directory, trust) in entries {
        let mut project = toml_edit::Table::new();
        project["trust_level"] = toml_edit::value(*trust);
        projects[&directory.to_string_lossy()] = toml_edit::Item::Table(project);
    }
    config["projects"] = toml_edit::Item::Table(projects);
    if let Some(markers) = markers {
        let mut values = toml_edit::Array::new();
        for marker in markers {
            values.push(*marker);
        }
        config["project_root_markers"] = toml_edit::value(values);
    }
    fs::write(path, config.to_string()).unwrap();
}

fn check_project(
    instance: &InstanceConfig,
    directory: &Path,
    arguments: &[OsString],
) -> io::Result<()> {
    let paths = crate::install::profile_sync::IsolationPaths {
        sqlite_home: instance.database_dir.clone(),
        log_dir: instance.log_dir.clone(),
    };
    let profiles = instance.validate_named_profiles(arguments, &paths)?;
    if let Some(directory) = crate::codex_args::project_directory(arguments, directory) {
        instance.validate_project_layers(arguments, &directory, &profiles, None, None, &paths)?;
    }
    Ok(())
}

#[test]
fn trusted_project_layers_cannot_change_bound_paths_or_credential_store() {
    let (temporary, instance) = launch_fixture();
    let project = temporary.path().canonicalize().unwrap().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::create_dir_all(project.join(".codex")).unwrap();
    configure_project_trust(&instance, &[(&project, "trusted")], None);
    let config = project.join(".codex/config.toml");
    for (key, value) in [
        (
            "sqlite_home",
            project
                .join("foreign-sqlite")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "log_dir",
            project.join("foreign-log").to_string_lossy().into_owned(),
        ),
        ("cli_auth_credentials_store", "keyring".to_owned()),
    ] {
        let mut layer = toml_edit::DocumentMut::new();
        layer[key] = toml_edit::value(value);
        let bytes = layer.to_string();
        fs::write(&config, &bytes).unwrap();
        for arguments in [
            vec![
                OsString::from("-C"),
                project.as_os_str().to_owned(),
                OsString::from("exec"),
                OsString::from("fixture"),
            ],
            vec![
                OsString::from("exec"),
                OsString::from(format!("--cd={}", project.display())),
                OsString::from("fixture"),
            ],
        ] {
            let error = check_project(&instance, temporary.path(), &arguments).unwrap_err();
            assert!(error.to_string().contains(key), "{error}");
        }
        assert_eq!(fs::read_to_string(&config).unwrap(), bytes);
    }
}

#[test]
fn project_validation_preserves_safe_policies_and_ignores_disabled_or_unrelated_layers() {
    let (temporary, instance) = launch_fixture();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    let nested = project.join("nested");
    for directory in [
        root.join(".codex"),
        project.join(".git"),
        project.join(".codex"),
        nested.join(".codex"),
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        root.join(".codex/config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    let safe = "approval_policy='never'\nsandbox_mode='read-only'\n[mcp_servers.fixture]\ncommand='fixture-tool'\n[mcp_servers.fixture.env]\nsqlite_home='service-value'\n[profiles.legacy]\nsqlite_home='ignored-project-key'\n";
    fs::write(project.join(".codex/config.toml"), safe).unwrap();
    fs::write(
        nested.join(".codex/config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    configure_project_trust(
        &instance,
        &[
            (&root, "trusted"),
            (&project, "trusted"),
            (&nested, "untrusted"),
        ],
        None,
    );
    check_project(&instance, &nested, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(project.join(".codex/config.toml")).unwrap(),
        safe
    );
    configure_project_trust(&instance, &[(&project, "untrusted")], None);
    check_project(&instance, &nested, &[]).unwrap();
    configure_project_trust(&instance, &[(&project, "trusted")], None);
    assert!(check_project(&instance, &nested, &[]).is_err());
    for arguments in [
        vec!["--help"],
        vec!["--version"],
        vec!["update"],
        vec!["exec", "--ignore-user-config", "fixture"],
    ] {
        let arguments: Vec<_> = arguments.into_iter().map(OsString::from).collect();
        check_project(&instance, &nested, &arguments).unwrap();
    }
}

#[test]
fn selected_profile_cli_and_custom_markers_keep_native_project_precedence() {
    let (temporary, instance) = launch_fixture();
    let project = temporary.path().canonicalize().unwrap().join("project");
    let nested = project.join("nested");
    fs::create_dir_all(project.join(".codex")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(project.join(".project"), "fixture").unwrap();
    fs::write(
        project.join(".codex/config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    configure_project_trust(&instance, &[(&project, "trusted")], Some(&[]));
    check_project(&instance, &nested, &[]).unwrap();
    let profile = format!(
        "project_root_markers=['.project']\n[projects.{}]\ntrust_level='trusted'\n",
        toml_edit::Value::from(project.to_string_lossy().into_owned())
    );
    fs::write(instance.codex_home.join("work.config.toml"), profile).unwrap();
    let mut arguments = vec![OsString::from("--profile"), OsString::from("work")];
    assert!(check_project(&instance, &nested, &arguments).is_err());
    arguments.extend([
        OsString::from("-c"),
        OsString::from(format!(
            "projects.{}.trust_level='untrusted'",
            toml_edit::Value::from(project.to_string_lossy().into_owned())
        )),
    ]);
    check_project(&instance, &nested, &arguments).unwrap();
}

#[test]
fn higher_project_layer_can_restore_the_bound_isolation_fields() {
    let (temporary, instance) = launch_fixture();
    let project = temporary.path().canonicalize().unwrap().join("project");
    let nested = project.join("nested");
    for directory in [
        project.join(".git"),
        project.join(".codex"),
        nested.join(".codex"),
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        project.join(".codex/config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    fs::write(
        nested.join(".codex/config.toml"),
        "cli_auth_credentials_store='file'\n",
    )
    .unwrap();
    configure_project_trust(&instance, &[(&project, "trusted")], None);
    check_project(&instance, &nested, &[]).unwrap();
}

#[test]
fn managed_trust_and_policies_retain_native_precedence_without_mutation() {
    let (temporary, instance) = launch_fixture();
    let project = temporary.path().canonicalize().unwrap().join("project");
    fs::create_dir_all(project.join(".codex")).unwrap();
    fs::write(project.join(".project"), "fixture").unwrap();
    let project_config = project.join(".codex/config.toml");
    let project_bytes = "cli_auth_credentials_store='keyring'\n";
    fs::write(&project_config, project_bytes).unwrap();
    configure_project_trust(&instance, &[(&project, "trusted")], None);
    let system: toml_edit::DocumentMut =
        "project_root_markers=['.project']\n[mcp_servers.policy]\ncommand='policy-tool'\n"
            .parse()
            .unwrap();
    let managed_text = format!(
        "approval_policy='on-request'\nsandbox_mode='read-only'\n[projects.{}]\ntrust_level='untrusted'\n",
        toml_edit::Value::from(project.to_string_lossy().into_owned())
    );
    let mut managed: toml_edit::DocumentMut = managed_text.parse().unwrap();
    let system_before = system.to_string();
    let paths = crate::install::profile_sync::IsolationPaths {
        sqlite_home: instance.database_dir.clone(),
        log_dir: instance.log_dir.clone(),
    };
    instance
        .validate_project_layers(&[], &project, &[], Some(&system), Some(&managed), &paths)
        .unwrap();
    assert_eq!(managed.to_string(), managed_text);
    assert_eq!(system.to_string(), system_before);
    assert_eq!(fs::read_to_string(&project_config).unwrap(), project_bytes);

    managed["projects"][project.to_string_lossy().as_ref()]["trust_level"] =
        toml_edit::value("trusted");
    assert!(
        instance
            .validate_project_layers(&[], &project, &[], Some(&system), Some(&managed), &paths)
            .is_err()
    );
    managed["cli_auth_credentials_store"] = toml_edit::value("file");
    instance
        .validate_project_layers(&[], &project, &[], Some(&system), Some(&managed), &paths)
        .unwrap();
}

#[cfg(windows)]
#[test]
fn project_trust_keys_preserve_native_spelling_and_lookup_precedence() {
    let (temporary, instance) = launch_fixture();
    let canonical = temporary.path().canonicalize().unwrap().join("project");
    let project = PathBuf::from(canonical.to_string_lossy().trim_start_matches(r"\\?\"));
    fs::create_dir_all(project.join(".codex")).unwrap();
    fs::write(
        project.join(".codex/config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    let forward = PathBuf::from(project.to_string_lossy().replace('\\', "/"));
    for key in [&forward, &canonical] {
        configure_project_trust(&instance, &[(key, "trusted")], None);
        check_project(&instance, &project, &[]).unwrap();
    }
    let uppercase = PathBuf::from(project.to_string_lossy().to_uppercase());
    configure_project_trust(&instance, &[(&uppercase, "trusted")], None);
    assert!(check_project(&instance, &project, &[]).is_err());
    configure_project_trust(
        &instance,
        &[(&project, "untrusted"), (&canonical, "trusted")],
        None,
    );
    check_project(&instance, &canonical, &[]).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_verbatim_paths_and_case_cannot_escape_instance_containment() {
    assert!(path_within(
        Path::new(r"\\?\C:\Users\Example\.codex\packages\bin\codex.exe"),
        Path::new(r"c:\users\example\.codex")
    ));
    assert!(!path_within(
        Path::new(r"C:\Users\Example\.codex-secondary\bin\codex.exe"),
        Path::new(r"c:\users\example\.codex")
    ));
    let temporary = tempfile::tempdir().unwrap();
    let verbatim = temporary.path().canonicalize().unwrap();
    no_redirects(&verbatim).unwrap();
    let regular = PathBuf::from(verbatim.to_string_lossy().trim_start_matches(r"\\?\"));
    no_redirects(&regular).unwrap();
    let (_temporary, mut layout, mut registry) = fixture();
    layout.user_home = PathBuf::from(
        layout
            .user_home
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    );
    layout.support = layout.user_home.join("support");
    registry.dodex.command_path = layout.public_bin().join("dodex.exe");
    layout.validate(&registry).unwrap();
    registry.dodex.database_dir = layout.user_home.join(".codex/sqlite");
    assert!(layout.validate(&registry).is_err());
}
