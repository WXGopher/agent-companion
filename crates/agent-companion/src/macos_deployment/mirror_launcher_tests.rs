use super::*;

fn executable(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn bundle(app: &Path, native: &str, updater: Option<&Path>, environment: Value, script: &[u8]) {
    executable(
        &app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE),
        LAUNCHER,
    );
    executable(&app.join("Contents/MacOS").join(native), script);
    let mut plist = json!({
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleExecutable": LAUNCHER_EXECUTABLE,
        "CFBundlePackageType": "APPL",
        "DodexNativeExecutable": native,
        "LSEnvironment": environment,
    });
    if let Some(updater) = updater {
        plist["DodexUpdaterExecutable"] = json!(updater);
    }
    let path = app.join("Contents/Info.plist");
    fs::write(&path, serde_json::to_vec(&plist).unwrap()).unwrap();
    command_ok(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "xml1"])
            .arg(path),
    )
    .unwrap();
}

fn launcher(app: &Path) -> Command {
    let mut command = Command::new(app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE));
    command
        .args(["--user-data-dir=/wrong-profile", "--", "ignored argument"])
        .env("CODEX_ELECTRON_USER_DATA_PATH", "/wrong-inherited-profile")
        .env("CODEX_HOME", "/wrong-inherited-home")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn fields(bytes: &[u8]) -> Vec<&str> {
    bytes
        .strip_suffix(&[0])
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|field| std::str::from_utf8(field).unwrap())
        .collect()
}

fn atomic_swapper(root: &Path) -> PathBuf {
    let source = root.join("swap.c");
    let swapper = root.join("swap");
    fs::write(
        &source,
        b"#include <stdio.h>\nint main(int argc, char **argv) {\n    return argc == 3 && renamex_np(argv[1], argv[2], RENAME_SWAP) == 0 ? 0 : 1;\n}\n",
    )
    .unwrap();
    let output = Command::new("xcrun")
        .args(["clang", "-Wall", "-Wextra", "-Werror"])
        .arg(source)
        .arg("-o")
        .arg(&swapper)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    swapper
}

#[test]
fn bootstrap_waits_for_direct_child_sync_then_execs_replaced_public_bundle() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let app = root.join("Dodex's $HOME `app` $(app).app");
    let stage = root.join("replacement.app");
    let swapper = atomic_swapper(&root);
    let updater = root.join("Companion's $HOME `updater` $(updater)");
    let log = root.join("order");
    let parent_comm = root.join("parent-comm");
    let data = root.join("new desktop's $HOME `data` $(data)");
    let home = root.join("new codex-home");
    executable(
        &updater,
        br#"#!/bin/sh
set -eu
printf '%s\0' updater "$$" "$PPID" "$@" > "$TEST_SYNC_LOG"
"$TEST_SWAPPER" "$3" "$TEST_NEXT_APP"
/bin/rm -r "$TEST_NEXT_APP"
/bin/ps -p "$PPID" -o comm= > "$TEST_PARENT_COMM"
printf '%s\0' updated >> "$TEST_SYNC_LOG"
"#,
    );
    bundle(
        &app,
        "OldNative",
        Some(&updater),
        json!({"CODEX_ELECTRON_USER_DATA_PATH": root.join("old desktop")}),
        b"#!/bin/sh\nprintf old-native\nexit 61\n",
    );
    let native = "New native's executable";
    bundle(
        &stage,
        native,
        Some(&updater),
        json!({
            "CODEX_ELECTRON_USER_DATA_PATH": data,
            "CODEX_HOME": home,
            "CODEX_CLI_PATH": "",
            "CODEX_APP_SERVER_FORCE_CLI": "0",
            "CODEX_SPARKLE_ENABLED": "false",
            "TEST_STRING": "new value with ' $HOME `data` $(data)",
            "TEST_NON_STRING": 7,
        }),
        br#"#!/bin/sh
set -eu
printf '%s\0' native "$$" >> "$TEST_SYNC_LOG"
printf '%s\0' "$$" "$0" "$@" "$CODEX_HOME" "$CODEX_ELECTRON_USER_DATA_PATH" "$CODEX_CLI_PATH" "$CODEX_APP_SERVER_FORCE_CLI" "$CODEX_SPARKLE_ENABLED" "$TEST_STRING" "${TEST_NON_STRING-unset}"
exit 37
"#,
    );
    let child = launcher(&app)
        .env("TEST_SYNC_LOG", &log)
        .env("TEST_SWAPPER", &swapper)
        .env("TEST_NEXT_APP", &stage)
        .env("TEST_PARENT_COMM", &parent_comm)
        .env("CODEX_CLI_PATH", "/stale/cli")
        .env("CODEX_APP_SERVER_FORCE_CLI", "1")
        .env("CODEX_SPARKLE_ENABLED", "true")
        .env("TEST_STRING", "stale")
        .env_remove("TEST_NON_STRING")
        .spawn()
        .unwrap();
    let pid = child.id().to_string();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(37));
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let order = fs::read(log).unwrap();
    let order = fields(&order);
    assert_eq!(order.len(), 9);
    assert_eq!(order[0], "updater");
    assert_ne!(order[1], pid);
    assert_eq!(order[2], pid, "updater must be a direct child of bootstrap");
    assert_eq!(
        &order[3..],
        [
            "dodex-app",
            "--sync-on-launch",
            app.to_str().unwrap(),
            "updated",
            "native",
            &pid,
        ]
    );
    assert_eq!(
        fields(&output.stdout),
        [
            pid,
            app.join("Contents/MacOS")
                .join(native)
                .display()
                .to_string(),
            format!("--user-data-dir={}", data.display()),
            home.display().to_string(),
            data.display().to_string(),
            String::new(),
            "0".into(),
            "false".into(),
            "new value with ' $HOME `data` $(data)".into(),
            "unset".into(),
        ]
    );
    assert_eq!(
        fs::read_to_string(parent_comm).unwrap().trim_end(),
        app.join("Contents/MacOS")
            .join(LAUNCHER_EXECUTABLE)
            .to_str()
            .unwrap(),
        "a waiting bootstrap stays identifiable at its public path after replacement"
    );
    assert!(!app.join("Contents/MacOS/OldNative").exists());
    assert!(!stage.exists());
}

const FALLBACK_NATIVE: &[u8] = br#"#!/bin/sh
printf '%s\0' "$$" "$0" "$@" "$CODEX_HOME" "$CODEX_ELECTRON_USER_DATA_PATH"
exit 37
"#;

#[test]
fn bootstrap_forwards_only_an_existing_project_and_keeps_the_secondary_profile() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let app = root.join("Public Dodex.app");
    let workspace = root.join("project with spaces");
    fs::create_dir(&workspace).unwrap();
    bundle(
        &app,
        "CurrentNative",
        Some(Path::new("/usr/bin/true")),
        json!({"CODEX_HOME": root.join("same-second"), "CODEX_ELECTRON_USER_DATA_PATH": root.join("same-desktop")}),
        b"#!/bin/sh\nprintf '%s\\0' \"$@\" \"$CODEX_HOME\"\n",
    );
    let output = launcher(&app)
        .arg("--open-project")
        .arg(&workspace)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fields(&output.stdout),
        [
            format!("--user-data-dir={}", root.join("same-desktop").display()),
            "--open-project".into(),
            workspace.display().to_string(),
            root.join("same-second").display().to_string(),
        ]
    );
    for invalid in ["relative", "/nonexistent-synthetic-project"] {
        let output = launcher(&app)
            .args(["--open-project", invalid])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn bootstrap_refuses_to_start_when_updater_fails() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let app = root.join("Dodex.app");
    let updater = root.join("failing-updater");
    let data = root.join("desktop-data");
    let home = root.join("codex-home");
    executable(
        &updater,
        b"#!/bin/sh\nprintf suppressed-output\nprintf suppressed-error >&2\nexit 23\n",
    );
    bundle(
        &app,
        "CurrentNative",
        Some(&updater),
        json!({
            "CODEX_ELECTRON_USER_DATA_PATH": data,
            "CODEX_HOME": home,
        }),
        FALLBACK_NATIVE,
    );
    let output = launcher(&app).output().unwrap();
    assert_eq!(output.status.code(), Some(78));
    assert!(
        output.stdout.is_empty(),
        "failed isolation validation must not start native app"
    );
}

#[test]
fn bootstrap_refuses_to_start_when_updater_is_missing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let missing_updater = root.join("missing-updater");
    let data = root.join("desktop-data");
    let home = root.join("codex-home");
    for (name, updater) in [
        ("no-key.app", None),
        ("missing-file.app", Some(missing_updater.as_path())),
    ] {
        let app = root.join(name);
        bundle(
            &app,
            "CurrentNative",
            updater,
            json!({
                "CODEX_ELECTRON_USER_DATA_PATH": data,
                "CODEX_HOME": home,
            }),
            FALLBACK_NATIVE,
        );
        let output = launcher(&app).output().unwrap();
        assert_eq!(output.status.code(), Some(78));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn bootstrap_and_its_updater_remove_inherited_identity_but_keep_containment() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let app = root.join("Dodex.app");
    let updater = root.join("updater");
    let log = root.join("updater-environment");
    executable(
        &updater,
        b"#!/bin/sh\n/usr/bin/env > \"$TEST_UPDATER_ENV\"\n",
    );
    bundle(
        &app,
        "Native",
        Some(&updater),
        json!({"CODEX_HOME": root.join("second"), "CODEX_ELECTRON_USER_DATA_PATH": root.join("desktop")}),
        b"#!/bin/sh\n/usr/bin/env\n",
    );
    let mut command = launcher(&app);
    command.env("TEST_UPDATER_ENV", &log);
    let unsafe_names = [
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_API_KEY",
        "CODEX_CONFIG_FILE",
        "OPENAI_API_KEY",
        "OPENAI_ACCESS_TOKEN",
        "CHATGPT_ACCESS_TOKEN",
        "OPENAI_IDENTITY_TOKEN_FILE",
        "NODE_OPTIONS",
        "NODE_PATH",
        "ELECTRON_RUN_AS_NODE",
    ];
    let keep_names = [
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
        "CODEX_NETWORK_PROXY_ACTIVE",
        "CODEX_CA_CERTIFICATE",
        "HTTPS_PROXY",
    ];
    for name in unsafe_names.into_iter().chain(keep_names) {
        command.env(name, "synthetic");
    }
    let output = command.output().unwrap();
    assert!(output.status.success());
    for bytes in [output.stdout, fs::read(log).unwrap()] {
        let names: Vec<_> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
            .collect();
        for name in unsafe_names {
            assert!(!names.iter().any(|actual| actual == name), "leaked {name}");
        }
        for name in keep_names {
            assert!(names.iter().any(|actual| actual == name), "lost {name}");
        }
    }
}
