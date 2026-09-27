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
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(
                request,
                json!({"id":2,"method":if kind == QueryKind::Limits { "account/rateLimits/read" } else { "account/usage/read" }})
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
                .write_all(format!("{}\n", json!({"id":2,"result":value})).as_bytes())
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
            (directory, executable)
        })
        .1
        .clone()
}

fn source(root: &Path, id: &str) -> Source {
    let home = root.join(id);
    fs::create_dir_all(&home).unwrap();
    Source {
        instance_id: id.into(),
        codex_home: home.clone(),
        executable_path: Some(executable()),
        database_path: home.join("database with spaces"),
    }
}

fn wait_for(service: &mut UsageService, mut done: impl FnMut(&UsageService) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        service.poll(agent_companion_core::now_unix_secs());
        if done(service) {
            return;
        }
        assert!(Instant::now() < deadline, "usage fixture did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn startup_publishes_limits_while_history_is_still_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "codex");
    fs::write(source.codex_home.join("mode"), "history-hang").unwrap();
    let mut service = UsageService::with_settings_path(directory.path().join("usage.json"));
    service.read_timeout = Duration::from_millis(800);
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
    assert!(snapshot.limits.elapsed_ms.unwrap() < 800);
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
    assert!(!requests.contains("account/read"));
    assert!(!requests.contains("thread/"));
    assert!(!requests.contains("turn/"));
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
        assert_eq!(
            args.contains("cli_auth_credentials_store=\"file\""),
            source.instance_id == "dodex"
        );
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
        service.read_timeout = Duration::from_secs(2);
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
