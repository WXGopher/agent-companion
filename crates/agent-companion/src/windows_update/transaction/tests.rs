use super::*;

struct Fixture {
    _temporary: TempDir,
    stage: TempDir,
    plan: Plan,
    originals: Vec<(String, Option<Vec<u8>>)>,
}

fn image(marker: &str) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes.extend_from_slice(marker.as_bytes());
    bytes
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("install with spaces 中文");
        fs::create_dir(&directory).unwrap();
        let stage = tempfile::Builder::new()
            .prefix(STAGE_PREFIX)
            .tempdir_in(&directory)
            .unwrap();
        fs::create_dir(stage.path().join("payload")).unwrap();
        fs::create_dir(stage.path().join("payload/docs")).unwrap();
        create_owned_tree(stage.path()).unwrap();
        let mut entries = Vec::new();
        let mut originals = Vec::new();
        for name in ["acomp.exe", "docs/RESUME.md", "agent-companion.exe"] {
            let old = if name.ends_with(".exe") {
                let bytes = image(&format!("old {name}"));
                fs::write(directory.join(name), &bytes).unwrap();
                Some(bytes)
            } else {
                None
            };
            let new = if name.ends_with(".exe") {
                image(&format!("new {name}"))
            } else {
                b"new documentation".to_vec()
            };
            let path = stage.path().join("payload").join(name);
            fs::write(&path, &new).unwrap();
            entries.push(PlannedEntry {
                new: Entry {
                    name: name.into(),
                    size: new.len() as u64,
                    sha256: super::super::hash_file(&path).unwrap(),
                },
                old: old.as_ref().map(|bytes| Fingerprint {
                    size: bytes.len() as u64,
                    sha256: super::super::hash_file(&directory.join(name)).unwrap(),
                }),
            });
            originals.push((name.into(), old));
        }
        fs::write(directory.join("personal-settings.json"), b"user settings").unwrap();
        let plan = Plan {
            schema: 1,
            executable: directory.join("agent-companion.exe"),
            parent_pid: 42,
            parent_created: 1,
            helper_sha256: entries.last().unwrap().old.as_ref().unwrap().sha256.clone(),
            tag: "v2.0.0".into(),
            version: "2.0.0".into(),
            nonce: "a".repeat(64),
            args: vec![],
            working_directory: directory,
            entries,
            result_path: None,
        };
        Self {
            _temporary: temporary,
            stage,
            plan,
            originals,
        }
    }

    fn directory(&self) -> &Path {
        self.plan.executable.parent().unwrap()
    }

    fn assert_originals(&self) {
        for (name, original) in &self.originals {
            match original {
                Some(bytes) => assert_eq!(
                    &fs::read(self.directory().join(name)).unwrap(),
                    bytes,
                    "{name}"
                ),
                None => assert!(
                    !self.directory().join(name).exists(),
                    "new entry survived rollback: {name}"
                ),
            }
        }
        assert_eq!(
            fs::read(self.directory().join("personal-settings.json")).unwrap(),
            b"user settings"
        );
    }
}

#[test]
fn apply_and_reverse_rollback_preserve_unrelated_files_and_remove_initially_missing_entries() {
    let fixture = Fixture::new();
    let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    fixture.assert_originals();
    transaction.apply().unwrap();
    for entry in &fixture.plan.entries {
        assert_eq!(
            super::super::hash_file(&fixture.directory().join(&entry.new.name)).unwrap(),
            entry.new.sha256
        );
    }
    assert!(transaction.changed());
    transaction.rollback().unwrap();
    assert!(transaction.restored());
    fixture.assert_originals();
    let journal = fs::read_to_string(fixture.stage.path().join("journal.jsonl")).unwrap();
    assert!(journal.contains("intent-retire"));
    assert!(journal.contains("intent-install"));
    assert!(journal.contains("intent-remove-new"));
    assert!(journal.contains("intent-restore"));
}

#[test]
fn competing_file_at_missing_destination_survives_failed_apply_and_rollback() {
    let fixture = Fixture::new();
    let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    fs::create_dir(fixture.directory().join("docs")).unwrap();
    fs::write(
        fixture.directory().join("docs/RESUME.md"),
        b"unrelated concurrent file",
    )
    .unwrap();
    assert!(transaction.apply().is_err());
    transaction.rollback().unwrap();
    assert_eq!(
        fs::read(fixture.directory().join("docs/RESUME.md")).unwrap(),
        b"unrelated concurrent file"
    );
    for (name, bytes) in fixture
        .originals
        .iter()
        .filter(|(_, bytes)| bytes.is_some())
    {
        assert_eq!(
            &fs::read(fixture.directory().join(name)).unwrap(),
            bytes.as_ref().unwrap()
        );
    }
}

#[test]
fn initialization_refuses_changed_payload_changed_install_and_cli_alias_directory() {
    let fixture = Fixture::new();
    fs::write(
        fixture.stage.path().join("payload/acomp.exe"),
        image("changed"),
    )
    .unwrap();
    assert!(Transaction::initialize(&fixture.plan, fixture.stage.path()).is_err());
    fixture.assert_originals();
    let fixture = Fixture::new();
    fs::write(
        fixture.directory().join("acomp.exe"),
        image("user modified"),
    )
    .unwrap();
    assert!(Transaction::initialize(&fixture.plan, fixture.stage.path()).is_err());
    let fixture = Fixture::new();
    fs::write(fixture.directory().join(".agent-companion-cli.json"), b"{}").unwrap();
    assert!(
        validate_install(&fixture.plan.executable)
            .unwrap_err()
            .to_string()
            .contains("CLI alias")
    );
    fixture.assert_originals();
}

#[test]
fn helper_initialization_rejects_hash_consistent_malformed_or_arm64_images() {
    for arm64 in [false, true] {
        let mut fixture = Fixture::new();
        let path = fixture.stage.path().join("payload/acomp.exe");
        let bytes = if arm64 {
            let mut bytes = image("wrong architecture");
            bytes[68..70].copy_from_slice(&0xaa64u16.to_le_bytes());
            bytes
        } else {
            b"hash-consistent invalid executable".to_vec()
        };
        fs::write(&path, &bytes).unwrap();
        fixture.plan.entries[0].new.size = bytes.len() as u64;
        fixture.plan.entries[0].new.sha256 = super::super::hash_file(&path).unwrap();
        assert!(Transaction::initialize(&fixture.plan, fixture.stage.path()).is_err());
        fixture.assert_originals();
        assert!(!fixture.stage.path().join("journal.jsonl").exists());
    }
}

#[test]
fn exclusive_install_lock_and_held_identities_prevent_concurrent_updates_and_writes() {
    let fixture = Fixture::new();
    let _transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    assert!(Transaction::initialize(&fixture.plan, fixture.stage.path()).is_err());
    assert!(fs::write(fixture.directory().join("acomp.exe"), b"changed").is_err());
    assert!(
        fs::rename(
            fixture.directory().join("acomp.exe"),
            fixture.directory().join("other.exe")
        )
        .is_err()
    );
    assert!(fs::write(fixture.stage.path().join("payload/acomp.exe"), b"changed").is_err());
    fixture.assert_originals();
}

#[test]
fn parent_timeout_leaves_all_package_paths_untouched() {
    let fixture = Fixture::new();
    let transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            std::process::id(),
        )
    }
    .unwrap();
    let process = unsafe { File::from_raw_handle(handle.0) };
    assert!(wait_parent(HANDLE(process.as_raw_handle()), Duration::from_millis(1)).is_err());
    assert!(!transaction.changed());
    assert!(!fixture.directory().join("docs").exists());
    fixture.assert_originals();
    let journal = fs::read_to_string(fixture.stage.path().join("journal.jsonl")).unwrap();
    assert!(!journal.contains("intent-retire"));
    assert!(!journal.contains("intent-install"));
}

fn system_executable(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"))
        .join("System32")
        .join(name)
}

fn start_probe(path: &Path) -> Child {
    Command::new(path)
        .args(["127.0.0.1", "-n", "30", "-w", "1000"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .unwrap()
}

#[test]
fn native_running_sibling_image_can_be_renamed_and_rolled_back_without_stopping_it() {
    let mut fixture = Fixture::new();
    let executable = fixture.directory().join("acomp.exe");
    fs::copy(system_executable("ping.exe"), &executable).unwrap();
    let bytes = fs::read(&executable).unwrap();
    fixture.originals[0].1 = Some(bytes.clone());
    fixture.plan.entries[0].old = Some(Fingerprint {
        size: bytes.len() as u64,
        sha256: super::super::hash_file(&executable).unwrap(),
    });
    let mut child = start_probe(&executable);
    let result = (|| {
        let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path())?;
        transaction.apply()?;
        assert!(
            child.try_wait()?.is_none(),
            "the updater must not stop a running sibling"
        );
        transaction.rollback()?;
        assert!(
            child.try_wait()?.is_none(),
            "rollback must not stop a running sibling"
        );
        fixture.assert_originals();
        Ok::<_, io::Error>(())
    })();
    let _ = stop_owned(&mut child); // This test owns the isolated ping probe.
    result.unwrap();
}

#[test]
fn external_non_delete_sharing_lock_fails_preflight_without_package_mutation() {
    let fixture = Fixture::new();
    let _locked = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(fixture.directory().join("acomp.exe"))
        .unwrap();
    assert!(Transaction::initialize(&fixture.plan, fixture.stage.path()).is_err());
    fixture.assert_originals();
}

#[test]
fn docs_junction_appearing_after_initialization_is_rejected_before_any_package_rename() {
    let fixture = Fixture::new();
    let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    let outside = fixture._temporary.path().join("outside-documents");
    fs::create_dir(&outside).unwrap();
    fs::write(
        outside.join("RESUME.md"),
        b"unrelated outside documentation",
    )
    .unwrap();
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = Command::new(powershell).args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:AC_UPDATE_TEST_LINK -Target $env:AC_UPDATE_TEST_TARGET | Out-Null"])
        .env("AC_UPDATE_TEST_LINK", fixture.directory().join("docs")).env("AC_UPDATE_TEST_TARGET", &outside)
        .stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW.0).output().unwrap();
    assert!(
        output.status.success(),
        "junction fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(transaction.apply().is_err());
    assert!(!transaction.changed());
    transaction.rollback().unwrap();
    assert_eq!(
        fs::read(outside.join("RESUME.md")).unwrap(),
        b"unrelated outside documentation"
    );
    fs::remove_dir(fixture.directory().join("docs")).unwrap();
    fixture.assert_originals();
}

#[test]
fn startup_timeout_and_post_start_commit_error_stop_only_owned_child_before_rollback() {
    let fixture = Fixture::new();
    let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    transaction.apply().unwrap();
    let mut child = Some(start_probe(&system_executable("ping.exe")));
    assert!(
        wait_startup(
            child.as_mut().unwrap(),
            &fixture.plan,
            fixture.stage.path(),
            Duration::from_millis(1)
        )
        .is_err()
    );
    rollback_failed(&mut transaction, &mut child).unwrap();
    assert!(child.as_mut().unwrap().try_wait().unwrap().is_some());
    fixture.assert_originals();

    let fixture = Fixture::new();
    let mut transaction = Transaction::initialize(&fixture.plan, fixture.stage.path()).unwrap();
    transaction.apply().unwrap();
    let mut child = Some(start_probe(&system_executable("ping.exe")));
    write_json_new(
        &fixture.stage.path().join("startup-ack.json"),
        &StartupAck {
            schema: 1,
            nonce: fixture.plan.nonce.clone(),
            pid: child.as_ref().unwrap().id(),
            version: fixture.plan.version.clone(),
        },
    )
    .unwrap();
    wait_startup(
        child.as_mut().unwrap(),
        &fixture.plan,
        fixture.stage.path(),
        Duration::from_secs(1),
    )
    .unwrap();
    // Substitute a real read-only handle to inject the durable-commit write
    // failure after startup succeeded; this is the same outer failure path.
    let journal_path = fixture.stage.path().join("journal.jsonl");
    drop(std::mem::replace(
        &mut transaction.journal,
        File::open(&journal_path).unwrap(),
    ));
    assert!(transaction.journal("committed", None).is_err());
    rollback_failed(&mut transaction, &mut child).unwrap();
    assert!(child.as_mut().unwrap().try_wait().unwrap().is_some());
    fixture.assert_originals();
}

#[test]
fn startup_ack_must_match_the_exact_child_nonce_and_selected_version() {
    for field in ["pid", "nonce", "version"] {
        let fixture = Fixture::new();
        let mut child = start_probe(&system_executable("ping.exe"));
        let ack = StartupAck {
            schema: 1,
            nonce: if field == "nonce" {
                "wrong".into()
            } else {
                fixture.plan.nonce.clone()
            },
            pid: if field == "pid" {
                child.id() + 1
            } else {
                child.id()
            },
            version: if field == "version" {
                "0.0.0".into()
            } else {
                fixture.plan.version.clone()
            },
        };
        write_json_new(&fixture.stage.path().join("startup-ack.json"), &ack).unwrap();
        let result = wait_startup(
            &mut child,
            &fixture.plan,
            fixture.stage.path(),
            Duration::from_secs(1),
        );
        let _ = stop_owned(&mut child);
        assert!(result.is_err(), "accepted mismatched {field}");
    }
}

#[test]
fn malformed_plan_and_redirected_or_relative_paths_fail_closed() {
    let fixture = Fixture::new();
    assert!(
        valid_plan(&fixture.plan, fixture.stage.path()).is_err(),
        "incomplete fixture manifest must fail helper validation"
    );
    assert!(run_helper(Path::new("relative/plan.json")).is_err());
    assert!(run_helper(&fixture.stage.path().join("wrong.json")).is_err());
    let path = fixture.stage.path().join("plan.json");
    fs::write(
        &path,
        br#"{"schema":1,"executable":"C:\\Windows\\System32\\cmd.exe","unknown":true}"#,
    )
    .unwrap();
    assert!(run_helper(&path).is_err());
    fixture.assert_originals();
    let result_path = fixture.stage.path().join("result.json");
    fs::write(&result_path, br#"{"message":"persisted failure"}"#).unwrap();
    assert_eq!(
        read_result(&result_path).as_deref(),
        Some("persisted failure")
    );
}
