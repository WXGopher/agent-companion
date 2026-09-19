use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    layout: Layout,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // macOS /var is a symlink. Production paths reject redirected ancestors.
        let root = directory.path().canonicalize().unwrap();
        let mut layout = Layout::for_home(root.join("user"));
        layout.system_applications = root.join("Applications");
        private_directory(&layout.user_home).unwrap();
        private_directory(&layout.system_applications).unwrap();
        Self {
            _directory: directory,
            layout,
        }
    }
    fn source(&self) -> PathBuf {
        let app = self.layout.system_applications.join("Codex.app");
        fake_runtime(&app);
        app
    }
}
#[derive(Default)]
struct FakeOps {
    copied: AtomicUsize,
    verified: AtomicUsize,
    fail_copy: bool,
    fail_copied_signature: bool,
    legacy: bool,
}
impl Operations for FakeOps {
    fn verify_runtime(&self, app: &Path) -> Result<(), String> {
        self.verified.fetch_add(1, Ordering::SeqCst);
        if !app.join("synthetic-valid-signature").is_file()
            || (self.fail_copied_signature && app.file_name().unwrap() == "Runtime.app")
        {
            return Err("Synthetic signature rejected".into());
        }
        Ok(())
    }
    fn copy_runtime(&self, source: &Path, destination: &Path) -> Result<(), String> {
        self.copied.fetch_add(1, Ordering::SeqCst);
        if self.fail_copy {
            create_private(destination)?;
            write_new(
                &destination.join("partial"),
                b"synthetic partial copy",
                0o600,
            )?;
            return Err("Synthetic permission denied".into());
        }
        fn copy(source: &Path, destination: &Path) {
            fs::create_dir(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &destination.join(entry.file_name()));
                } else {
                    fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
                }
            }
        }
        copy(source, destination);
        Ok(())
    }
    fn legacy_fingerprints_match(&self, _: &Path, _: &Path) -> bool {
        self.legacy
    }
}
fn fake_runtime(app: &Path) {
    private_directory(&app.join("Contents/MacOS")).unwrap();
    private_directory(&app.join("Contents/Resources")).unwrap();
    write_new(&app.join("synthetic-valid-signature"), b"fixture", 0o600).unwrap();
    write_new(&app.join("Contents/MacOS/ChatGPT"), b"#!/bin/sh\n/usr/bin/env > \"$CODEX_HOME/synthetic-observed-env\"\nprintf '%s' \"$1\" > \"$CODEX_HOME/synthetic-observed-arg\"\n", 0o755).unwrap();
    write_new(
        &app.join("Contents/Resources/codex"),
        b"synthetic cli",
        0o755,
    )
    .unwrap();
}
fn deploy_fixture(fixture: &Fixture, ops: &dyn Operations) -> InstanceConfig {
    deploy_with(&fixture.layout, ops, |_, _| {}).unwrap()
}
fn child_names(path: &Path) -> Vec<String> {
    let mut children = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    children.sort();
    children
}

#[test]
fn default_is_disabled_without_discovering_or_creating_dodex() {
    let fixture = Fixture::new();
    let fake = FakeOps::default();
    private_directory(&fixture.layout.system_applications.join("Dodex.app")).unwrap();
    let state = read_saved_state(&fixture.layout);
    assert!(!state.status.enabled && !state.status.deployed && !state.status.busy);
    assert!(state.instance.is_none());
    assert!(!fixture.layout.support.exists());
    assert!(!fixture.layout.applications.exists());
    assert_eq!(fake.verified.load(Ordering::SeqCst), 0);
}
#[test]
fn fresh_deployment_is_private_separate_and_does_not_start_runtime() {
    let fixture = Fixture::new();
    fixture.source();
    let original = fixture.layout.user_home.join(".codex");
    private_directory(&original).unwrap();
    write_new(&original.join("auth.json"), b"synthetic-do-not-copy", 0o600).unwrap();
    write_new(
        &original.join("config.toml"),
        b"synthetic-do-not-copy",
        0o600,
    )
    .unwrap();
    let ops = FakeOps::default();
    let mut phases = Vec::new();
    let instance = deploy_with(&fixture.layout, &ops, |phase, _| {
        phases.push(phase.to_owned())
    })
    .unwrap();
    assert_eq!(phases, ["copying", "configuring", "verifying", "finishing"]);
    assert!(fixture.layout.root().join(MARKER).is_file());
    assert!(!instance.codex_home.join("auth.json").exists());
    assert!(launcher_text(&fixture.layout, &instance).contains(&format!(
        "--disk-cache-dir={}",
        instance.desktop_user_data.join("Cache").display()
    )));
    assert!(!instance.codex_home.join("synthetic-observed-env").exists());
    assert_eq!(child_names(&instance.codex_home), ["config.toml", "sqlite"]);
    assert_eq!(
        fs::metadata(instance.codex_home.join("config.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&instance.codex_home)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(ops.copied.load(Ordering::SeqCst), 1);
    assert_eq!(ops.verified.load(Ordering::SeqCst), 2);
    assert!(!fixture.layout.settings().exists()); // publishing alone does not enable monitoring
    validate_managed(&fixture.layout, &ops, &instance).unwrap();
}
#[test]
fn launcher_clears_all_inherited_auth_and_instance_overrides() {
    let fixture = Fixture::new();
    fixture.source();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    let result = Command::new(instance.launcher_app.join("Contents/MacOS/Dodex"))
        .env("OPENAI_API_KEY", "synthetic")
        .env("CHATGPT_ACCESS_TOKEN", "synthetic")
        .env("CODEX_THREAD_ID", "synthetic")
        .env("CODEX_HOME", "/synthetic-wrong-home")
        .env("CODEX_SQLITE_HOME", "/synthetic-wrong-database")
        .env("CODEX_APP_SERVER_URL", "synthetic")
        .env("ELECTRON_RUN_AS_NODE", "1")
        .env("NODE_OPTIONS", "synthetic")
        .arg("--user-data-dir=/synthetic-wrong-desktop")
        .status()
        .unwrap();
    assert!(result.success());
    let env = fs::read_to_string(instance.codex_home.join("synthetic-observed-env")).unwrap();
    for forbidden in [
        "synthetic-wrong",
        "OPENAI_API_KEY=",
        "CHATGPT_ACCESS_TOKEN=",
        "CODEX_THREAD_ID=",
        "CODEX_APP_SERVER_URL=",
        "ELECTRON_RUN_AS_NODE=",
        "NODE_OPTIONS=",
    ] {
        assert!(
            !env.contains(forbidden),
            "inherited override survived: {forbidden}"
        );
    }
    assert!(env.contains(&format!("CODEX_HOME={}\n", instance.codex_home.display())));
    assert!(env.contains(&format!(
        "CODEX_SQLITE_HOME={}\n",
        instance.database_dir.display()
    )));
    assert_eq!(
        fs::read_to_string(instance.codex_home.join("synthetic-observed-arg")).unwrap(),
        format!("--user-data-dir={}", instance.desktop_user_data.display())
    );
}
#[test]
fn repeat_deployment_adopts_without_overwriting_any_profile_files() {
    let fixture = Fixture::new();
    fixture.source();
    let ops = FakeOps::default();
    let instance = deploy_fixture(&fixture, &ops);
    let config = instance.codex_home.join("config.toml");
    let edited = format!(
        "{}\n# synthetic existing preferences\nmodel = \"synthetic-model\"\n",
        fs::read_to_string(&config).unwrap()
    );
    fs::write(&config, &edited).unwrap();
    write_new(
        &instance.codex_home.join("auth.json"),
        b"synthetic private fixture",
        0o600,
    )
    .unwrap();
    let before = fs::metadata(&config).unwrap().modified().unwrap();
    assert_eq!(deploy_fixture(&fixture, &ops), instance);
    assert_eq!(ops.copied.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read_to_string(&config).unwrap(), edited);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);
}
#[test]
fn missing_source_and_invalid_signature_create_no_environment() {
    let fixture = Fixture::new();
    let ops = FakeOps::default();
    assert!(
        deploy_with(&fixture.layout, &ops, |_, _| {})
            .unwrap_err()
            .contains("先将 Codex")
    );
    let source = fixture.source();
    fs::remove_file(source.join("synthetic-valid-signature")).unwrap();
    assert!(
        deploy_with(&fixture.layout, &ops, |_, _| {})
            .unwrap_err()
            .contains("signature")
    );
    assert!(!fixture.layout.root().exists());
    assert!(!fixture.layout.instance().launcher_app.exists());
    assert_eq!(ops.copied.load(Ordering::SeqCst), 0);
}
#[test]
fn copy_and_copied_signature_failures_clean_only_this_attempt() {
    for fail_copy in [true, false] {
        let fixture = Fixture::new();
        fixture.source();
        let old_stage = fixture
            .layout
            .support
            .join(".dodex-stage-interrupted-other-process");
        private_directory(&old_stage).unwrap();
        write_new(
            &old_stage.join("untouched"),
            b"synthetic unrelated staging",
            0o600,
        )
        .unwrap();
        let ops = FakeOps {
            fail_copy,
            fail_copied_signature: !fail_copy,
            ..FakeOps::default()
        };
        assert!(deploy_with(&fixture.layout, &ops, |_, _| {}).is_err());
        assert!(!fixture.layout.root().exists());
        assert!(!fixture.layout.instance().launcher_app.exists());
        assert_eq!(
            child_names(&fixture.layout.support),
            [".dodex-stage-interrupted-other-process", "deployment.lock"]
        );
        assert_eq!(
            child_names(&fixture.layout.applications),
            Vec::<String>::new()
        );
        assert_eq!(
            fs::read(old_stage.join("untouched")).unwrap(),
            b"synthetic unrelated staging"
        );
        deploy_fixture(&fixture, &FakeOps::default());
    }
}
#[test]
fn permission_error_is_reported_and_does_not_modify_existing_files() {
    let fixture = Fixture::new();
    fixture.source();
    // Deterministic filesystem denial: a file where an applications directory
    // would be, in addition to the injected EACCES path tested above.
    write_new(
        &fixture.layout.applications,
        b"synthetic preserved file",
        0o600,
    )
    .unwrap();
    assert!(deploy_with(&fixture.layout, &FakeOps::default(), |_, _| {}).is_err());
    assert_eq!(
        fs::read(&fixture.layout.applications).unwrap(),
        b"synthetic preserved file"
    );
    assert!(!fixture.layout.root().exists());
}
#[test]
fn interrupted_staging_is_ignored_and_after_publish_is_recoverable() {
    let fixture = Fixture::new();
    fixture.source();
    let ops = FakeOps::default();
    let instance = deploy_fixture(&fixture, &ops);
    // Simulate a process death after runtime publication, before launcher rename.
    fs::remove_file(fixture.layout.root().join(MARKER)).unwrap();
    fs::remove_dir_all(&instance.launcher_app).unwrap();
    assert!(validate_managed(&fixture.layout, &ops, &instance).is_err());
    assert_eq!(deploy_fixture(&fixture, &ops), instance);
    assert_eq!(ops.copied.load(Ordering::SeqCst), 1);
    validate_managed(&fixture.layout, &ops, &instance).unwrap();
    // Also recover death after launcher publication but before completion marker.
    fs::remove_file(fixture.layout.root().join(MARKER)).unwrap();
    let launcher_time = fs::metadata(&instance.launcher_app)
        .unwrap()
        .modified()
        .unwrap();
    deploy_fixture(&fixture, &ops);
    assert_eq!(
        fs::metadata(&instance.launcher_app)
            .unwrap()
            .modified()
            .unwrap(),
        launcher_time
    );
}
#[test]
fn foreign_or_tampered_environment_is_not_overwritten() {
    let fixture = Fixture::new();
    fixture.source();
    private_directory(&fixture.layout.root()).unwrap();
    write_new(
        &fixture.layout.root().join("foreign"),
        b"synthetic foreign",
        0o600,
    )
    .unwrap();
    assert!(deploy_with(&fixture.layout, &FakeOps::default(), |_, _| {}).is_err());
    assert_eq!(child_names(&fixture.layout.root()), ["foreign"]);
    fs::remove_dir_all(fixture.layout.root()).unwrap();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    fs::write(
        instance.launcher_app.join("Contents/MacOS/Dodex"),
        b"synthetic incompatible launcher",
    )
    .unwrap();
    assert!(deploy_with(&fixture.layout, &FakeOps::default(), |_, _| {}).is_err());
    assert_eq!(
        fs::read(instance.launcher_app.join("Contents/MacOS/Dodex")).unwrap(),
        b"synthetic incompatible launcher"
    );
}
#[test]
fn symlinked_data_and_auth_locations_are_rejected_without_reading_them() {
    let fixture = Fixture::new();
    fixture.source();
    let ops = FakeOps::default();
    let instance = deploy_fixture(&fixture, &ops);
    let external = fixture.layout.user_home.join("synthetic-external");
    write_new(&external, b"synthetic external", 0o600).unwrap();
    std::os::unix::fs::symlink(&external, instance.codex_home.join("auth.json")).unwrap();
    assert!(validate_existing(&fixture.layout, &ops, &instance).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"synthetic external");
}
#[test]
fn malformed_nested_and_inline_config_overrides_are_rejected() {
    let fixture = Fixture::new();
    fixture.source();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    let config = instance.codex_home.join("config.toml");
    for text in [
        "cli_auth_credentials_store = \"file\"\ninvalid = ",
        "cli_auth_credentials_store = \"file\"\n[profiles.work]\ncli_auth_credentials_store = \"keyring\"",
        "cli_auth_credentials_store = \"file\"\nprofiles = { work = { sqlite_home = '/synthetic-other' } }",
        "cli_auth_credentials_store = \"file\"\ncli_auth_credentials_store = \"file\"",
        "cli_auth_credentials_store = \"file\"\nlog_dir = '/synthetic-other'",
    ] {
        fs::write(&config, text).unwrap();
        assert!(validate_config(&config, &instance).is_err());
    }
    fs::write(&config, "'cli_auth_credentials_store' = 'file' # synthetic compatible\n[profiles.work]\ncli_auth_credentials_store = 'file'\n").unwrap();
    validate_config(&config, &instance).unwrap();
}
fn legacy_fixture() -> (Fixture, InstanceConfig) {
    let fixture = Fixture::new();
    let layout = &fixture.layout;
    let runtime = layout.system_applications.join("Codex B Runtime.app");
    fake_runtime(&runtime);
    let instance = InstanceConfig {
        id: "dodex".into(),
        label: "Dodex".into(),
        codex_home: layout.user_home.join(".codex-second"),
        desktop_user_data: layout.user_home.join("Library/Application Support/Codex-B"),
        database_dir: layout.user_home.join(".codex-second/sqlite"),
        launcher_app: layout.system_applications.join("Dodex.app"),
        cli_path: runtime.join("Contents/Resources/codex"),
        runtime_app: runtime,
    };
    private_directory(&instance.database_dir).unwrap();
    private_directory(&instance.desktop_user_data.join("tools")).unwrap();
    private_directory(&instance.launcher_app.join("Contents/MacOS")).unwrap();
    write_new(
        &instance.codex_home.join("config.toml"),
        config_text(&instance).as_bytes(),
        0o600,
    )
    .unwrap();
    let manager = instance.desktop_user_data.join("tools/codex_b_manager.py");
    write_new(
        &manager,
        format!("USER_ROOT = Path({})", toml_string(&layout.user_home)).as_bytes(),
        0o600,
    )
    .unwrap();
    write_new(
        &instance.launcher_app.join("Contents/MacOS/Dodex"),
        manager.to_string_lossy().as_bytes(),
        0o755,
    )
    .unwrap();
    (fixture, instance)
}
#[test]
fn exact_known_legacy_layout_can_be_adopted_read_only() {
    let (fixture, instance) = legacy_fixture();
    let layout = &fixture.layout;
    let config_before = fs::metadata(instance.codex_home.join("config.toml"))
        .unwrap()
        .modified()
        .unwrap();
    let ops = FakeOps {
        legacy: true,
        ..FakeOps::default()
    };
    assert_eq!(deploy_fixture(&fixture, &ops), instance);
    assert!(!layout.root().exists());
    assert!(!layout.applications.exists());
    assert_eq!(ops.copied.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::metadata(instance.codex_home.join("config.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        config_before
    );
    assert!(
        deploy_with(layout, &FakeOps::default(), |_, _| {})
            .unwrap_err()
            .contains("不兼容")
    );
}
#[test]
fn preferences_are_separate_and_changes_have_distinct_generations() {
    let fixture = Fixture::new();
    fixture.source();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    save_record(&fixture.layout, true, &instance).unwrap();
    let enabled = read_saved_state(&fixture.layout);
    assert!(enabled.status.deployed && enabled.status.busy);
    assert!(!enabled.status.enabled); // must await verification
    save_record(&fixture.layout, false, &instance).unwrap();
    let disabled = read_saved_state(&fixture.layout);
    assert!(disabled.status.deployed && !disabled.status.enabled && !disabled.status.busy);
    assert_ne!(enabled.preference_stamp, disabled.preference_stamp);
    assert!(instance.launcher_app.exists());
}
#[test]
fn parallel_deployment_is_rejected_and_kernel_lock_releases_on_drop() {
    use std::sync::{Arc, Barrier};
    struct BlockingOps {
        inner: FakeOps,
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }
    impl Operations for BlockingOps {
        fn verify_runtime(&self, app: &Path) -> Result<(), String> {
            self.inner.verify_runtime(app)
        }
        fn copy_runtime(&self, source: &Path, destination: &Path) -> Result<(), String> {
            self.entered.wait();
            self.release.wait();
            self.inner.copy_runtime(source, destination)
        }
        fn legacy_fingerprints_match(&self, _: &Path, _: &Path) -> bool {
            false
        }
    }
    let fixture = Fixture::new();
    fixture.source();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let layout = fixture.layout.clone();
    let enter_clone = entered.clone();
    let release_clone = release.clone();
    let worker = std::thread::spawn(move || {
        deploy_with(
            &layout,
            &BlockingOps {
                inner: FakeOps::default(),
                entered: enter_clone,
                release: release_clone,
            },
            |_, _| {},
        )
    });
    entered.wait();
    let error = deploy_with(&fixture.layout, &FakeOps::default(), |_, _| {}).unwrap_err();
    assert!(error.contains("另一进程"));
    release.wait();
    worker.join().unwrap().unwrap();
    deploy_fixture(&fixture, &FakeOps::default());
}
#[test]
fn exclusive_publication_never_replaces_an_existing_destination() {
    let fixture = Fixture::new();
    let source = fixture.layout.user_home.join("synthetic-source");
    let destination = fixture.layout.user_home.join("synthetic-destination");
    create_private(&source).unwrap();
    create_private(&destination).unwrap();
    write_new(&destination.join("kept"), b"synthetic existing", 0o600).unwrap();
    assert!(rename_exclusive(&source, &destination).is_err());
    assert_eq!(
        fs::read(destination.join("kept")).unwrap(),
        b"synthetic existing"
    );
}

#[test]
fn changed_preferences_discard_in_flight_validation() {
    let fixture = Fixture::new();
    fixture.source();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    save_record(&fixture.layout, true, &instance).unwrap();
    let mut state = read_saved_state(&fixture.layout);
    let request_stamp = state.preference_stamp;
    save_record(&fixture.layout, false, &instance).unwrap();
    assert!(!validation_is_current(
        &mut state,
        request_stamp,
        preference_stamp(&fixture.layout)
    ));
    assert!(!state.status.enabled && !state.status.busy && !state.initialized);
    let state = read_saved_state(&fixture.layout);
    assert!(!state.status.enabled && !state.status.busy);
    assert_eq!(state.status.phase, "disabled");
}
#[test]
fn completion_marker_publication_is_atomic_and_exclusive() {
    let fixture = Fixture::new();
    fixture.source();
    let instance = deploy_fixture(&fixture, &FakeOps::default());
    let marker = fixture.layout.root().join(MARKER);
    let completed = fs::read(&marker).unwrap();
    assert!(write_new_atomic(&marker, b"synthetic foreign replacement", 0o600).is_err());
    assert_eq!(fs::read(&marker).unwrap(), completed);
    // A process dying while writing the temporary marker leaves the final name
    // absent. Retry uses the manifest, ignoring its incomplete temporary artifact.
    fs::remove_file(&marker).unwrap();
    let abandoned = fixture.layout.root().join(".completion-stage-interrupted");
    write_new(&abandoned, b"{", 0o600).unwrap();
    let recovered = deploy_fixture(&fixture, &FakeOps::default());
    assert_eq!(instance, recovered);
    assert_eq!(fs::read(&marker).unwrap(), completed);
    assert_eq!(fs::read(&abandoned).unwrap(), b"{");
}
#[test]
fn redirected_database_credentials_and_primary_aliases_are_rejected() {
    let fixture = Fixture::new();
    fixture.source();
    let ops = FakeOps::default();
    let instance = deploy_fixture(&fixture, &ops);
    let external = fixture.layout.user_home.join("synthetic-external");
    write_new(&external, b"synthetic unrelated", 0o600).unwrap();
    let database = instance.database_dir.join("state_5.sqlite");
    std::os::unix::fs::symlink(&external, &database).unwrap();
    assert!(validate_existing(&fixture.layout, &ops, &instance).is_err());
    fs::remove_file(&database).unwrap();
    fs::hard_link(&external, &database).unwrap();
    assert!(validate_existing(&fixture.layout, &ops, &instance).is_err());
    fs::remove_file(&database).unwrap();
    fs::hard_link(&external, instance.codex_home.join("auth.json")).unwrap();
    assert!(validate_existing(&fixture.layout, &ops, &instance).is_err());
    fs::remove_file(instance.codex_home.join("auth.json")).unwrap();
    let primary = fixture.layout.user_home.join(".codex");
    std::os::unix::fs::symlink(&instance.codex_home, &primary).unwrap();
    assert!(
        validate_existing(&fixture.layout, &ops, &instance)
            .unwrap_err()
            .contains("重叠")
    );
    fs::remove_file(&primary).unwrap();
    private_directory(&primary).unwrap();
    write_new(
        &primary.join("config.toml"),
        format!("sqlite_home = {}\n", toml_string(&instance.database_dir)).as_bytes(),
        0o600,
    )
    .unwrap();
    assert!(
        validate_existing(&fixture.layout, &ops, &instance)
            .unwrap_err()
            .contains("重叠")
    );
    assert_eq!(fs::read(external).unwrap(), b"synthetic unrelated");
}
#[test]
fn real_filesystem_permission_denial_preserves_existing_content() {
    let fixture = Fixture::new();
    fixture.source();
    private_directory(&fixture.layout.support).unwrap();
    let kept = fixture.layout.support.join("synthetic-kept");
    write_new(&kept, b"synthetic untouched", 0o600).unwrap();
    fs::set_permissions(&fixture.layout.support, fs::Permissions::from_mode(0o500)).unwrap();
    let probe = fixture.layout.support.join("synthetic-permission-probe");
    if fs::write(&probe, b"test").is_ok() {
        // A root-run test suite bypasses UNIX directory permissions.
        fs::set_permissions(&fixture.layout.support, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let result = deploy_with(&fixture.layout, &FakeOps::default(), |_, _| {});
    fs::set_permissions(&fixture.layout.support, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(!fixture.layout.root().exists());
    assert_eq!(fs::read(kept).unwrap(), b"synthetic untouched");
}

#[test]
fn official_codesign_requirement_is_an_inline_expression() {
    // An Apple-signed system fixture must fail the OpenAI identity requirement,
    // rather than fail to parse it as if the expression were a file name.
    let result = Command::new("/usr/bin/codesign")
        .args([
            "--verify",
            "--strict",
            "-R",
            OFFICIAL_REQUIREMENT,
            "/usr/bin/true",
        ])
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("code failed to satisfy specified code requirement")
    );
}

#[test]
fn known_legacy_application_alias_is_adopted_without_changing_it() {
    for relative in [false, true] {
        let (fixture, instance) = legacy_fixture();
        let bundle = fixture.layout.system_applications.join("Codex B.app");
        fs::rename(&instance.launcher_app, &bundle).unwrap();
        let target = if relative {
            PathBuf::from("Codex B.app")
        } else {
            bundle.clone()
        };
        std::os::unix::fs::symlink(&target, &instance.launcher_app).unwrap();
        let executable = bundle.join("Contents/MacOS/Dodex");
        let before = fs::read(&executable).unwrap();
        let config = instance.codex_home.join("config.toml");
        let config_before = fs::read(&config).unwrap();
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        assert_eq!(deploy_fixture(&fixture, &ops), instance);
        validate_existing(&fixture.layout, &ops, &instance).unwrap();
        assert_eq!(fs::read_link(&instance.launcher_app).unwrap(), target);
        assert_eq!(fs::read(&executable).unwrap(), before);
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert_eq!(ops.copied.load(Ordering::SeqCst), 0);
        assert!(!fixture.layout.root().exists());
        assert!(
            validate_legacy(&fixture.layout, &FakeOps::default())
                .unwrap_err()
                .contains("不兼容")
        );
    }
}

#[test]
fn unknown_or_broken_legacy_application_aliases_remain_rejected() {
    let (fixture, instance) = legacy_fixture();
    let foreign = fixture.layout.system_applications.join("Unrecognized.app");
    fs::rename(&instance.launcher_app, &foreign).unwrap();
    std::os::unix::fs::symlink(&foreign, &instance.launcher_app).unwrap();
    let ops = FakeOps {
        legacy: true,
        ..FakeOps::default()
    };
    assert!(
        validate_legacy(&fixture.layout, &ops)
            .unwrap_err()
            .contains("启动别名")
    );
    fs::remove_file(&instance.launcher_app).unwrap();
    std::os::unix::fs::symlink(
        fixture.layout.system_applications.join("Codex B.app"),
        &instance.launcher_app,
    )
    .unwrap();
    assert!(
        validate_legacy(&fixture.layout, &ops)
            .unwrap_err()
            .contains("已失效")
    );
    assert_eq!(ops.verified.load(Ordering::SeqCst), 0);
    assert!(foreign.join("Contents/MacOS/Dodex").is_file());
    assert!(!fixture.layout.root().exists());
}

#[test]
fn runtime_feature_scan_supports_large_archives_and_split_markers() {
    let fixture = Fixture::new();
    let archive = fixture.layout.user_home.join("synthetic-app.asar");
    let mut file = File::create(&archive).unwrap();
    file.set_len(128 * 1024 * 1024 + 1).unwrap();
    for feature in RUNTIME_FEATURES {
        file.write_all(feature).unwrap();
        file.write_all(b"\n").unwrap();
    }
    assert!(runtime_supports_isolation(&archive).unwrap());

    struct ShortReads(std::io::Cursor<Vec<u8>>);
    impl Read for ShortReads {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let length = buffer.len().min(7);
            self.0.read(&mut buffer[..length])
        }
    }
    let features = RUNTIME_FEATURES.join(&b' ');
    assert!(read_runtime_features(ShortReads(std::io::Cursor::new(features))).unwrap());
    assert!(!read_runtime_features(std::io::Cursor::new(RUNTIME_FEATURES[0])).unwrap());
    let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "synthetic");
    struct FailedRead(Option<std::io::Error>);
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(self.0.take().unwrap())
        }
    }
    assert!(read_runtime_features(FailedRead(Some(error))).is_err());
}
