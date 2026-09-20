#![cfg(windows)]

use std::{fs, process::Command};

#[test]
fn standalone_command_requires_its_deployment_and_reserves_only_management_flags() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("用户 a & b's %tools%!");
    fs::create_dir(&directory).unwrap();
    let command = directory.join("dodex.exe");
    fs::copy(env!("CARGO_BIN_EXE_agent-companion"), &command).unwrap();

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

    // Even --help/--version belong to Codex, so an undeployed environment must
    // fail closed instead of printing Companion help or launching the desktop.
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["--unknown"],
        vec!["a b&c'd%!"],
        vec!["exec", "--deploy"],
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
