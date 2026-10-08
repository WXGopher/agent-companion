use super::*;

fn fixture(root: &Path) -> InstanceConfig {
    let instance = InstanceConfig::at(&root.join("用户 a & b's %tools%!/Dodex"));
    fs::create_dir_all(&instance.codex_home).unwrap();
    fs::create_dir_all(&instance.desktop_user_data).unwrap();
    fs::create_dir_all(instance.runtime_app.parent().unwrap()).unwrap();
    fs::write(&instance.runtime_app, b"desktop icon fixture").unwrap();
    instance
}

fn source(root: &Path, payload: &[u8]) -> PathBuf {
    let path = root.join("download.exe");
    // IShellLink::Save inspects executable targets; a header-only PE fixture
    // is rejected by Windows, so use a small valid native image.
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--crate-name", "dodex_shortcut_probe", "--edition", "2024"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/windows_deployment/fixtures/cli_probe.rs"
            ))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(payload);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn start_entry_uses_a_stable_gui_copy_and_repairs_without_changing_profile() {
    let _com = ComApartment::new().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let instance = fixture(temp.path());
    let source = source(temp.path(), b"version one");
    let shortcut = temp.path().join("Start 用户/Dodex.lnk");
    let root = instance.codex_home.parent().unwrap();
    let executable = root.join("dodex.exe");
    let config = instance.codex_home.join("config.toml");
    fs::write(&config, b"personal settings").unwrap();
    register(&source, &instance, &shortcut).unwrap();
    assert_eq!(
        launcher::kind(&executable).unwrap(),
        launcher::Kind::Desktop
    );
    assert_eq!(launcher::kind(&source).unwrap(), launcher::Kind::Console);
    let owner = launcher::ownership(root).unwrap().unwrap();
    assert_eq!(owner.schema, 2);
    assert_eq!(owner.entry_revision, 2);
    assert_eq!(owner.update_route, "companion");
    let link = load_shortcut(&shortcut).unwrap();
    assert!(launcher::same_path(
        &shortcut_target(&link).unwrap(),
        &executable
    ));
    let mut buffer = vec![0; 32768];
    unsafe { link.GetArguments(&mut buffer).unwrap() };
    assert!(text(&buffer).is_empty());
    unsafe { link.GetWorkingDirectory(&mut buffer).unwrap() };
    assert!(launcher::same_path(
        Path::new(&text(&buffer)),
        &instance.desktop_user_data
    ));
    let mut icon = 0;
    unsafe { link.GetIconLocation(&mut buffer, &mut icon).unwrap() };
    assert!(launcher::same_path(
        Path::new(&text(&buffer)),
        &instance.runtime_app
    ));
    drop(link);
    let before = fs::metadata(&executable).unwrap().modified().unwrap();
    // Legacy GUI entries upgrade their ownership record even when the running
    // launcher is already byte-identical and must not be replaced.
    let legacy = serde_json::json!({
        "schema": 1,
        "owner": launcher::OWNER,
        "hashes": [launcher::file_hash(&executable).unwrap()]
    });
    fs::write(
        root.join(launcher::MARKER),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    fs::remove_file(&source).unwrap();
    // The installed desktop launcher can recreate its missing shortcut itself.
    // Shell inspection can briefly retain a mapped view after drop(link), as
    // it can for the production atomic replacement. Wait for fixture cleanup.
    for attempt in 0..6 {
        match fs::remove_file(&shortcut) {
            Ok(()) => break,
            Err(error)
                if attempt < 5 && matches!(error.raw_os_error(), Some(5 | 32 | 33 | 1224)) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
            }
            Err(error) => panic!("cannot remove shortcut fixture: {error}"),
        }
    }
    register(&executable, &instance, &shortcut).unwrap();
    assert_eq!(launcher::ownership(root).unwrap().unwrap().schema, 2);
    assert_eq!(
        fs::metadata(&executable).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(fs::read(&config).unwrap(), b"personal settings");
    let updated = self::source(temp.path(), b"version two");
    register(&updated, &instance, &shortcut).unwrap();
    assert!(fs::read(&executable).unwrap().ends_with(b"version two"));
    assert_eq!(fs::read(&config).unwrap(), b"personal settings");
}

#[test]
fn start_entry_migrates_managed_cli_shortcuts_and_preserves_unrelated_ones() {
    let _com = ComApartment::new().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let instance = fixture(temp.path());
    let source = source(temp.path(), b"launcher");
    let shortcut = temp.path().join("Dodex.lnk");
    let cli_directory = temp.path().join("bin");
    fs::create_dir(&cli_directory).unwrap();
    launcher::install_command(&source, &cli_directory, launcher::Kind::Console).unwrap();
    let cli = cli_directory.join("dodex.exe");
    write_shortcut(&shortcut, &cli, &instance).unwrap();
    register(&source, &instance, &shortcut).unwrap();
    let link = load_shortcut(&shortcut).unwrap();
    assert!(launcher::same_path(
        &shortcut_target(&link).unwrap(),
        &instance.codex_home.parent().unwrap().join("dodex.exe")
    ));
    assert_eq!(launcher::kind(&cli).unwrap(), launcher::Kind::Console);
    drop(link);

    let unrelated = temp.path().join("unrelated.lnk");
    write_shortcut(&unrelated, &source, &instance).unwrap();
    let before = fs::read(&unrelated).unwrap();
    assert!(
        register(&source, &instance, &unrelated)
            .unwrap_err()
            .contains("未覆盖")
    );
    assert_eq!(fs::read(&unrelated).unwrap(), before);
    let invalid = temp.path().join("invalid.lnk");
    fs::write(&invalid, b"unrelated shortcut data").unwrap();
    assert!(register(&source, &instance, &invalid).is_err());
    assert_eq!(fs::read(&invalid).unwrap(), b"unrelated shortcut data");
}
