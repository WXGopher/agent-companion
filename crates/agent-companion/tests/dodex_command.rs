#![cfg(windows)]

use std::{fs, process::Command};

#[test]
fn standalone_native_command_dispatches_help_and_rejects_arguments_without_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("用户 a & b's %tools%!");
    fs::create_dir(&directory).unwrap();
    let command = directory.join("dodex.exe");
    fs::copy(env!("CARGO_BIN_EXE_agent-companion"), &command).unwrap();

    let help = Command::new(&command).arg("--help").output().unwrap();
    assert!(help.status.success());
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(output.contains("Usage: dodex"), "{output}");
    assert!(output.contains("--deploy") && output.contains("--check"));

    for args in [
        vec!["--deploy", "--check"],
        vec!["--unknown"],
        vec!["a b&c'd%!"],
    ] {
        let output = Command::new(&command).args(&args).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(
            !output.stderr.is_empty(),
            "errors must remain visible in a shell"
        );
    }
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
}
