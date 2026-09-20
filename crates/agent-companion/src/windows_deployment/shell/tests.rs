use super::*;

fn executable(payload: &[u8], subsystem: u16) -> Vec<u8> {
    let mut bytes = vec![0; 64 + 24 + 112];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[84..86].copy_from_slice(&112u16.to_le_bytes());
    bytes[86..88].copy_from_slice(&2u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes[156..158].copy_from_slice(&subsystem.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

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
    fs::write(&source, executable(b"companion executable", 2)).unwrap();
    let registered = register(&source, &env).unwrap();
    assert!(registered.in_current_path);
    assert_eq!(registered.directory, known);
    assert!(!unrelated.join("dodex.exe").exists());
    assert!(!env.stable_bin().exists());
    assert_eq!(
        fs::read(known.join("dodex.exe")).unwrap(),
        executable(b"companion executable", 3)
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        executable(b"companion executable", 2)
    );
    assert_eq!(
        ownership(&known).unwrap().unwrap().hashes,
        [file_hash(&known.join("dodex.exe")).unwrap()]
    );
    assert_ne!(
        file_hash(&source).unwrap(),
        file_hash(&known.join("dodex.exe")).unwrap()
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
    fs::write(&source, executable(b"version one", 2)).unwrap();
    let registered = register(&source, &env).unwrap();
    assert!(!registered.in_current_path);
    assert_eq!(registered.directory, env.stable_bin());
    assert!(registered.message().contains("旧终端不会自动更新 PATH"));
    let target = registered.directory.join("dodex.exe");
    fs::remove_file(&target).unwrap();
    register(&source, &env).unwrap();
    assert_eq!(fs::read(&target).unwrap(), executable(b"version one", 3));
    fs::write(&source, executable(b"version two", 2)).unwrap();
    register(&source, &env).unwrap();
    fs::remove_file(&source).unwrap();
    assert_eq!(fs::read(&target).unwrap(), executable(b"version two", 3));
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
    fs::write(&source, executable(b"old", 2)).unwrap();
    env.path = cargo.as_os_str().to_owned();
    // An older release installed its unmodified GUI executable. Its original
    // ownership hash authorizes migration to a console copy of the new build.
    fs::copy(&source, cargo.join("dodex.exe")).unwrap();
    save_ownership(&cargo, vec![file_hash(&source).unwrap()], false).unwrap();
    fs::write(&source, executable(b"new", 2)).unwrap();
    env.path = std::env::join_paths([&cargo, &local]).unwrap();
    assert_eq!(register(&source, &env).unwrap().directory, cargo);
    assert_eq!(
        fs::read(cargo.join("dodex.exe")).unwrap(),
        executable(b"new", 3)
    );
    assert_eq!(
        ownership(&cargo).unwrap().unwrap().hashes,
        [file_hash(&cargo.join("dodex.exe")).unwrap()]
    );
    assert!(!local.join("dodex.exe").exists());
}

#[test]
fn hardlinked_build_source_is_copied_to_an_independent_launcher() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let source = temp.path().join("build.exe");
    fs::write(&source, executable(b"companion build", 2)).unwrap();
    fs::hard_link(&source, temp.path().join("deps.exe")).unwrap();
    let registered = register(&source, &env).unwrap();
    let target = registered.directory.join("dodex.exe");
    no_redirects(&target).unwrap();
    fs::write(&target, b"changed independently").unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        executable(b"companion build", 2)
    );
}

#[test]
fn unrelated_and_modified_commands_are_never_replaced_or_shadowed() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = environment(temp.path());
    let source = temp.path().join("download.exe");
    fs::write(&source, executable(b"companion", 2)).unwrap();
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
    fs::write(&source, executable(b"companion", 2)).unwrap();
    fs::write(directory.join(MARKER), b"unrelated data").unwrap();
    assert!(register(&source, &env).is_err());
    assert_eq!(fs::read(directory.join(MARKER)).unwrap(), b"unrelated data");
    fs::remove_file(directory.join(MARKER)).unwrap();
    fs::hard_link(&source, directory.join("dodex.exe")).unwrap();
    assert!(register(&source, &env).is_err());
    assert_eq!(fs::read(&source).unwrap(), executable(b"companion", 2));
}

#[test]
fn console_conversion_rejects_invalid_headers_before_changing_registration() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let source = temp.path().join("download.exe");
    fs::write(&source, executable(b"original", 2)).unwrap();
    let registered = register(&source, &env).unwrap();
    let target = registered.directory.join("dodex.exe");
    let marker = fs::read(registered.directory.join(MARKER)).unwrap();
    let valid = executable(b"invalid", 2);
    let mut invalid = vec![Vec::new(), b"not a PE executable".to_vec()];
    for (start, bytes) in [
        (0, b"XX".as_slice()),
        (60, u32::MAX.to_le_bytes().as_slice()),
        (64, b"bad!".as_slice()),
        (84, 69u16.to_le_bytes().as_slice()),
        (86, 0x2002u16.to_le_bytes().as_slice()),
        (88, 0x107u16.to_le_bytes().as_slice()),
        (156, 1u16.to_le_bytes().as_slice()),
    ] {
        let mut bytes_to_test = valid.clone();
        bytes_to_test[start..start + bytes.len()].copy_from_slice(bytes);
        invalid.push(bytes_to_test);
    }
    for bytes in invalid {
        fs::write(&source, &bytes).unwrap();
        assert!(register(&source, &env).is_err());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(fs::read(&target).unwrap(), executable(b"original", 3));
        assert_eq!(fs::read(registered.directory.join(MARKER)).unwrap(), marker);
    }
}

#[test]
fn console_conversion_supports_pe32_and_clears_the_checksum() {
    let temp = tempfile::tempdir().unwrap();
    let env = environment(temp.path());
    let source = temp.path().join("download.exe");
    let mut bytes = executable(b"32-bit", 2);
    bytes[88..90].copy_from_slice(&0x10bu16.to_le_bytes());
    bytes[152..156].copy_from_slice(&1234u32.to_le_bytes());
    fs::write(&source, &bytes).unwrap();
    let registered = register(&source, &env).unwrap();
    bytes[152..158].copy_from_slice(&[0, 0, 0, 0, 3, 0]);
    assert_eq!(
        fs::read(registered.directory.join("dodex.exe")).unwrap(),
        bytes
    );
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
