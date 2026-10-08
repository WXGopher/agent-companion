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
