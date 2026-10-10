//! End-to-end tests over the real binaries: a headless `agent-companion` on one side of a
//! named pipe, `agent-companion-hook` on the other, with synthetic Codex payloads.
//!
//! Each test binds its own randomly named pipe through `AGENT_COMPANION_PIPE_NAME`, so
//! they neither collide with each other nor with an Agent Companion the developer happens
//! to be running.

#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long to wait for a line the server is expected to print promptly.
const LINE_TIMEOUT: Duration = Duration::from_secs(15);

fn agent_companion_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-companion"))
}

/// `agent-companion-hook` lands in the same target directory as `agent-companion`, but it belongs to
/// another package so there is no `CARGO_BIN_EXE_` for it.
fn hook_exe() -> PathBuf {
    let path = agent_companion_exe()
        .parent()
        .expect("the test binary has a parent directory")
        .join(format!(
            "agent-companion-hook{}",
            std::env::consts::EXE_SUFFIX
        ));

    if !path.exists() {
        // `cargo test --workspace` builds it for us; `cargo test -p agent-companion` does
        // not, so cover that case rather than failing confusingly.
        let built = Command::new(env!("CARGO"))
            .args(["build", "-p", "agent-companion-hook"])
            .status();
        assert!(
            built.is_ok_and(|status| status.success()) && path.exists(),
            "could not find or build {}",
            path.display()
        );
    }
    path
}

/// A pipe name no other test or process is using.
fn unique_pipe_name(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "agent-companion-test-{label}-{}-{nanos}",
        std::process::id()
    )
}

/// A headless `agent-companion`, killed when the test drops it.
struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(pipe_name: &str, extra_args: &[&str]) -> Self {
        let mut child = Command::new(agent_companion_exe())
            .arg("headless")
            .args(extra_args)
            .env("AGENT_COMPANION_PIPE_NAME", pipe_name)
            // Never let an ambient skip flag disable the thing under test.
            .env_remove("AGENT_COMPANION_SKIP_HOOKS")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .spawn()
            .expect("failed to spawn agent-companion");

        let stdout = child.stdout.take().expect("piped stdout");
        let lines = spawn_line_reader(stdout);
        let server = Server { child, lines };

        let ready = server.next_line();
        assert!(
            ready.contains("listening on") && ready.contains(pipe_name),
            "unexpected readiness line: {ready}"
        );
        server
    }

    fn next_line(&self) -> String {
        self.lines
            .recv_timeout(LINE_TIMEOUT)
            .expect("timed out waiting for a line from agent-companion")
    }

    /// Read lines until one contains `needle`, returning everything consumed.
    fn wait_for(&self, needle: &str) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            let line = self.next_line();
            let found = line.contains(needle);
            seen.push(line);
            if found {
                return seen;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Move blocking reads off the test thread so a hung server times out instead
/// of hanging the suite.
fn spawn_line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

struct HookRun {
    stdout: String,
    success: bool,
    elapsed: Duration,
}

/// Feed `payload` to `agent-companion-hook` on stdin and collect what it produced.
fn run_hook(pipe_name: &str, payload: &str, skip_hooks: bool) -> HookRun {
    let mut command = Command::new(hook_exe());
    command
        .args(["--source", "codex"])
        .env("AGENT_COMPANION_PIPE_NAME", pipe_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if skip_hooks {
        command.env("AGENT_COMPANION_SKIP_HOOKS", "1");
    } else {
        command.env_remove("AGENT_COMPANION_SKIP_HOOKS");
    }

    let started = Instant::now();
    let mut child = command
        .spawn()
        .expect("failed to spawn agent-companion-hook");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .expect("failed to write the payload");

    let output = child
        .wait_with_output()
        .expect("failed to wait for the hook");
    HookRun {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        success: output.status.success(),
        elapsed: started.elapsed(),
    }
}

fn pre_tool_use_payload() -> &'static str {
    r#"{"hook_event_name":"PreToolUse","session_id":"abc12345-0000-4000-8000-000000000001","transcript_path":"C:\\synthetic\\transcript.jsonl","cwd":"C:\\synthetic\\project","tool_name":"Bash","tool_input":{"command":"git status"},"permission_mode":"default"}"#
}

fn permission_request_payload() -> &'static str {
    r#"{"hook_event_name":"PermissionRequest","session_id":"abc12345-0000-4000-8000-000000000001","transcript_path":"C:\\synthetic\\transcript.jsonl","cwd":"C:\\synthetic\\project","tool_name":"Bash","tool_input":{"command":"git status"},"tool_use_id":"tu-1"}"#
}

#[test]
fn permission_request_gets_an_allow_decision() {
    let pipe_name = unique_pipe_name("allow");
    let server = Server::start(&pipe_name, &["--auto-allow"]);
    assert!(server.next_line().contains("auto-allowed"));

    let run = run_hook(&pipe_name, permission_request_payload(), false);
    assert!(run.success, "the hook must always exit 0");

    let decision: serde_json::Value =
        serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
            panic!("stdout was not JSON ({error}): {:?}", run.stdout);
        });

    assert!(decision.get("suppressOutput").is_none());
    let specific = &decision["hookSpecificOutput"];
    assert_eq!(specific["hookEventName"], "PermissionRequest");
    assert_eq!(specific["decision"]["behavior"], "allow");
    assert!(run.stdout.ends_with('\n'), "output must end in a newline");

    // The server saw the event and said so.
    let log = server.wait_for("auto-allowed");
    assert!(
        log.iter()
            .any(|line| line.contains("PermissionRequest") && line.contains("abc12345")),
        "expected the event in the log, got {log:?}"
    );
}

#[test]
fn installed_codex_command_handles_shell_characters_and_forwards_stdin() {
    let pipe_name = unique_pipe_name("codex-installed");
    let server = Server::start(&pipe_name, &["--auto-allow"]);
    server.next_line();
    let dir = tempfile::tempdir().unwrap();
    let binary_dir = dir.path().join("a space & dollar$ and 'quote'");
    std::fs::create_dir(&binary_dir).unwrap();
    let binary = binary_dir.join("agent-companion-hook.exe");
    std::fs::copy(hook_exe(), &binary).unwrap();
    let home = dir.path().join("codex");
    agent_companion_core::install::install_codex(&home, &binary).unwrap();
    let hooks: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("hooks.json")).unwrap()).unwrap();
    let command = hooks["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = Command::new("cmd.exe")
        .args(["/d", "/s", "/c", command])
        .env("AGENT_COMPANION_PIPE_NAME", &pipe_name)
        .env_remove("AGENT_COMPANION_SKIP_HOOKS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(permission_request_payload().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        decision["hookSpecificOutput"]["decision"],
        serde_json::json!({"behavior":"allow"})
    );
    assert!(
        server
            .wait_for("auto-allowed")
            .iter()
            .any(|line| line.contains("codex"))
    );
}

/// `PreToolUse` fires before **every** tool call, including everything the
/// user's own settings already allow, and its hook waits 45 s for an answer.
/// Deciding one would mean deciding hundreds of times a session on the user's
/// behalf — so no mode does, and this is the guard that none ever will.
#[test]
fn pre_tool_use_is_never_decided_even_under_auto_allow() {
    for flags in [
        &["--auto-allow"][..],
        &["--auto-allow", "--pre-tool-use=ack"][..],
        &[][..],
    ] {
        let pipe_name = unique_pipe_name("never");
        let server = Server::start(&pipe_name, flags);
        let _banner = server.next_line();

        let run = run_hook(&pipe_name, pre_tool_use_payload(), false);
        assert!(run.success, "the hook must always exit 0");
        assert!(
            run.stdout.is_empty(),
            "{flags:?} decided a PreToolUse: {:?}",
            run.stdout
        );
        assert!(
            run.elapsed < Duration::from_secs(5),
            "{flags:?} left the tool call stalling for {:?}",
            run.elapsed
        );
    }
}

#[test]
fn hook_fails_open_when_no_app_is_running() {
    // Nothing is bound to this name, which is exactly the point.
    let pipe_name = unique_pipe_name("no-server");

    let run = run_hook(&pipe_name, pre_tool_use_payload(), false);

    assert!(run.success, "a missing app must still exit 0");
    assert!(
        run.stdout.is_empty(),
        "a missing app must produce no stdout, got {:?}",
        run.stdout
    );
    // The budget that matters is the hook's own 45 s, not a stopwatch: the
    // point is that a missing app costs a moment, not a turn. Two seconds is
    // loose enough to survive a loaded test machine and still prove that.
    assert!(
        run.elapsed < Duration::from_secs(2),
        "failing open took {:?}; it must not stall the agent",
        run.elapsed
    );
}

#[test]
fn non_blocking_events_do_not_wait_for_a_decision() {
    let pipe_name = unique_pipe_name("session-start");
    let server = Server::start(&pipe_name, &["--auto-allow"]);
    assert!(server.next_line().contains("auto-allowed"));

    let payload = r#"{"hook_event_name":"SessionStart","session_id":"beef0001-0000-4000-8000-000000000002","cwd":"C:\\synthetic\\project","source":"startup"}"#;
    let run = run_hook(&pipe_name, payload, false);

    assert!(run.success);
    assert!(
        run.stdout.is_empty(),
        "SessionStart takes no decision, got {:?}",
        run.stdout
    );
    assert!(
        run.elapsed < Duration::from_secs(2),
        "SessionStart should send and leave, took {:?}",
        run.elapsed
    );

    let line = server.next_line();
    assert!(
        line.contains("SessionStart") && line.contains("beef0001"),
        "unexpected log line: {line}"
    );
}

#[test]
fn skip_hooks_env_short_circuits_everything() {
    let pipe_name = unique_pipe_name("skip");
    let server = Server::start(&pipe_name, &["--auto-allow"]);
    assert!(server.next_line().contains("auto-allowed"));

    let run = run_hook(&pipe_name, pre_tool_use_payload(), true);

    assert!(run.success);
    assert!(run.stdout.is_empty(), "got {:?}", run.stdout);
    assert!(
        run.elapsed < Duration::from_secs(1),
        "skipping took {:?}",
        run.elapsed
    );

    // And the server never heard about it. Prove that by sending a second event
    // with hooks enabled: the first line the server logs is that one.
    let payload = r#"{"hook_event_name":"Stop","session_id":"cafe0002-0000-4000-8000-000000000003","cwd":"C:\\synthetic\\project"}"#;
    run_hook(&pipe_name, payload, false);
    let line = server.next_line();
    assert!(
        line.contains("Stop") && line.contains("cafe0002"),
        "the skipped event should never have reached the server; got {line}"
    );
}

/// One Agent Companion at a time: starting a second one evicts the first rather than
/// failing, because the pipe *is* Agent Companion's identity and two of them would mean
/// two readouts, one of which receives nothing.
///
/// Headless stands in for the app here, which is the point of it standing down
/// the same way: the takeover is the same code path on both.
#[test]
fn a_second_agent_companion_takes_the_pipe_from_the_first() {
    let pipe_name = unique_pipe_name("takeover");
    let mut first = Server::start(&pipe_name, &[]);
    assert!(first.next_line().contains("observing only"));

    let second = Server::start(&pipe_name, &[]);
    assert!(second.next_line().contains("it stood down"));
    assert!(second.next_line().contains("observing only"));

    // The incumbent said so and left.
    let farewell = first.wait_for("stood down");
    assert!(
        farewell
            .last()
            .is_some_and(|line| line.contains("stood down")),
        "expected a stand-down line, got {farewell:?}"
    );
    let status = first
        .child
        .wait()
        .expect("the evicted server should have exited");
    assert!(
        status.success(),
        "standing down is not a failure: {status:?}"
    );

    // And the survivor really owns the pipe: a hook fired now reaches it.
    let payload = r#"{"hook_event_name":"Stop","session_id":"cafe0003-0000-4000-8000-000000000001","cwd":"C:\\synthetic\\project"}"#;
    run_hook(&pipe_name, payload, false);
    let line = second.next_line();
    assert!(
        line.contains("Stop") && line.contains("cafe0003"),
        "the new server should be the one serving; got {line}"
    );
}

/// Without `--auto-allow`, the event a human would actually be asked about is
/// held and the agent falls back to its own prompt.
#[test]
fn a_permission_request_is_held_without_auto_allow() {
    let pipe_name = unique_pipe_name("hold");
    let server = Server::start(&pipe_name, &[]);
    assert!(server.next_line().contains("observing only"));

    // A held PermissionRequest blocks its hook for a day, so run it detached
    // and check the server's side of the story instead.
    let mut child = Command::new(hook_exe())
        .args(["--source", "codex"])
        .env("AGENT_COMPANION_PIPE_NAME", &pipe_name)
        .env_remove("AGENT_COMPANION_SKIP_HOOKS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn agent-companion-hook");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(permission_request_payload().as_bytes())
        .expect("failed to write the payload");

    let log = server.wait_for("holding");
    assert!(
        log.iter()
            .any(|line| line.contains("waiting for a decision")),
        "expected the approval prompt, got {log:?}"
    );
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the hook should still be blocked, not finished"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn acking_pre_tool_use_releases_the_hook_without_a_decision() {
    // The M2 experiment shape: PreToolUse steps aside so the session reaches
    // PermissionRequest, where the reply schema can actually be tested.
    let pipe_name = unique_pipe_name("ack");
    let server = Server::start(&pipe_name, &["--auto-allow"]);
    assert!(server.next_line().contains("PreToolUse=ack"));

    let run = run_hook(&pipe_name, pre_tool_use_payload(), false);

    assert!(run.success);
    assert!(
        run.stdout.is_empty(),
        "an ack must decide nothing, got {:?}",
        run.stdout
    );
    assert!(
        run.elapsed < Duration::from_secs(5),
        "an ack must release the hook at once, took {:?}",
        run.elapsed
    );
    assert!(
        server
            .wait_for("acked")
            .iter()
            .any(|line| line.contains("PreToolUse")),
        "the server should log the ack"
    );
}

/// The flat `PermissionRequest` shape — `"decision": "allow"` with a sibling
/// `reason` — reads as a **denial** to the hook protocol, so the
/// 2026-08-23 verdict removed it from the codebase. This is the guard against
/// it coming back: there is no flag that can ask for it any more.
#[test]
fn the_flat_permission_request_shape_cannot_be_asked_for() {
    let pipe_name = unique_pipe_name("flat");
    let refused = Command::new(agent_companion_exe())
        .args(["headless", "--auto-allow", "--permission-request=string"])
        .env("AGENT_COMPANION_PIPE_NAME", &pipe_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn agent-companion");

    assert!(
        !refused.status.success(),
        "--permission-request must no longer exist"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("--permission-request"),
        "expected clap to reject the removed flag, got {stderr:?}"
    );
}

/// Holding a `PreToolUse` is a diagnostic, not a default: it is what you ask for
/// when you want to watch a hook block, and asking for it is the only way to get
/// it.
#[test]
fn holding_pre_tool_use_is_available_on_request() {
    let pipe_name = unique_pipe_name("hold-pre");
    let server = Server::start(&pipe_name, &["--pre-tool-use=hold"]);
    assert!(server.next_line().contains("PreToolUse=hold"));

    let mut child = Command::new(hook_exe())
        .args(["--source", "codex"])
        .env("AGENT_COMPANION_PIPE_NAME", &pipe_name)
        .env_remove("AGENT_COMPANION_SKIP_HOOKS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn agent-companion-hook");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(pre_tool_use_payload().as_bytes())
        .expect("failed to write the payload");

    server.wait_for("holding");
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "the hook should still be blocked, not finished"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// There is no `--pre-tool-use=allow` any more, and there must never be one
/// again: it decided, on the user's behalf, every tool call a session made.
#[test]
fn pre_tool_use_cannot_be_told_to_approve() {
    let pipe_name = unique_pipe_name("gate");
    let refused = Command::new(agent_companion_exe())
        .args(["headless", "--auto-allow", "--pre-tool-use=allow"])
        .env("AGENT_COMPANION_PIPE_NAME", &pipe_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn agent-companion");

    assert!(
        !refused.status.success(),
        "`allow` must not be a PreToolUse mode"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("allow"),
        "expected clap to reject the value, got {stderr:?}"
    );
}

#[test]
fn the_session_table_summary_follows_the_event_stream() {
    let pipe_name = unique_pipe_name("summary");
    let server = Server::start(&pipe_name, &[]);
    assert!(server.next_line().contains("observing only"));

    let start = r#"{"hook_event_name":"SessionStart","session_id":"5umm0001-0000-4000-8000-000000000006","cwd":"C:\\synthetic\\project","source":"startup"}"#;
    run_hook(&pipe_name, start, false);

    let summary = server.wait_for("session");
    let line = summary
        .iter()
        .find(|line| line.contains("=="))
        .unwrap_or_else(|| panic!("expected a table summary, got {summary:?}"));
    assert!(
        line.contains("1 session: 1 running, 0 waiting"),
        "unexpected summary: {line}"
    );

    // Stop moves the session out of `running`, which is a change worth printing.
    let stop = r#"{"hook_event_name":"Stop","session_id":"5umm0001-0000-4000-8000-000000000006","cwd":"C:\\synthetic\\project"}"#;
    run_hook(&pipe_name, stop, false);

    let after = server.wait_for("done");
    assert!(
        after
            .iter()
            .any(|line| line.contains("0 running") && line.contains("1 done")),
        "expected the completed session in the summary, got {after:?}"
    );
}
