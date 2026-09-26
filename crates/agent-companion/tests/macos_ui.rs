#![cfg(target_os = "macos")]

/// Exercise the real native view hierarchy offscreen, including its AppKit
/// scroll view and changing intrinsic size. The existing macOS CI test job
/// therefore covers the Swift layer as well as the Rust snapshot reader.
#[test]
fn native_dashboard_layouts() {
    use agent_companion_core::dashboard::{Snapshot, Weekly};

    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_vec(&Snapshot {
            active_count: 7,
            weekly: Some(Weekly {
                used_percent: 45,
                resets_at: Some(4_000_000_000),
                expired: false,
            }),
            codex_home: "/synthetic/.codex".into(),
            ..Snapshot::default()
        })
        .unwrap(),
    )
    .unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let result = std::process::Command::new("sh")
        .arg("scripts/test-macos-ui.sh")
        .env("AGENT_COMPANION_TEST_SNAPSHOT", snapshot)
        .env("AGENT_COMPANION_TEST_VERSION", env!("CARGO_PKG_VERSION"))
        .current_dir(root)
        .output()
        .expect("could not run native macOS layout checks");
    assert!(
        result.status.success(),
        "native layout checks failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
