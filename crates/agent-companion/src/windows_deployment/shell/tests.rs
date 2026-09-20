use super::*;

fn environment(root: &Path) -> Environment {
    let home = root.join("用户 a & b's %tools%!");
    let local = home.join("AppData/Local");
    let roaming = home.join("AppData/Roaming");
    let current_directory = root.join("working");
    fs::create_dir_all(&current_directory).unwrap();
    Environment {
        variables: vec![
            ("USERPROFILE".into(), home.as_os_str().to_owned()),
            ("LOCALAPPDATA".into(), local.as_os_str().to_owned()),
            ("APPDATA".into(), roaming.as_os_str().to_owned()),
            ("tools".into(), "must-not-expand-recursively".into()),
        ],
        local,
        home,
        roaming: Some(roaming),
        current_directory,
        path: OsString::new(),
        extensions: ".COM;.EXE;.BAT;.CMD;.PY".into(),
    }
}

#[test]
fn registration_prefers_a_known_existing_path_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = environment(temp.path());
    let unrelated = temp.path().join("arbitrary-path");
    let known = env.home.join(".cargo/bin");
    for directory in [&unrelated, &known] {
        fs::create_dir_all(directory).unwrap();
    }
    env.path = std::env::join_paths([&unrelated, &known]).unwrap();
    let source = temp.path().join("agent-companion.exe");
    fs::write(&source, b"companion executable").unwrap();
    let registered = register(&source, &env).unwrap();
    assert!(registered.in_current_path);
    assert_eq!(registered.directory, known);
    assert!(!unrelated.join("dodex.exe").exists());
    assert!(!env.stable_bin().exists());
    assert_eq!(
        fs::read(known.join("dodex.exe")).unwrap(),
        b"companion executable"
    );
    let before = fs::metadata(known.join(MARKER))
        .unwrap()
        .modified()
        .unwrap();
    register(&source, &env).unwrap();
    assert_eq!(
        fs::metadata(known.join(MARKER))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    // Re-registering the running command itself is a no-copy operation.
    register(&known.join("dodex.exe"), &env).unwrap();
    assert_eq!(
        fs::metadata(known.join(MARKER))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
}

#[test]
fn missing_path_falls_back_to_stable_bin_and_repairs_a_missing_command() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let source = temp.path().join("download.exe");
    fs::write(&source, b"version one").unwrap();
    let registered = register(&source, &env).unwrap();
    assert!(!registered.in_current_path);
    assert_eq!(registered.directory, env.stable_bin());
    assert!(registered.message().contains("旧终端不会自动更新 PATH"));
    let target = registered.directory.join("dodex.exe");
    fs::remove_file(&target).unwrap();
    register(&source, &env).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"version one");
    fs::write(&source, b"version two").unwrap();
    register(&source, &env).unwrap();
    fs::remove_file(&source).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"version two");
    assert_eq!(
        ownership(&registered.directory).unwrap().unwrap().hashes,
        [file_hash(&target).unwrap()]
    );
}

#[test]
fn managed_command_first_on_path_is_upgraded_before_another_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = environment(temp.path());
    let cargo = env.home.join(".cargo/bin");
    let local = env.home.join(".local/bin");
    fs::create_dir_all(&cargo).unwrap();
    fs::create_dir_all(&local).unwrap();
    let source = temp.path().join("download.exe");
    fs::write(&source, b"old").unwrap();
    env.path = cargo.as_os_str().to_owned();
    register(&source, &env).unwrap();
    fs::write(&source, b"new").unwrap();
    env.path = std::env::join_paths([&cargo, &local]).unwrap();
    assert_eq!(register(&source, &env).unwrap().directory, cargo);
    assert_eq!(fs::read(cargo.join("dodex.exe")).unwrap(), b"new");
    assert!(!local.join("dodex.exe").exists());
}

#[test]
fn hardlinked_build_source_is_copied_to_an_independent_launcher() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let source = temp.path().join("build.exe");
    fs::write(&source, b"companion build").unwrap();
    fs::hard_link(&source, temp.path().join("deps.exe")).unwrap();
    let registered = register(&source, &env).unwrap();
    let target = registered.directory.join("dodex.exe");
    no_redirects(&target).unwrap();
    fs::write(&target, b"changed independently").unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"companion build");
}

#[test]
fn unrelated_and_modified_commands_are_never_replaced_or_shadowed() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = environment(temp.path());
    let source = temp.path().join("download.exe");
    fs::write(&source, b"companion").unwrap();
    let known = env.home.join(".cargo/bin");
    let other = temp.path().join("other");
    fs::create_dir_all(&known).unwrap();
    fs::create_dir_all(&other).unwrap();
    // Reject both earlier commands and commands a new installation would shadow.
    for paths in [[&other, &known], [&known, &other]] {
        env.path = std::env::join_paths(paths).unwrap();
        for name in ["dodex", "dodex.exe", "dodex.cmd", "dodex.ps1", "dodex.py"] {
            let conflict_path = other.join(name);
            fs::write(&conflict_path, b"unrelated command").unwrap();
            let error = register(&source, &env).err().unwrap();
            assert!(error.contains(&conflict_path.display().to_string()));
            assert_eq!(fs::read(&conflict_path).unwrap(), b"unrelated command");
            assert!(!known.join("dodex.exe").exists());
            fs::remove_file(conflict_path).unwrap();
        }
    }
    register(&source, &env).unwrap();
    fs::write(known.join("dodex.exe"), b"replaced externally").unwrap();
    assert!(register(&source, &env).is_err());
    assert_eq!(
        fs::read(known.join("dodex.exe")).unwrap(),
        b"replaced externally"
    );
}

#[test]
fn unrelated_marker_and_hardlinked_launcher_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let directory = env.stable_bin();
    fs::create_dir_all(&directory).unwrap();
    let source = temp.path().join("download.exe");
    fs::write(&source, b"companion").unwrap();
    fs::write(directory.join(MARKER), b"unrelated data").unwrap();
    assert!(register(&source, &env).is_err());
    assert_eq!(fs::read(directory.join(MARKER)).unwrap(), b"unrelated data");
    fs::remove_file(directory.join(MARKER)).unwrap();
    fs::hard_link(&source, directory.join("dodex.exe")).unwrap();
    assert!(register(&source, &env).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"companion");
}

#[test]
fn user_path_preserves_existing_entries_type_and_unexpanded_variables() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let directory = env.stable_bin();
    let current = UserPath {
        value: "%CUSTOM%;C:\\Other Dir;".into(),
        kind: REG_EXPAND_SZ,
    };
    let updated = append_user_path(&current, &directory, &env)
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.value,
        "%CUSTOM%;C:\\Other Dir;%LOCALAPPDATA%\\AgentCompanion\\bin"
    );
    assert_eq!(updated.kind, REG_EXPAND_SZ);
    assert!(
        append_user_path(&updated, &directory, &env)
            .unwrap()
            .is_none()
    );
    let plain = UserPath {
        value: "%LOCALAPPDATA%\\AgentCompanion\\bin".into(),
        kind: REG_SZ,
    };
    let plain_updated = append_user_path(&plain, &directory, &env).unwrap().unwrap();
    assert_eq!(plain_updated.kind, REG_SZ);
    assert!(
        plain_updated
            .value
            .to_string_lossy()
            .ends_with(directory.to_str().unwrap())
    );
    assert!(
        append_user_path(&plain_updated, &directory, &env)
            .unwrap()
            .is_none()
    );
    let case = UserPath {
        value: format!("\"{}\\\"", directory.display().to_string().to_uppercase()).into(),
        kind: REG_SZ,
    };
    assert!(append_user_path(&case, &directory, &env).unwrap().is_none());
    let empty = UserPath {
        value: OsString::new(),
        kind: REG_EXPAND_SZ,
    };
    assert_eq!(
        append_user_path(&empty, &directory, &env)
            .unwrap()
            .unwrap()
            .value,
        "%LOCALAPPDATA%\\AgentCompanion\\bin"
    );
}
