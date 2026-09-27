#![cfg(target_os = "macos")]

/// Exercise the real native view hierarchy offscreen, including its AppKit
/// scroll view and changing intrinsic size. The existing macOS CI test job
/// therefore covers the Swift layer as well as the Rust snapshot reader.
#[test]
fn native_dashboard_layouts() {
    use agent_companion_core::dashboard::{Instance, Snapshot, Weekly};

    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("snapshot.json");
    std::fs::write(
        &snapshot,
        serde_json::to_vec(
            &Snapshot {
                active_count: 7,
                weekly: Some(Weekly {
                    used_percent: 45,
                    resets_at: Some(4_000_000_000),
                    expired: false,
                }),
                codex_home: "/synthetic/.codex".into(),
                ..Snapshot::default()
            }
            .with_instance(Instance {
                instance_id: "codex".into(),
                label: "Codex".into(),
                codex_home: "/synthetic/.codex".into(),
                database_path: Some("/synthetic/.codex".into()),
                ..Default::default()
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let update_snapshot = temp.path().join("update-snapshot.json");
    std::fs::write(
        &update_snapshot,
        serde_json::to_vec(&serde_json::json!({
            "latestVersion": "0.3.22",
            "releaseUrl": "https://github.com/WXGopher/agent-companion/releases/tag/v0.3.22"
        }))
        .unwrap(),
    )
    .unwrap();
    let usage_snapshot = temp.path().join("usage-snapshot.json");
    let usage = agent_companion_core::usage_service::InstanceSnapshot {
        source: agent_companion_core::usage_service::Source {
            instance_id: "codex".into(),
            codex_home: "/synthetic/.codex".into(),
            executable_path: None,
            database_path: "/synthetic/.codex".into(),
        },
        limits: agent_companion_core::usage_service::QuerySnapshot {
            value: Some(serde_json::json!({"rateLimits": {
                "secondary": {"usedPercent":45,"windowDurationMins":10080,"resetsAt":4_000_000_000u64}
            }})),
            last_success_at: Some(1_000),
            ..Default::default()
        },
        history: Default::default(),
    };
    std::fs::write(
        &usage_snapshot,
        serde_json::to_vec(&serde_json::json!({
            "intervalMinutes": 5, "instances": [usage]
        }))
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
        .env("AGENT_COMPANION_TEST_USAGE_SNAPSHOT", usage_snapshot)
        .env("AGENT_COMPANION_TEST_UPDATE_SNAPSHOT", update_snapshot)
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
