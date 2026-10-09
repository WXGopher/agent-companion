use super::*;
use std::{fs, sync::OnceLock};

#[tokio::test]
async fn quota_protocol_is_handshake_and_exactly_one_business_call() {
    for kind in [QueryKind::Limits, QueryKind::History] {
        let (client, server) = tokio::io::duplex(16384);
        let (input, output) = tokio::io::split(client);
        let mut rpc = Rpc::new(input, output, Duration::from_secs(2));
        let fixture = async {
            let (input, mut output) = tokio::io::split(server);
            let mut lines = BufReader::new(input).lines();
            let initialized: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(initialized["method"], "initialize");
            assert_eq!(
                initialized["params"]["capabilities"]["experimentalApi"],
                true
            );
            output
                .write_all(b"{\"id\":1,\"result\":{}}\n")
                .await
                .unwrap();
            let notification: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(notification, json!({"method":"initialized"}));
            let config: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(
                config,
                json!({"id":2,"method":"config/read","params":{"includeLayers":false}})
            );
            output
                .write_all(b"{\"id\":2,\"result\":{\"config\":{}}}\n")
                .await
                .unwrap();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(
                request,
                json!({"id":3,"method":if kind == QueryKind::Limits { "account/rateLimits/read" } else { "account/usage/read" }})
            );
            output
                .write_all(
                    b"{\"method\":\"notification\",\"params\":{}}\n{\"id\":100,\"result\":{}}\n",
                )
                .await
                .unwrap();
            let value = match kind {
                QueryKind::Limits => json!({"rateLimits":{}}),
                QueryKind::History => json!({"summary":{}}),
            };
            output
                .write_all(format!("{}\n", json!({"id":3,"result":value})).as_bytes())
                .await
                .unwrap();
            // If the reader sends a preflight, another quota call, token call,
            // thread, or turn, this assertion sees it before EOF.
            assert!(lines.next_line().await.unwrap().is_none());
        };
        let client = async {
            let result = exchange(&mut rpc, kind).await.unwrap().unwrap();
            drop(rpc);
            result
        };
        let (value, ()) = tokio::join!(client, fixture);
        assert!(value.is_object());
    }
}

#[test]
fn malformed_payloads_are_failures_and_missing_windows_are_successes() {
    for value in [
        json!({}),
        json!({"rateLimits":false}),
        json!({"rateLimits":{"primary":{"usedPercent":"20"}}}),
        json!({"rateLimits":{"secondary":{"resetsAt":10}}}),
        json!({"rateLimits":{},"rateLimitsByLimitId":{"codex":{"primary":false}}}),
    ] {
        assert!(validate_result(value, QueryKind::Limits).is_err());
    }
    for value in [
        json!({"rateLimits":{}}),
        json!({"rateLimits":null}),
        json!({"rateLimitsByLimitId":{"codex":{}}}),
        json!({"rateLimits":{"primary":null,"secondary":null}}),
    ] {
        assert!(validate_result(value, QueryKind::Limits).unwrap()["rateLimits"].is_object());
    }
    for value in [
        json!({}),
        json!({"summary":null}),
        json!({"summary":{"lifetimeTokens":"12"}}),
        json!({"summary":{},"dailyUsageBuckets":[{"tokens":1}]}),
    ] {
        assert!(validate_result(value, QueryKind::History).is_err());
    }
    assert!(validate_result(json!({"summary":{}}), QueryKind::History).is_ok());
    let failure = decode(
        &json!({"id":2,"error":{"code":-32601,"message":"secret-token"}}),
        QueryKind::Limits,
    )
    .unwrap_err();
    assert!(failure.starts_with("Update Codex CLI"));
    assert!(!failure.contains("secret"));
}

#[tokio::test]
async fn transport_enforces_deadlines_and_response_byte_limits() {
    let (client, _server) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(client);
    let mut rpc = Rpc::new(input, output, Duration::from_millis(15));
    assert_eq!(
        exchange(&mut rpc, QueryKind::Limits)
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    let bytes = vec![b'x'; MAX_RESPONSE_BYTES + 1];
    let mut rpc = Rpc::new(bytes.as_slice(), tokio::io::sink(), Duration::from_secs(1));
    assert!(
        rpc.receive(1)
            .await
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
    let mut rpc = Rpc::new(
        b"not json\n".as_slice(),
        tokio::io::sink(),
        Duration::from_secs(1),
    );
    assert!(rpc.receive(1).await.is_err());
}

fn executable() -> PathBuf {
    static EXECUTABLE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    EXECUTABLE
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("usage_server.rs");
            fs::write(&source, include_str!("fixture.rs")).unwrap();
            let executable = directory.path().join(if cfg!(windows) {
                "usage-server.exe"
            } else {
                "usage-server"
            });
            let output = Command::new("rustc")
                .args(["--edition=2024"])
                .arg(source)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            // A fresh executable may spend seconds in macOS launch validation.
            // Warm only this disposable fixture before request deadlines start.
            let warm = directory.path().join("warmup");
            fs::create_dir(&warm).unwrap();
            let status = Command::new(&executable)
                .env("CODEX_HOME", &warm)
                .env("CODEX_SQLITE_HOME", &warm)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                status.success(),
                "could not warm up the synthetic native fixture"
            );
            (directory, executable)
        })
        .1
        .clone()
}

fn source(root: &Path, id: &str) -> Source {
    let home = root.join(id);
    fs::create_dir_all(&home).unwrap();
    write_identity(&home, false);
    Source {
        instance_id: id.into(),
        codex_home: home.clone(),
        executable_path: Some(executable()),
        database_path: home.join("database with spaces"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn discovery_accepts_existing_native_tui_without_an_updater_package_manifest() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let native = root.join("Caskroom/codex/fixture/codex-aarch64-apple-darwin");
    fs::create_dir_all(native.parent().unwrap()).unwrap();
    fs::write(&native, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
    let entry = root.join("codex");
    symlink(&native, &entry).unwrap();
    assert_eq!(
        primary_macos_executable(
            &root,
            [entry],
            &root.join("Applications"),
            &root.join("user/Applications"),
        ),
        Some(native),
        "usage discovery must not require an updater-owned package layout"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn discovery_rejects_unknown_wrappers_and_does_not_use_an_app_bundled_cli() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let entry = root.join("codex");
    fs::write(
        &entry,
        "#!/bin/sh\nCODEX_HOME=/another-account exec another-codex \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).unwrap();
    let system = root.join("Applications");
    let user = root.join("user/Applications");
    assert_eq!(
        primary_macos_executable(&root, [entry.clone()], &system, &user),
        None
    );
    let app = system.join("ChatGPT.app");
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    fs::write(app.join("Contents/Info.plist"), "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>com.openai.codex</string></dict></plist>").unwrap();
    for relative in ["Contents/MacOS/ChatGPT", "Contents/Resources/codex"] {
        let path = app.join(relative);
        fs::write(&path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(
        primary_macos_executable(&root, [entry], &system, &user),
        None,
    );
}

#[cfg(target_os = "macos")]
#[test]
fn discovery_resolves_official_npm_layouts_without_running_javascript() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let (platform, triple) = if cfg!(target_arch = "aarch64") {
        ("codex-darwin-arm64", "aarch64-apple-darwin")
    } else {
        ("codex-darwin-x64", "x86_64-apple-darwin")
    };
    for (layout, relative) in [
        ("nested", "bin/codex"),
        ("hoisted", "bin/codex"),
        ("bundled", "bin/codex"),
        ("nested", "codex/codex"),
        ("hoisted", "codex/codex"),
        ("bundled", "codex/codex"),
    ] {
        let prefix = root.join(layout).join(relative.split('/').next().unwrap());
        let package = prefix.join("lib/node_modules/@openai/codex");
        fs::create_dir_all(package.join("bin")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name":"@openai/codex","bin":{"codex":"bin/codex.js"}}"#,
        )
        .unwrap();
        let script = package.join("bin/codex.js");
        fs::write(
            &script,
            "#!/usr/bin/env node\nthrow Error('must never execute');\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let platform_package = match layout {
            "nested" => package.join("node_modules/@openai").join(platform),
            "hoisted" => package.parent().unwrap().join(platform),
            _ => package.clone(),
        };
        let native = platform_package.join("vendor").join(triple).join(relative);
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        if layout != "bundled" {
            fs::write(
                platform_package.join("package.json"),
                r#"{"name":"@openai/codex"}"#,
            )
            .unwrap();
        }
        fs::write(&native, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
        let entry = prefix.join("codex");
        symlink(&script, &entry).unwrap();
        assert_eq!(
            primary_macos_executable(
                &root,
                [entry.clone()],
                &root.join("Applications"),
                &root.join("user/Applications")
            ),
            Some(native),
            "failed to resolve the official {layout} npm layout"
        );
        fs::write(
            package.join("package.json"),
            r#"{"name":"unrelated-package","bin":{"codex":"bin/codex.js"}}"#,
        )
        .unwrap();
        assert_eq!(
            primary_macos_executable(
                &root,
                [entry],
                &root.join("Applications"),
                &root.join("user/Applications")
            ),
            None
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn discovery_does_not_borrow_a_secondary_instances_native_runtime() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let linked_home = root.join("home-alias");
    symlink(&root, &linked_home).unwrap();
    for relative in [
        "Applications/Dodex.app/Contents/Resources/codex",
        "Applications/.Dodex/runtime/bin/codex",
        "Library/Application Support/AgentCompanion/Tui/packages/fixture/bin/codex",
        "Library/Application Support/AgentCompanion/Dodex/Runtime.app/Contents/Resources/codex",
        "Library/Application Support/Codex-B/runtime/bin/codex",
    ] {
        let native = root.join(relative);
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&native, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
        // Also exercise the updater-compatible native branch.
        fs::write(
            native
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("codex-package.json"),
            "{}",
        )
        .unwrap();
        let entry = root.join("codex");
        symlink(&native, &entry).unwrap();
        assert_eq!(
            primary_macos_executable(
                &linked_home,
                [entry.clone()],
                &root.join("PrimaryApplications"),
                &root.join("user/Applications")
            ),
            None
        );
        fs::remove_file(entry).unwrap();
    }
}

fn write_identity(home: &Path, second: bool) {
    // Synthetic JWTs only. Token material deliberately is not an identity.
    let payload = if second {
        "eyJzdWIiOiJ1c2VyLWIiLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoid29ya3NwYWNlIn19"
    } else {
        "eyJzdWIiOiJ1c2VyLWEiLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoid29ya3NwYWNlIn19"
    };
    fs::write(home.join("auth.json"), json!({"fixture_account": if second {"b"} else {"a"}, "tokens": {
        "id_token": format!("header.{payload}.signature"),
        "access_token": "synthetic-access", "refresh_token": "synthetic-refresh", "account_id": "workspace"
    }}).to_string()).unwrap();
}

#[test]
fn strict_worker_supports_the_legacy_native_feature_set() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "dodex");
    fs::write(source.codex_home.join("mode"), "legacy-strict").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    service.sync_sources(vec![source], agent_companion_core::now_unix_secs());
    wait_for(&mut service, |service| {
        !service.snapshot("dodex").unwrap().limits.loading
    });
    let snapshot = service.snapshot("dodex").unwrap();
    assert!(
        snapshot.limits.value.is_some(),
        "{:?}",
        snapshot.limits.error
    );
}

#[test]
fn same_path_account_switch_clears_both_snapshots_before_a_failed_read() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    wait_for(&mut service, |service| {
        let snapshot = service.snapshot("codex").unwrap();
        snapshot.limits.value.is_some() && snapshot.history.value.is_some()
    });
    write_identity(&source.codex_home, true);
    fs::write(source.codex_home.join("mode"), "exit").unwrap();
    service.tick(now + 1);
    wait_for(&mut service, |service| {
        service.snapshot("codex").unwrap().limits.value.is_none()
    });
    let snapshot = service.snapshot("codex").unwrap();
    assert!(
        snapshot.limits.value.is_none(),
        "old user's quota survived account switch"
    );
    assert!(
        snapshot.history.value.is_none(),
        "old user's history survived account switch"
    );
    wait_for(&mut service, |service| {
        !service.snapshot("codex").unwrap().limits.loading
    });
    assert!(service.snapshot("codex").unwrap().limits.value.is_none());
}

fn wait_for(service: &mut UsageService, mut done: impl FnMut(&UsageService) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        service.poll(agent_companion_core::now_unix_secs());
        if done(service) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "usage fixture did not finish: {:?}",
            service.snapshots()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn startup_publishes_limits_while_history_is_still_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    fs::write(source.codex_home.join("mode"), "history-hang").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    service.read_timeout = Duration::from_secs(5);
    let now = agent_companion_core::now_unix_secs();
    let started = Instant::now();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "UI event waited for a worker"
    );
    wait_for(&mut service, |service| {
        service.snapshot("codex").unwrap().limits.value.is_some()
    });
    let snapshot = service.snapshot("codex").unwrap();
    assert!(snapshot.history.loading);
    assert!(snapshot.limits.elapsed_ms.unwrap() < 5_000);
    eprintln!(
        "[usage-fixture] startup to allowance result: {} ms; history still loading",
        snapshot.limits.elapsed_ms.unwrap()
    );
    wait_for(&mut service, |service| {
        !service.snapshot("codex").unwrap().history.loading
    });
    let snapshot = service.snapshot("codex").unwrap();
    assert!(snapshot.history.error.is_some());
    assert!(snapshot.limits.error.is_none());
    let requests = fs::read_to_string(source.codex_home.join("requests.log")).unwrap();
    assert_eq!(requests.matches("account/rateLimits/read").count(), 1);
    assert_eq!(requests.matches("account/usage/read").count(), 1);
    assert_eq!(requests.matches("\"method\":\"initialize\"").count(), 1);
    assert_eq!(requests.matches("\"method\":\"config/read\"").count(), 1);
    assert!(!requests.contains("account/read"));
    assert!(!requests.contains("thread/"));
    assert!(!requests.contains("turn/"));
    assert_eq!(
        fs::read_dir(&source.codex_home)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("pid-"))
            .count(),
        1,
        "quota and history must reuse one owned native worker"
    );
}

#[test]
fn each_instance_uses_its_own_runtime_home_database_and_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let primary = source(directory.path(), "codex");
    let mut secondary = source(directory.path(), "dodex");
    let second_executable = directory.path().join(if cfg!(windows) {
        "second-runtime.exe"
    } else {
        "second-runtime"
    });
    fs::copy(executable(), &second_executable).unwrap();
    secondary.executable_path = Some(second_executable.clone());
    let inherited: Vec<_> = [
        "CODEX_HOME",
        "CODEX_SQLITE_HOME",
        "CODEX_PROFILE",
        "OPENAI_API_KEY",
        "CHATGPT_API_KEY",
        "ELECTRON_RUN_AS_NODE",
        "NODE_OPTIONS",
        "DYLD_INSERT_LIBRARIES",
        "LD_PRELOAD",
        "PATH",
    ]
    .into_iter()
    .map(|name| (OsString::from(name), OsString::from("inherited-secret")))
    .collect();
    let environment = isolated_environment(&secondary, inherited);
    assert_eq!(
        environment.get(&OsString::from("CODEX_HOME")),
        Some(&secondary.codex_home.as_os_str().to_owned())
    );
    assert_eq!(
        environment.get(&OsString::from("CODEX_SQLITE_HOME")),
        Some(&secondary.database_path.as_os_str().to_owned())
    );
    assert_eq!(
        environment.len(),
        3,
        "only PATH and the two instance overrides survive"
    );
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    service.sync_sources(
        vec![primary.clone(), secondary.clone()],
        agent_companion_core::now_unix_secs(),
    );
    wait_for(&mut service, |service| {
        service
            .snapshots()
            .iter()
            .all(|snapshot| !snapshot.limits.loading)
    });
    for source in [primary, secondary] {
        let log = fs::read_to_string(source.codex_home.join("environment.log")).unwrap();
        assert!(log.contains(&source.codex_home.to_string_lossy().to_string()));
        assert!(log.contains(&source.database_path.to_string_lossy().to_string()));
        let args = fs::read_to_string(source.codex_home.join("arguments.log")).unwrap();
        assert!(args.contains("cli_auth_credentials_store=\"file\""));
        assert!(args.contains("sqlite_home="));
        let expected = if source.instance_id == "codex" {
            23
        } else {
            61
        };
        assert_eq!(
            service
                .snapshot(&source.instance_id)
                .unwrap()
                .limits
                .value
                .unwrap()["rateLimits"]["secondary"]["usedPercent"],
            expected
        );
    }
    assert_eq!(
        service.snapshot("dodex").unwrap().source.executable_path,
        Some(second_executable.canonicalize().unwrap())
    );
}

#[test]
fn external_interval_save_rebases_even_when_minutes_are_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.json");
    UsageSettings {
        refresh_interval_minutes: 5,
    }
    .save(&path)
    .unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(path.clone());
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source], now);
    wait_for(&mut service, |service| {
        !service.snapshot("codex").unwrap().limits.loading
    });
    service.tick(now + 1);
    // Force deterministic metadata even on filesystems with coarse stamps.
    let write = |minutes, offset| {
        UsageSettings {
            refresh_interval_minutes: minutes,
        }
        .save(&path)
        .unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(now + offset))
            .unwrap();
    };
    write(5, 2);
    service.tick(now + 2);
    assert_eq!(
        service.snapshot("codex").unwrap().limits.next_query_at,
        Some(now + 302)
    );
    write(1, 3);
    service.tick(now + 3);
    assert_eq!(service.interval_minutes(), 1);
    assert_eq!(
        service.snapshot("codex").unwrap().limits.next_query_at,
        Some(now + 63)
    );
    assert!(!service.snapshot("codex").unwrap().limits.loading);
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(unix)]
fn assert_processes_exit(home: &Path) {
    let pids: Vec<_> = fs::read_dir(home)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_prefix("pid-")?
                .parse::<i32>()
                .ok()
        })
        .collect();
    assert!(!pids.is_empty());
    let end = Instant::now() + Duration::from_secs(3);
    while pids.iter().any(|pid| process_alive(*pid)) {
        assert!(Instant::now() < end, "app-server process survived cleanup");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn assert_processes_exited(home: &Path) {
    let mut count = 0;
    for entry in fs::read_dir(home).unwrap().flatten() {
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_prefix("pid-"))
            .and_then(|pid| pid.parse::<i32>().ok())
        {
            count += 1;
            assert!(
                !process_alive(pid),
                "stop returned before reaping its child"
            );
        }
    }
    assert!(count > 0);
}

#[cfg(unix)]
#[test]
fn timeout_removal_replacement_and_service_drop_reap_children() {
    for action in ["timeout", "remove", "replace", "stop", "drop"] {
        eprintln!("[usage-fixture] lifecycle {action}");
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path(), "codex");
        fs::write(source.codex_home.join("mode"), "hang").unwrap();
        let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
        // A freshly compiled executable may spend more than 100 ms in macOS
        // launch validation under a full parallel test run. The timeout test
        // must reach the fixture, rather than killing it before main starts.
        service.read_timeout = Duration::from_secs(5);
        let now = agent_companion_core::now_unix_secs();
        service.sync_sources(vec![source.clone()], now);
        wait_for(&mut service, |_| {
            source.codex_home.join("requests.log").is_file()
        });
        match action {
            "timeout" => {
                wait_for(&mut service, |service| {
                    !service.snapshot("codex").unwrap().limits.loading
                });
                assert!(service.snapshot("codex").unwrap().limits.error.is_some());
            }
            "remove" => {
                service.sync_sources(Vec::new(), now);
                assert!(service.snapshot("codex").is_none());
            }
            "replace" => {
                let mut replacement = source.clone();
                replacement.executable_path = Some(directory.path().join("missing-runtime"));
                service.sync_sources(vec![replacement], now);
                wait_for(&mut service, |service| {
                    !service.snapshot("codex").unwrap().limits.loading
                });
                assert!(service.snapshot("codex").unwrap().limits.value.is_none());
            }
            "stop" => {
                service.stop();
                assert_processes_exited(&source.codex_home);
            }
            "drop" => {
                drop(service);
                assert_processes_exited(&source.codex_home);
            }
            _ => unreachable!(),
        }
        assert_processes_exit(&source.codex_home);
    }
}

#[test]
fn runtime_exit_is_reported_without_retrying() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    fs::write(source.codex_home.join("mode"), "exit").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    wait_for(&mut service, |service| {
        !service.snapshot("codex").unwrap().limits.loading
    });
    assert!(service.snapshot("codex").unwrap().limits.error.is_some());
    for offset in 1..100 {
        service.tick(now + offset);
    }
    let requests = fs::read_to_string(source.codex_home.join("requests.log")).unwrap();
    assert_eq!(requests.matches("account/rateLimits/read").count(), 1);
}

#[test]
fn token_rotation_does_not_clear_readings_or_trigger_an_extra_query() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().limits.value.is_some()
    });
    let before = service.snapshot("codex").unwrap();
    let path = source.codex_home.join("auth.json");
    let mut auth: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    auth["tokens"]["access_token"] = json!("rotated-synthetic-access");
    auth["tokens"]["refresh_token"] = json!("rotated-synthetic-refresh");
    auth["last_refresh"] = json!("2026-10-02T01:02:03Z");
    fs::write(path, auth.to_string()).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    service.tick(now + 2);
    assert_eq!(service.snapshot("codex").unwrap(), before);
    let requests = fs::read_to_string(source.codex_home.join("requests.log")).unwrap();
    assert_eq!(requests.matches("account/rateLimits/read").count(), 1);
}

#[test]
fn logout_clears_last_success_without_launching_an_unauthenticated_query() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().history.value.is_some()
    });
    fs::remove_file(source.codex_home.join("auth.json")).unwrap();
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().limits.error.is_some()
    });
    service.refresh("codex", now + 1);
    let snapshot = service.snapshot("codex").unwrap();
    assert!(snapshot.limits.value.is_none() && snapshot.history.value.is_none());
    assert!(snapshot.limits.error.unwrap().contains("Sign in"));
    assert!(!snapshot.limits.loading);
    let requests = fs::read_to_string(source.codex_home.join("requests.log")).unwrap();
    assert_eq!(requests.matches("account/rateLimits/read").count(), 1);
}

#[test]
fn request_triggered_switch_replaces_native_account_before_the_periodic_probe() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    fs::write(source.codex_home.join("mode"), "history-hang").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().limits.value.is_some()
    });
    assert!(service.snapshot("codex").unwrap().history.loading);
    write_identity(&source.codex_home, true);
    service.refresh("codex", now + 1);
    wait_for(&mut service, |s| {
        s.snapshot("codex")
            .unwrap()
            .limits
            .value
            .as_ref()
            .is_some_and(|v| v["rateLimits"]["secondary"]["usedPercent"] == 73)
    });
    assert!(service.snapshot("codex").unwrap().history.value.is_none());
    assert_eq!(
        fs::read_dir(&source.codex_home)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("pid-"))
            .count(),
        2
    );
    service.stop();
    #[cfg(unix)]
    assert_processes_exited(&source.codex_home);
}

#[test]
fn queued_old_result_is_rechecked_before_publication() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    fs::write(source.codex_home.join("mode"), "hang").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    wait_for(&mut service, |s| {
        s.jobs.values().any(|r| r.identity.is_some())
    });
    let request = service
        .jobs
        .values()
        .find(|r| r.identity.is_some())
        .unwrap()
        .clone();
    let worker_id = service.workers[&request.source.codex_home].id;
    write_identity(&source.codex_home, true);
    service
        .sender
        .send(Event::Completed {
            worker_id,
            request,
            result: Ok(json!({"rateLimits":{"secondary":{"usedPercent":99}}})),
            completed_at: now,
            elapsed_ms: 1,
        })
        .unwrap();
    service.poll(now);
    assert!(service.snapshot("codex").unwrap().limits.value.is_none());
    assert!(
        service
            .jobs
            .values()
            .all(|r| r.identity.as_ref().is_some_and(|i| i.user == "user-b"))
    );
}

#[test]
fn one_instance_timeout_does_not_delay_the_other_instances_quota_or_history() {
    let directory = tempfile::tempdir().unwrap();
    let primary = source(directory.path(), "codex");
    let secondary = source(directory.path(), "dodex");
    fs::write(primary.codex_home.join("mode"), "hang").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    service.read_timeout = READ_TIMEOUT;
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![primary, secondary], now);
    service.load_history("dodex", now);
    wait_for(&mut service, |s| {
        s.snapshot("dodex").unwrap().limits.value.is_some()
            && s.snapshot("dodex").unwrap().history.value.is_some()
    });
    assert!(service.snapshot("codex").unwrap().limits.loading);
    wait_for(&mut service, |s| {
        !s.snapshot("codex").unwrap().limits.loading
    });
    assert!(service.snapshot("codex").unwrap().limits.error.is_some());
    assert!(service.snapshot("dodex").unwrap().limits.error.is_none());
}

#[test]
fn changed_backend_origin_invalidates_both_account_caches() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().history.value.is_some()
    });
    fs::write(source.codex_home.join("mode"), "hang").unwrap();
    fs::write(
        source.codex_home.join("config.toml"),
        "chatgpt_base_url='http://127.0.0.1:1/backend-api'\n",
    )
    .unwrap();
    service.refresh("codex", now + 1);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().limits.value.is_none()
    });
    assert!(service.snapshot("codex").unwrap().history.value.is_none());
}

#[test]
fn native_config_mismatch_clears_caches_without_retry_loop_and_manual_refresh_recovers() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    let now = agent_companion_core::now_unix_secs();
    service.sync_sources(vec![source.clone()], now);
    service.load_history("codex", now);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().history.value.is_some()
    });
    fs::write(source.codex_home.join("mode"), "wrong-config").unwrap();
    service.refresh("codex", now + 1);
    wait_for(&mut service, |s| {
        s.snapshot("codex")
            .unwrap()
            .limits
            .error
            .as_ref()
            .is_some_and(|error| error.contains("configuration could not be verified"))
    });
    let snapshot = service.snapshot("codex").unwrap();
    assert!(snapshot.limits.value.is_none() && snapshot.history.value.is_none());
    std::thread::sleep(Duration::from_millis(1100));
    service.tick(now + 3);
    let requests = fs::read_to_string(source.codex_home.join("requests.log")).unwrap();
    assert_eq!(requests.matches("account/rateLimits/read").count(), 1);
    assert_eq!(requests.matches("config/read").count(), 2);
    fs::write(source.codex_home.join("mode"), "").unwrap();
    service.refresh("codex", now + 4);
    wait_for(&mut service, |s| {
        s.snapshot("codex").unwrap().limits.value.is_some()
    });
    assert!(service.snapshot("codex").unwrap().limits.error.is_none());
}
