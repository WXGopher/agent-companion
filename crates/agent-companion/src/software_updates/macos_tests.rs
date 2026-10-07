use super::*;
use std::os::unix::fs::symlink;

#[test]
fn fresh_terminal_entry_does_not_require_a_legacy_adapter() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().canonicalize().unwrap();
    let entry = home.join(".local/bin/dodex");
    assert!(!legacy_entry(&home, &entry).unwrap());
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(&entry, b"unrelated command").unwrap();
    assert!(legacy_entry(&home, &entry).is_err());
    fs::remove_file(&entry).unwrap();
    symlink(home.join("missing"), &entry).unwrap();
    assert!(legacy_entry(&home, &entry).is_err());
}

#[test]
fn release_parsing_rejects_prerelease_and_filters_delta_wrong_arch_and_os() {
    assert_eq!(
        parse_cli_release(br#"{"tag_name":"rust-v0.160.0","draft":false,"prerelease":false}"#)
            .unwrap()
            .version
            .version,
        "0.160.0"
    );
    assert!(
        parse_cli_release(
            br#"{"tag_name":"rust-v0.161.0-alpha.1","draft":false,"prerelease":false}"#
        )
        .is_err()
    );
    let feed = br#"<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>
      <item><sparkle:version>100</sparkle:version><sparkle:shortVersionString>26.9.1</sparkle:shortVersionString><sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
       <enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-26.9.1.zip" length="500"/>
       <sparkle:deltas><enclosure sparkle:version="999" url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-delta.zip" length="10"/></sparkle:deltas>
      </item>
      <item><sparkle:version>200</sparkle:version><sparkle:shortVersionString>26.9.2</sparkle:shortVersionString>
       <enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-x86_64-26.9.2.zip" length="500"/></item>
      <item><sparkle:version>300</sparkle:version><sparkle:shortVersionString>26.9.3</sparkle:shortVersionString><sparkle:minimumSystemVersion>99.0</sparkle:minimumSystemVersion>
       <enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-26.9.3.zip" length="500"/></item>
      <item><sparkle:version>400</sparkle:version><sparkle:shortVersionString>26.9.4</sparkle:shortVersionString><sparkle:channel>beta</sparkle:channel>
       <enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-26.9.4.zip" length="500"/></item>
    </channel></rss>"#;
    let release = parse_appcast(feed, "aarch64", "15.0").unwrap();
    assert_eq!(release.version.build.as_deref(), Some("100"));
    assert_eq!(release.length, 500);
    for url in [
        "http://persistent.oaistatic.com/codex-app-prod/app.zip",
        "https://persistent.oaistatic.com.evil/codex-app-prod/app.zip",
        "https://persistent.oaistatic.com/codex-app-prod/../else/app.zip",
    ] {
        assert!(!official_app_url(url));
    }
}

#[test]
fn pathless_cli_and_every_bundle_child_block_replacement() {
    let paths = vec![
        PathBuf::from("/Applications/ChatGPT.app"),
        PathBuf::from("/user/packages/release"),
    ];
    for process in [
        "codex\n",
        "dodex\n",
        "codex-code-mode-host\n",
        "/Applications/ChatGPT.app/Contents/Frameworks/Helper\n",
        "/user/packages/release/bin/codex-code-mode-host\n",
    ] {
        assert!(process_matches(process.as_bytes(), &paths), "{process}");
    }
    assert!(!process_matches(
        b"/Applications/Other.app/Contents/MacOS/Other\n",
        &paths
    ));
}

#[test]
fn secondary_replacement_does_not_block_the_primary_or_an_unaffected_app() {
    let app = PathBuf::from("/user/Applications/Dodex.app");
    let package =
        PathBuf::from("/user/Library/Application Support/AgentCompanion/Tui/packages/current");
    let source = b"/Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n/opt/homebrew/Caskroom/codex/0.160.1/bin/codex\ncodex\n";
    assert!(!secondary_process_matches(
        source,
        &[app.clone(), package.clone()]
    ));
    let running_app = b"/user/Applications/Dodex.app/Contents/Frameworks/Helper\n";
    assert!(!secondary_process_matches(
        running_app,
        std::slice::from_ref(&package)
    ));
    assert!(secondary_process_matches(running_app, &[app]));
    assert!(secondary_process_matches(
        format!("{}/bin/codex-code-mode-host\n", package.display()).as_bytes(),
        &[package]
    ));
}

#[test]
fn complete_package_metadata_rejects_resources_linked_to_another_installation() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().canonicalize().unwrap();
    for directory in ["bin", "codex-path", "codex-resources"] {
        fs::create_dir(package.join(directory)).unwrap();
    }
    for file in ["bin/codex", "bin/codex-code-mode-host", "codex-path/rg"] {
        fs::write(package.join(file), b"fixture").unwrap();
    }
    fs::write(package.join("codex-package.json"), br#"{"layoutVersion":1,"version":"0.160.1","entrypoint":"bin/codex","resourcesDir":"codex-resources","pathDir":"codex-path"}"#).unwrap();
    assert_eq!(managed_tui::package_version(&package).unwrap(), "0.160.1");
    fs::rename(package.join("codex-resources"), package.join("elsewhere")).unwrap();
    symlink(package.join("elsewhere"), package.join("codex-resources")).unwrap();
    assert!(managed_tui::package_version(&package).is_err());
}

#[test]
fn failed_app_publication_restores_original_and_backup_retry_is_safe() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let app = root.join("Codex.app");
    fs::create_dir(&app).unwrap();
    fs::write(app.join("original"), b"retained").unwrap();
    let backup = root.join("backup.app");
    assert!(publish_with_backup(&root.join("missing.app"), &app, &backup).is_err());
    assert_eq!(fs::read(app.join("original")).unwrap(), b"retained");
    assert!(!backup.exists());
    let entry = root.join("dodex");
    let saved = root.join("saved-dodex");
    fs::write(&entry, b"original adapter").unwrap();
    preserve_original(&entry, &saved).unwrap();
    preserve_original(&entry, &saved).unwrap();
    fs::write(&entry, b"changed behind the updater").unwrap();
    assert!(preserve_original(&entry, &saved).is_err());
    assert_eq!(fs::read(&saved).unwrap(), b"original adapter");
}

#[test]
fn redirect_paths_and_concurrent_editors_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    symlink(&root, root.join("redirect")).unwrap();
    assert!(no_redirects(&root.join("redirect/file")).is_err());
    let first = acquire_lock(&root.join("update.lock")).unwrap();
    assert!(acquire_lock(&root.join("update.lock")).is_err());
    drop(first);
    assert!(acquire_lock(&root.join("update.lock")).is_ok());
}

fn wrapper_fixture(root: &Path) -> Binding {
    let package = root.join("complete package");
    fs::create_dir_all(package.join("bin")).unwrap();
    let binding = Binding {
        package: package.clone(),
        entry: root.join("dodex"),
        app: root.join("Public Dodex.app"),
        profile_home: root.join("existing-second"),
        sqlite_home: root.join("existing-second/sqlite"),
        desktop_data: root.join("existing desktop"),
        log_dir: root.join("existing desktop/logs"),
        original: root.join("original adapter"),
        companion: PathBuf::from("/usr/bin/true"),
        version: "0.160.0".into(),
    };
    fs::create_dir_all(&binding.sqlite_home).unwrap();
    fs::create_dir_all(binding.profile_home.join("sessions")).unwrap();
    fs::write(
        binding.profile_home.join("sessions/synthetic.jsonl"),
        b"synthetic conversation before update",
    )
    .unwrap();
    fs::write(
        binding.sqlite_home.join("synthetic-state"),
        b"same account index",
    )
    .unwrap();
    let probe = br#"#!/usr/bin/python3 -B
import json, os, pathlib, sys
home = pathlib.Path(os.environ['CODEX_HOME'])
print(json.dumps({'argv':sys.argv[1:],'env':dict(os.environ),'cwd':os.getcwd(),'pid':os.getpid(),
  'session':(home/'sessions/synthetic.jsonl').read_text(), 'index':(pathlib.Path(os.environ['CODEX_SQLITE_HOME'])/'synthetic-state').read_text()}))
sys.exit(int(os.environ.get('TEST_NATIVE_EXIT','0')))
"#;
    fs::write(package.join("bin/codex"), probe).unwrap();
    fs::set_permissions(package.join("bin/codex"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&binding.entry, render_wrapper(&binding).unwrap()).unwrap();
    fs::set_permissions(&binding.entry, fs::Permissions::from_mode(0o755)).unwrap();
    binding
}

#[test]
fn wrapper_preserves_existing_session_paths_native_resume_arguments_pid_and_exit() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let binding = wrapper_fixture(&root);
    let child = Command::new(&binding.entry)
        .args(["resume", "synthetic-thread-id", "--", "prompt with spaces"])
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &root)
        .env("CODEX_HOME", root.join("primary-must-not-be-read"))
        .env("CODEX_SQLITE_HOME", root.join("wrong-index"))
        .env("CODEX_THREAD_ID", "wrong-session")
        .env("OPENAI_API_KEY", "synthetic-other-account")
        .env("NODE_OPTIONS", "unsafe-other-instance")
        .env("CODEX_SANDBOX", "seatbelt")
        .env("CODEX_NETWORK_PROXY_ACTIVE", "1")
        .env("TEST_NATIVE_EXIT", "37")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(37));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["pid"], pid);
    assert_eq!(value["cwd"], root.to_string_lossy().as_ref());
    assert_eq!(value["session"], "synthetic conversation before update");
    assert_eq!(value["index"], "same account index");
    assert_eq!(
        value["env"]["CODEX_HOME"],
        binding.profile_home.to_string_lossy().as_ref()
    );
    assert_eq!(
        value["env"]["CODEX_SQLITE_HOME"],
        binding.sqlite_home.to_string_lossy().as_ref()
    );
    assert_eq!(value["env"]["CODEX_SANDBOX"], "seatbelt");
    assert_eq!(value["env"]["CODEX_NETWORK_PROXY_ACTIVE"], "1");
    for key in ["CODEX_THREAD_ID", "OPENAI_API_KEY", "NODE_OPTIONS"] {
        assert!(value["env"].get(key).is_none());
    }
    let args = value["argv"].as_array().unwrap();
    assert_eq!(
        &args[6..],
        serde_json::json!(["resume", "synthetic-thread-id", "--", "prompt with spaces"])
            .as_array()
            .unwrap()
    );
    assert!(
        args[3]
            .as_str()
            .unwrap()
            .contains(binding.sqlite_home.to_str().unwrap())
    );
    assert!(
        args[5]
            .as_str()
            .unwrap()
            .contains(binding.log_dir.to_str().unwrap())
    );
    assert_eq!(
        fs::read(binding.profile_home.join("sessions/synthetic.jsonl")).unwrap(),
        b"synthetic conversation before update"
    );
}

#[test]
fn wrapper_routing_blocks_isolation_overrides_and_keeps_prompt_words_native() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let binding = wrapper_fixture(&root);
    let script = r#"import json, runpy, sys
module = runpy.run_path(sys.argv[1],run_name='test')
route = module['route']
assert route(['--','update'])[0] == 'native'
assert route(['resume','abc','app'])[0] == 'native'
assert route(['--image','picture','app'])[0] == 'native'
assert route(['-C','path with spaces','app','project'])[0] == 'app'
assert route(['--config','model="app"','update'])[0] == 'update'
for args in [['--config=sqlite_home="/bad"'],['-clog_dir="/bad"'],['-c','cli_auth_credentials_store="keyring"']]:
 try: module['native_args'](args)
 except ValueError: pass
 else: raise AssertionError(args)
assert not any(k.startswith('DYLD_') for k in module['environment']({'DYLD_LIBRARY_PATH':'bad'}))
captured = []
class Done(Exception): pass
def capture(*args): captured.append(args); raise Done()
module['os'].execve = capture
try: module['main'](['update'])
except Done: pass
assert captured[0][1][-2:] == ['--software-action','update-all']
assert 'CODEX_HOME' not in captured[0][2] and 'CODEX_SQLITE_HOME' not in captured[0][2]
"#;
    let output = Command::new("/usr/bin/python3")
        .args(["-B", "-c", script])
        .arg(&binding.entry)
        .env("CODEX_HOME", "/synthetic/second")
        .env("CODEX_SQLITE_HOME", "/synthetic/second/sqlite")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_uses_current_public_app_and_requests_native_single_instance_forwarding() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let binding = wrapper_fixture(&root);
    fs::write(
        &binding.original,
        br#"def route(args): return 'app', 0, []
def app_request(args, prefix, cwd): return False, cwd
def open_workspace(*args): raise AssertionError('must never launch hidden runtime')
"#,
    )
    .unwrap();
    let script = r#"import runpy, sys
module = runpy.run_path(sys.argv[1],run_name='test')
captured = []
class Done(Exception): pass
def capture(*args): captured.append(args); raise Done()
module['os'].execve = capture
try: module['main'](['app','.'])
except Done: pass
assert captured[0][0] == '/usr/bin/open'
assert captured[0][1] == ['/usr/bin/open','-n','-a',module['BINDING']['app'],'--args','--open-project',module['os'].getcwd()]
assert 'CODEX_HOME' not in captured[0][2]
"#;
    let output = Command::new("/usr/bin/python3")
        .args(["-B", "-c", script])
        .arg(&binding.entry)
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn app_publication_resumes_every_crash_boundary_and_rejects_foreign_installations() {
    for phase in 0..3 {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let pending = AppPublication {
            schema: 1,
            destination: root.join("Codex.app"),
            staged: root.join("staged.app"),
            backup: root.join("backup.app"),
            original: Version {
                version: "old".into(),
                build: Some("1".into()),
            },
            target: Version {
                version: "new".into(),
                build: Some("2".into()),
            },
        };
        let write = |path: &Path, version: &str| {
            fs::create_dir(path).unwrap();
            fs::write(path.join("version"), version).unwrap();
        };
        write(&pending.destination, "old");
        write(&pending.staged, "new");
        if phase >= 1 {
            fs::rename(&pending.destination, &pending.backup).unwrap();
        }
        if phase >= 2 {
            fs::rename(&pending.staged, &pending.destination).unwrap();
        }
        let verify = |path: &Path, expected: &Version| {
            if fs::read_to_string(path.join("version")).ok().as_deref()
                == Some(expected.version.as_str())
            {
                Ok(())
            } else {
                Err("changed bundle".into())
            }
        };
        resume_app_publication(&pending, verify).unwrap();
        resume_app_publication(&pending, verify).unwrap();
        assert_eq!(
            fs::read_to_string(pending.destination.join("version")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(pending.backup.join("version")).unwrap(),
            "old"
        );
        fs::write(pending.destination.join("version"), "foreign").unwrap();
        assert!(resume_app_publication(&pending, verify).is_err());
        assert_eq!(
            fs::read_to_string(pending.destination.join("version")).unwrap(),
            "foreign"
        );
        assert_eq!(
            fs::read_to_string(pending.backup.join("version")).unwrap(),
            "old"
        );
    }
}
