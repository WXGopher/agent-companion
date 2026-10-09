//! Native entry contracts run on both macOS and Windows with disposable state.
use agent_companion_core::tui_instance::{self, Channel, Layout, Registry, SCHEMA};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

fn fixture() -> (tempfile::TempDir, Layout, Registry) {
    let temporary = tempfile::tempdir().unwrap();
    let user_home = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("user a & b's tools");
    fs::create_dir_all(&user_home).unwrap();
    let support = if cfg!(target_os = "macos") {
        user_home.join("Library/Application Support/AgentCompanion")
    } else if cfg!(windows) {
        user_home.join("local/AgentCompanion")
    } else {
        user_home.join(".local/share/agent-companion")
    };
    let layout = Layout { user_home, support };
    let home = layout.support.join("DodexApp/codex-home");
    let instance = layout.secondary(home.clone(), home.join("sqlite"), home.join("log"));
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("config.toml"),
        format!(
            "cli_auth_credentials_store=\"file\"\nsqlite_home={}\nlog_dir={}\n",
            serde_json::to_string(&instance.database_dir.to_string_lossy()).unwrap(),
            serde_json::to_string(&instance.log_dir.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    let package = home.join("packages/standalone/releases/1.0.0");
    fs::create_dir_all(package.join("bin")).unwrap();
    let native = package.join("bin").join(tui_instance::executable_name());
    let status = Command::new("rustc")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tui_probe.rs"
        ))
        .arg("-o")
        .arg(&native)
        .status()
        .unwrap();
    assert!(status.success());
    for member in [
        if cfg!(windows) {
            "bin/codex-code-mode-host.exe"
        } else {
            "bin/codex-code-mode-host"
        },
        if cfg!(windows) {
            "codex-path/rg.exe"
        } else {
            "codex-path/rg"
        },
    ] {
        let path = package.join(member);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(&native, path).unwrap();
    }
    fs::create_dir_all(package.join("codex-resources")).unwrap();
    fs::write(package.join("codex-package.json"), serde_json::to_vec(&serde_json::json!({
        "layoutVersion":1, "version":"1.0.0", "entrypoint":format!("bin/{}", tui_instance::executable_name()),
        "resourcesDir":"codex-resources", "pathDir":"codex-path"
    })).unwrap()).unwrap();
    let current = home.join("packages/standalone/current");
    select(&package, &current);
    let primary_home = layout.user_home.join(".codex");
    let primary_native = layout
        .user_home
        .join("primary-runtime")
        .join(tui_instance::executable_name());
    fs::create_dir_all(primary_native.parent().unwrap()).unwrap();
    fs::copy(&native, &primary_native).unwrap();
    let primary = tui_instance::InstanceConfig {
        id: "codex".into(),
        label: "Codex".into(),
        codex_home: primary_home.clone(),
        database_dir: primary_home.clone(),
        log_dir: primary_home.join("log"),
        install_dir: layout.user_home.join("primary-bin"),
        cli_path: primary_native,
        command_path: layout.public_bin().join(tui_instance::executable_name()),
        channel: Channel::Native,
        updater: None,
    };
    fs::create_dir_all(layout.public_bin()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_dodex"), &instance.command_path).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_dodex"), &primary.command_path).unwrap();
    let registry = Registry {
        schema: SCHEMA,
        primary: Some(primary),
        dodex: instance,
        dodex_enabled: false,
    };
    fs::write(layout.record(), serde_json::to_vec(&registry).unwrap()).unwrap();
    (temporary, layout, registry)
}
fn select(package: &Path, current: &Path) {
    if fs::symlink_metadata(current).is_ok() {
        #[cfg(unix)]
        fs::remove_file(current).unwrap();
        #[cfg(windows)]
        fs::remove_dir(current).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(package, current).unwrap();
    #[cfg(windows)]
    {
        // PowerShell's filesystem provider expects drive paths rather than
        // Rust's verbatim spelling. The native registry retains its paths.
        let provider_path = |path: &Path| {
            path.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_owned()
        };
        assert!(Command::new("powershell.exe").args(["-NoProfile", "-Command", "New-Item -ItemType Junction -Path $env:TEST_CURRENT -Target $env:TEST_RELEASE | Out-Null"])
            .env("TEST_CURRENT", provider_path(current)).env("TEST_RELEASE", provider_path(package)).status().unwrap().success());
        assert_eq!(
            current.canonicalize().unwrap(),
            package.canonicalize().unwrap()
        );
    }
}
fn launch(layout: &Layout, entry: &Path) -> Command {
    let mut command = Command::new(entry);
    command
        .env("HOME", &layout.user_home)
        .env("USERPROFILE", &layout.user_home)
        .env("LOCALAPPDATA", layout.user_home.join("local"))
        .env("CODEX_HOME", "/inherited-wrong-account")
        .env("CODEX_SQLITE_HOME", "/inherited-wrong-database")
        .env("CODEX_INSTALL_DIR", "/inherited-wrong-install")
        .env("CODEX_CLI_PATH", "/inherited-wrong-runtime")
        .env("CODEX_THREAD_ID", "inherited-session")
        .env("CODEX_DAEMON_SOCKET", "inherited-socket")
        .env("CODEX_APP_SERVER_USE_LOCAL_DAEMON", "0")
        .env("CODEX_ACCESS_TOKEN", "synthetic-wrong-account")
        .env("OPENAI_API_KEY", "synthetic-wrong-account")
        .env("CODEX_SANDBOX", "inherited-containment")
        .env("HTTPS_PROXY", "http://synthetic-proxy.invalid")
        .current_dir(&layout.user_home);
    command
}

#[test]
fn terminal_arguments_identity_stdin_cwd_exit_and_dynamic_versions_are_native() {
    let (_temporary, layout, registry) = fixture();
    for arguments in [
        vec!["resume", "id", "--", "app"],
        vec!["exec", "--exit-73"],
        vec!["login", "status"],
        vec!["mcp", "list"],
        vec!["plugin", "list"],
        vec!["app-server", "daemon", "status"],
        vec!["agents"],
        vec!["queue", "id", "message"],
        vec!["fork", "id"],
        vec!["--no-daemon", "resume", "id"],
        vec!["update", "--help"],
    ] {
        let mut child = launch(&layout, &registry.dodex.command_path)
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"native input")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(if arguments.contains(&"--exit-73") {
                73
            } else {
                0
            }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!(
            "CODEX_HOME={}",
            registry.dodex.codex_home.display()
        )));
        assert!(text.contains(&format!(
            "CODEX_INSTALL_DIR={}",
            registry.dodex.install_dir.display()
        )));
        assert!(text.contains("stdin=native input"));
        let native_cwd = text
            .lines()
            .find_map(|line| line.strip_prefix("cwd="))
            .unwrap();
        assert_eq!(
            Path::new(native_cwd).canonicalize().unwrap(),
            layout.user_home.canonicalize().unwrap(),
            "The native working directory must remain the original directory"
        );
        for name in [
            "CODEX_CLI_PATH",
            "CODEX_THREAD_ID",
            "CODEX_DAEMON_SOCKET",
            "CODEX_APP_SERVER_USE_LOCAL_DAEMON",
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
        ] {
            assert!(text.contains(&format!("{name}=ABSENT")), "{text}");
        }
        assert!(text.contains("CODEX_SANDBOX=inherited-containment"));
        assert!(text.contains("HTTPS_PROXY=http://synthetic-proxy.invalid"));
        assert_eq!(
            text.lines().next().unwrap(),
            format!("args={arguments:?}"),
            "Native daemon attachment must not see synthetic overrides"
        );
        assert_eq!(
            text.contains("--no-daemon"),
            arguments.contains(&"--no-daemon")
        );
        assert!(
            text.contains(
                format!("{:?}", arguments)
                    .trim_start_matches('[')
                    .trim_end_matches(']')
            ),
            "arguments changed: {text}"
        );
    }
    let output = launch(&layout, &registry.primary.as_ref().unwrap().command_path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(&format!(
        "CODEX_HOME={}",
        registry.primary.as_ref().unwrap().codex_home.display()
    )));
    assert!(!text.contains("cli_auth_credentials_store"));
    for args in [["app"], ["help"]] {
        let mut cmd = launch(&layout, &registry.dodex.command_path);
        if args == ["help"] {
            cmd.args(["help", "app"]);
        } else {
            cmd.args(args);
        }
        let output = cmd.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("no longer supported"));
    }
    let first = registry.dodex.runtime().unwrap();
    let second = registry
        .dodex
        .codex_home
        .join("packages/standalone/releases/2.0.0");
    fn copy(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &destination.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
    }
    copy(first.parent().unwrap().parent().unwrap(), &second);
    let record_before = fs::read(layout.record()).unwrap();
    select(
        &second,
        &registry
            .dodex
            .codex_home
            .join("packages/standalone/current"),
    );
    let output = launch(&layout, &registry.dodex.command_path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let runtime = text
        .lines()
        .find_map(|line| line.strip_prefix("runtime="))
        .unwrap();
    assert_eq!(
        Path::new(runtime).canonicalize().unwrap(),
        second
            .join("bin")
            .join(tui_instance::executable_name())
            .canonicalize()
            .unwrap()
    );
    assert_eq!(fs::read(layout.record()).unwrap(), record_before);
}

#[test]
fn primary_help_does_not_invoke_its_package_manager() {
    let (_temporary, layout, mut registry) = fixture();
    let primary = registry.primary.as_mut().unwrap();
    let manager = primary.cli_path.with_file_name(if cfg!(windows) {
        "package-manager.exe"
    } else {
        "package-manager"
    });
    fs::copy(&primary.cli_path, &manager).unwrap();
    primary.channel = Channel::Homebrew;
    primary.updater = Some(manager);
    let entry = primary.command_path.clone();
    fs::write(layout.record(), serde_json::to_vec(&registry).unwrap()).unwrap();
    let before = fs::read(layout.record()).unwrap();
    for arguments in [
        vec!["help", "update"],
        vec!["update", "--help"],
        vec!["update", "-h"],
        vec!["update", "-hc", "model='fixture'"],
        vec!["update", "--unknown"],
        vec!["update", "unexpected-value"],
    ] {
        let output = launch(&layout, &entry).args(&arguments).output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!("args={arguments:?}")), "{text}");
        assert!(
            !text.contains("package-manager"),
            "Help invoked the updater: {text}"
        );
    }
    let output = launch(&layout, &entry).arg("update").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("args=[\"upgrade\", \"--cask\", \"codex\"]"),
        "{text}"
    );
    assert!(text.contains("package-manager"), "{text}");
    assert_eq!(fs::read(layout.record()).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn exec_keeps_pid_and_native_signal_exit() {
    use std::os::unix::process::ExitStatusExt;
    let (_temporary, layout, registry) = fixture();
    let mut child = launch(&layout, &registry.dodex.command_path)
        .arg("--signal-wait")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
}

#[cfg(windows)]
#[test]
fn windows_console_ctrl_c_and_full_native_exit_code_are_preserved() {
    use std::os::windows::process::CommandExt;
    let (_temporary, layout, registry) = fixture();
    let output = launch(&layout, &registry.dodex.command_path)
        .arg("--exit-32")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0x1234),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut child = launch(&layout, &registry.dodex.command_path)
        .arg("--signal-wait")
        .creation_flags(0x00000010)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    assert!(
        Command::new(registry.dodex.runtime().unwrap())
            .args(["--send-ctrl-c", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(child.wait().unwrap().code(), Some(77));
}
