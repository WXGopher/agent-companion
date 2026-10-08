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
