#![cfg(windows)]

use std::{fs, process::Command};

fn console_copy(source: &std::path::Path, target: &std::path::Path) {
    let mut bytes = fs::read(source).unwrap();
    let pe = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    let optional = pe + 24;
    bytes[optional + 64..optional + 70].copy_from_slice(&[0, 0, 0, 0, 3, 0]);
    fs::write(target, bytes).unwrap();
}

#[test]
fn standalone_command_requires_its_deployment_and_reserves_only_management_flags() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("用户 a & b's %tools%!");
    fs::create_dir(&directory).unwrap();
    let command = directory.join("dodex.exe");
    console_copy(
        std::path::Path::new(env!("CARGO_BIN_EXE_agent-companion")),
        &command,
    );

    let local = directory.join("local");
    let help = Command::new(&command)
        .args(["--deploy", "--help"])
        .env("LOCALAPPDATA", &local)
        .output()
        .unwrap();
    assert!(help.status.success());
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(output.contains("Usage: dodex"), "{output}");
    assert!(output.contains("--deploy") && output.contains("--check"));

    for (args, usage) in [
        (["app", "--help"], "Usage: dodex app"),
        (["update", "--help"], "Usage: dodex update"),
    ] {
        let help = Command::new(&command)
            .args(args)
            .env("LOCALAPPDATA", &local)
            .output()
            .unwrap();
        assert!(help.status.success());
        assert!(String::from_utf8_lossy(&help.stdout).contains(usage));
    }

    // Even --help/--version belong to Codex, so an undeployed environment must
    // fail closed instead of printing Companion help or launching the desktop.
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["--unknown"],
        vec!["a b&c'd%!"],
        vec!["exec", "--deploy"],
        vec!["--", "update"],
        vec![
            "resume",
            "synthetic-id",
            "--",
            "-c",
            "sqlite_home=literal prompt",
        ],
    ] {
        let output = Command::new(&command)
            .args(&args)
            .env("LOCALAPPDATA", &local)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).starts_with("dodex: "),
            "CLI arguments must reach deployment validation: {args:?}"
        );
    }
    for args in [["--deploy", "--check"], ["--check", "exec"]] {
        let output = Command::new(&command)
            .args(args)
            .env("LOCALAPPDATA", &local)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: dodex"));
    }
    assert!(
        !local.exists(),
        "invalid invocations must not deploy a profile"
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn gui_copy_routes_to_the_desktop_entry_even_with_redirected_stdio() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("App 用户 a & b's %tools%!");
    fs::create_dir(&directory).unwrap();
    let command = directory.join("Dodex.exe");
    fs::copy(env!("CARGO_BIN_EXE_agent-companion"), &command).unwrap();
    let local = directory.join("local");
    let output = Command::new(&command)
        .arg("--help")
        .env("LOCALAPPDATA", &local)
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("Open the isolated Dodex desktop app."),
        "{help}"
    );
    assert!(help.to_ascii_lowercase().contains("usage: dodex"), "{help}");
    assert!(!local.exists(), "desktop help must not deploy a profile");
    // CLI arguments cannot accidentally run an agent from the Start entry.
    let output = Command::new(&command)
        .args(["exec", "example"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
}
