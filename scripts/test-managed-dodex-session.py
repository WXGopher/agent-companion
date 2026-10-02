#!/usr/bin/env python3
"""Opt-in native session regression, with synthetic data and macOS network denial.

Usage: python3 scripts/test-managed-dodex-session.py --codex /path/to/bin/codex
No login, model turn, installation or real account files are used.
"""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


def verify(executable, root, companion):
    package = executable.resolve().parent.parent
    manifest = json.loads((package / "codex-package.json").read_text())
    assert manifest["entrypoint"] == "bin/codex"
    profile, sqlite, desktop = root / "existing-second", root / "existing-second/sqlite", root / "desktop"
    workspace = root / "workspace"
    for path in (profile, sqlite, desktop / "logs", workspace, root / "user"):
        path.mkdir(parents=True, exist_ok=True)
    (profile / "config.toml").write_text('cli_auth_credentials_store = "file"\n'
        + "sqlite_home = " + json.dumps(str(sqlite)) + "\n"
        + "log_dir = " + json.dumps(str(desktop / "logs")) + "\n")
    binding = dict(package=str(package), entry=str(root / "dodex"), app=str(root / "Dodex.app"),
                   profile_home=str(profile), sqlite_home=str(sqlite), desktop_data=str(desktop),
                   log_dir=str(desktop / "logs"), original=str(root / "unused-adapter"),
                   companion=str(companion.resolve()), version=manifest["version"])
    template = (Path(__file__).resolve().parents[1] / "crates/agent-companion/src/software_updates/dodex-wrapper.py").read_text()
    serialized = json.dumps(binding, separators=(",", ":"), ensure_ascii=False)
    wrapper = root / "dodex"
    wrapper.write_text(template.replace("# __BINDING_MARKER__", "# agent-companion-dodex: " + serialized)
                       .replace("__BINDING_JSON__", json.dumps(serialized)))
    wrapper.chmod(0o700)
    thread = "22222222-2222-4222-8222-222222222222"
    turn = "33333333-3333-4333-8333-333333333333"
    rollout = profile / "sessions/2026/10/02" / ("rollout-2026-10-02T00-00-00-" + thread + ".jsonl")
    rollout.parent.mkdir(parents=True)
    answer = "Synthetic assistant reply retained across the terminal runtime upgrade."
    def event(kind, payload, second):
        return dict(timestamp=f"2026-10-02T00:00:{second:02d}.000Z", type=kind, payload=payload)
    records = [
        event("session_meta", dict(id=thread, timestamp="2026-10-02T00:00:00.000Z", cwd=str(workspace),
              originator="codex_cli_rs", cli_version="0.155.0", source="cli", model_provider="openai", base_instructions={"text": ""}), 0),
        event("event_msg", dict(type="task_started", turn_id=turn, model_context_window=272000), 1),
        event("event_msg", dict(type="user_message", message="Synthetic history. No model call.", images=[], local_images=[], text_elements=[]), 2),
        event("response_item", dict(type="message", role="user", content=[dict(type="input_text", text="Synthetic history. No model call.")]), 2),
        event("response_item", dict(type="message", role="assistant", phase="final_answer", content=[dict(type="output_text", text=answer)]), 3),
        event("event_msg", dict(type="agent_message", message=answer, phase="final_answer"), 3),
        event("event_msg", dict(type="task_complete", turn_id=turn, last_agent_message=answer), 4),
    ]
    rollout.write_text("".join(json.dumps(record) + "\n" for record in records))
    real_home = Path.home()
    sandbox = '(version 1) (allow default) (deny network*)'
    # The installed package is a read-only test input, never a profile. Exclude
    # the rest of the real primary home, and all known secondary/App homes.
    sandbox += '(deny file-read* file-write* (require-all (subpath ' + json.dumps(str(real_home / ".codex")) + ') (require-not (subpath ' + json.dumps(str(package)) + '))))'
    sandbox += '(deny file-write* (subpath ' + json.dumps(str(package)) + '))'
    for path in (real_home / ".codex-second", real_home / "Library/Application Support/Codex", real_home / "Library/Application Support/Codex-B"):
        sandbox += '(deny file-read* file-write* (subpath ' + json.dumps(str(path)) + '))'
    environment = dict(HOME=str(root / "user"), PATH="/usr/bin:/bin:/usr/sbin:/sbin", TMPDIR=str(root), LANG="en_US.UTF-8",
                       CODEX_HOME="/synthetic-wrong-primary", CODEX_SQLITE_HOME="/synthetic-wrong-primary/sqlite")
    args = ["/usr/bin/sandbox-exec", "-p", sandbox, str(wrapper), "-c", "analytics.enabled=false", "-c", "feedback.enabled=false", "app-server", "--stdio"]
    inbox = queue.Queue()
    with (root / "native.stderr").open("w") as stderr:
        process = subprocess.Popen(args, cwd=workspace, env=environment, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=stderr, text=True, start_new_session=True)
        def receive():
            for line in process.stdout:
                try:
                    inbox.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
        threading.Thread(target=receive, daemon=True).start()
        def send(message):
            process.stdin.write(json.dumps(message) + "\n")
            process.stdin.flush()
        def request(identifier, method, params):
            send(dict(id=identifier, method=method, params=params))
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                try:
                    message = inbox.get(timeout=0.5)
                except queue.Empty:
                    if process.poll() is not None:
                        raise RuntimeError("Native fixture exited; see " + str(root / "native.stderr"))
                    continue
                if message.get("id") == identifier:
                    assert "result" in message, message
                    return message["result"]
            raise TimeoutError(method)
        try:
            request(1, "initialize", dict(clientInfo=dict(name="companion_native_fixture", version="0.1"),
                                         capabilities=dict(experimentalApi=True, explicitGatewayOauth=True)))
            send(dict(method="initialized"))
            listing = request(2, "thread/list", dict(limit=10, modelProviders=[], sourceKinds=["cli"]))
            assert any(item["id"] == thread for item in listing["data"])
            reading = request(3, "thread/read", dict(threadId=thread, includeTurns=True))
            resuming = request(4, "thread/resume", dict(threadId=thread, cwd=str(workspace), approvalPolicy="never", sandbox="read-only"))
            for result in (reading, resuming):
                assert result["thread"]["id"] == thread
                assert answer in json.dumps(result)
            assert resuming["thread"]["path"] == str(rollout)
            assert not (root / "user/.codex/sessions").exists()
            print(f"PASS: native {manifest['version']} through the managed wrapper listed, read and resumed the original synthetic session; no model turn or network access.")
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
            process.stdout.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", required=True, type=Path)
    parser.add_argument("--companion", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/agent-companion")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="companion-native-session-") as temporary:
        verify(args.codex, Path(temporary).resolve(), args.companion)
