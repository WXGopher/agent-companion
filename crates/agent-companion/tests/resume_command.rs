//! Only synthetic homes and rollouts are used; no real account/runtime is opened.
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SESSION: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_SESSION: &str = "00000000-0000-4000-8000-000000000002";
const DODEX_SESSION: &str = "00000000-0000-4000-8000-000000000003";

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    local: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("user");
        let project = root.join("project");
        let local = root.join("local");
        for path in [&home, &project, &local, &project.join("child")] {
            fs::create_dir_all(path).unwrap();
        }
        let primary = home.join(".codex");
        rollout(&primary, &project, SESSION, "original work");
        rollout(
            &primary,
            &project.join("child"),
            OTHER_SESSION,
            "child project",
        );
        #[cfg(target_os = "macos")]
        let support = home.join("Library/Application Support/AgentCompanion");
        #[cfg(windows)]
        let support = local.join("AgentCompanion");
        #[cfg(not(any(target_os = "macos", windows)))]
        let support = home.join(".local/share/agent-companion");
        let dodex_home = support.join("Dodex/codex-home");
        rollout(&dodex_home, &project, DODEX_SESSION, "second environment");
        #[cfg(target_os = "macos")]
        let record = support.join("dual-instance.json");
        #[cfg(not(target_os = "macos"))]
        let record = support.join("Dodex/companion-deployment.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(record, serde_json::to_vec(&json!({"schema":1, "enabled":false, "instance": {
            "codex_home": dodex_home, "database_dir": support.join("Dodex/sqlite"), "cli_path": support.join("Dodex/missing-codex")
        }})).unwrap()).unwrap();
        Self {
            _temporary: temporary,
            home,
            project,
            local,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_acomp"));
        command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("LOCALAPPDATA", &self.local)
            .env_remove("CODEX_HOME");
        command
    }
    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }
}

fn rollout(home: &Path, cwd: &Path, id: &str, title: &str) {
    let directory = home.join("sessions/2026/01/01");
    fs::create_dir_all(&directory).unwrap();
    let contents = [
        json!({"timestamp":"2026-01-01T00:00:00Z", "type":"session_meta", "payload":{"id":id,"cwd":cwd,"originator":"codex_cli_rs","source":"cli","cli_version":"0.0.0-unverified"}}),
        json!({"timestamp":"2026-01-01T00:00:01Z", "type":"event_msg", "payload":{"type":"user_message","message":title}}),
    ].map(|value| value.to_string()).join("\n");
    fs::write(
        directory.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl")),
        contents + "\n",
    )
    .unwrap();
}

#[test]
fn both_entries_offer_resume_help_and_version() {
    for (binary, name) in [
        (env!("CARGO_BIN_EXE_acomp"), "acomp"),
        (env!("CARGO_BIN_EXE_agent-companion"), "agent-companion"),
    ] {
        let version = Command::new(binary).arg("--version").output().unwrap();
        assert!(version.status.success());
        assert_eq!(
            String::from_utf8(version.stdout).unwrap().trim(),
            format!("{name} {}", env!("CARGO_PKG_VERSION"))
        );
        let help = Command::new(binary)
            .args(["resume", "--help"])
            .output()
            .unwrap();
        assert!(help.status.success());
        let help = String::from_utf8(help.stdout).unwrap();
        for flag in ["--account", "--profile", "--list", "--details"] {
            assert!(help.contains(flag));
        }
    }
}

#[test]
fn list_is_exact_directory_and_includes_disabled_dodex() {
    let fixture = Fixture::new();
    let output = fixture.run(&["resume", "--list"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains(SESSION), "{output}");
    assert!(output.contains(DODEX_SESSION), "{output}");
    assert!(!output.contains(OTHER_SESSION), "{output}");
}

#[test]
fn unsupported_runtime_details_are_read_only_and_do_not_print_config_secrets() {
    let fixture = Fixture::new();
    let config = fixture.home.join(".codex/config.toml");
    let sentinel = "private-value-must-never-be-printed";
    fs::write(&config, format!("model = 'gpt-test'\n[mcp_servers.test]\ncommand = 'example'\n[mcp_servers.test.env]\nSECRET = '{sentinel}'\n")).unwrap();
    let output = fixture.run(&["resume", SESSION, "--account", "codex", "--details"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("Disabled:"), "{output}");
    assert!(!output.contains(sentinel), "{output}");
    assert!(!fixture.home.join(".codex/auth.json").exists());
    assert!(fs::read_to_string(config).unwrap().contains(sentinel));
}

#[test]
fn malformed_config_is_a_visible_disabled_combination() {
    let fixture = Fixture::new();
    fs::write(
        fixture.home.join(".codex/config.toml"),
        "secret = 'private-malformed-value",
    )
    .unwrap();
    let output = fixture.run(&["resume", SESSION, "--account", "codex", "--details"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("Disabled:"));
    assert!(!output.contains("private-malformed-value"));
}

#[test]
fn redirected_input_does_not_silently_choose_session_or_account() {
    let fixture = Fixture::new();
    let output = fixture.run(&["resume"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Choose a session in a terminal"));
    let output = fixture.run(&["resume", SESSION]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Choose a quota account in a terminal")
    );
}
