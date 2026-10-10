//! Exercise the GUI lifecycle through its pipe, with isolated data and no
//! taskbar readout. No real agent, credential, hook setting or terminal is used.
#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Gui(Child);

impl Gui {
    fn start(root: &Path, pipe: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_agent-companion"))
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .env("AGENT_COMPANION_CONFIG_DIR", root)
            .env("AGENT_COMPANION_PIPE_NAME", pipe)
            .env("USERPROFILE", root)
            .env("HOME", root)
            .env("APPDATA", root)
            .env("LOCALAPPDATA", root)
            .env("CODEX_HOME", root.join(".codex"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let gui = Self(child);
        // A harmless hello also waits until the isolated pipe is listening.
        send(
            pipe,
            &[json!({"type":"hello", "hello":{"client":"display-test"}})],
        );
        gui
    }

    fn wait_for_exit(&mut self) {
        eventually(|| {
            self.0.try_wait().unwrap().map(|status| {
                assert!(status.success());
            })
        });
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn eventually<T>(mut read: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Agent Companion"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn send(pipe: &str, frames: &[Value]) {
    let mut connection = eventually(|| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(agent_companion_core::pipe::pipe_path(pipe))
            .ok()
    });
    for frame in frames {
        writeln!(connection, "{frame}").unwrap();
    }
    connection.flush().unwrap();
}

fn hook(source: &str, event: &str) -> Value {
    json!({"type":"command", "command": {
        "type":"processHook", "source":source,
        "hook":{"hook_event_name":event, "session_id":source, "cwd":"display-test"}
    }})
}

fn snapshot(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
#[ignore = "runs isolated Slint GUI processes; requires a Windows desktop"]
fn hooks_control_visibility_and_restarts_preserve_the_last_display() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("display.json");
    let now = agent_companion_core::now_unix_secs();
    let pipe = format!("agent-companion-display-test-{}-{now}", std::process::id());
    fs::write(
        root.join("config.json"),
        r#"{"taskbar":{"enabled":false},"completionNotifications":false}"#,
    )
    .unwrap();
    let previous = json!({
        "codex":{"visible":true,"lastSeen":now-3600},
        "sessions":[{"id":"saved","title":"previous project","detail":"Done","phase":"completed","source":"codex"}],
        "updatedAt":now-3600
    });
    let saved = serde_json::to_vec(&previous).unwrap();
    fs::write(&path, &saved).unwrap();
    let mut gui = Gui::start(root, &pipe);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        fs::read(&path).unwrap(),
        saved,
        "startup leaves saved task text alone"
    );
    send(&pipe, &[hook("codex", "UserPromptSubmit")]);
    eventually(|| {
        let state = snapshot(&path);
        (state["codex"]["visible"] == true
            && state["sessions"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1 && rows[0]["id"] == "codex"))
        .then_some(())
    });
    assert!(snapshot(&path).get("usage").is_none());
    let shutdown = json!({"type":"event","event":{"type":"shutdown"}});
    send(&pipe, std::slice::from_ref(&shutdown));
    gui.wait_for_exit();
    let mut state = snapshot(&path);
    state["codex"]["lastSeen"] = json!(now - 3600);
    let saved = serde_json::to_vec(&state).unwrap();
    fs::write(&path, &saved).unwrap();
    let mut restarted = Gui::start(root, &pipe);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(fs::read(&path).unwrap(), saved);
    send(&pipe, &[hook("codex", "SessionEnd"), shutdown]);
    restarted.wait_for_exit();
    assert_eq!(snapshot(&path)["codex"]["visible"], false);
    assert!(snapshot(&path)["sessions"].as_array().unwrap().is_empty());
}
