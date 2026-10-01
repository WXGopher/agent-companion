#!/usr/bin/env python3
"""Exercise the actual resume TUI in a PTY using only synthetic session homes."""
import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import select
import signal
import struct
import sys
import tempfile
import termios
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, required=True)
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
with tempfile.TemporaryDirectory(prefix="acomp-resume-tui-") as temporary:
    root = Path(temporary).resolve()
    home = root / "user"
    project = root / "project"
    sessions = home / ".codex/sessions/2026/01/01"
    sessions.mkdir(parents=True)
    project.mkdir()
    session = "00000000-0000-4000-8000-000000000001"
    rows = [
        {"timestamp": "2026-01-01T00:00:00Z", "type": "session_meta", "payload": {
            "id": session, "cwd": str(project), "originator": "codex_cli_rs", "source": "cli", "cli_version": "0.0.0-unverified"}},
        {"timestamp": "2026-01-01T00:00:01Z", "type": "event_msg", "payload": {
            "type": "user_message", "message": "original work 中文"}},
    ]
    (sessions / f"rollout-2026-01-01T00-00-00-{session}.jsonl").write_text(
        "\n".join(json.dumps(row) for row in rows) + "\n"
    )
    child, master = os.forkpty()
    if child == 0:
        os.chdir(project)
        environment = dict(os.environ, HOME=str(home), USERPROFILE=str(home), TERM="xterm-256color")
        environment.pop("CODEX_HOME", None)
        os.execve(binary, [str(binary), "resume"], environment)
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 110, 0, 0))
    transcript = bytearray()
    exited = False

    def drain_pending(timeout=0.05):
        if select.select([master], [], [], timeout)[0]:
            try:
                transcript.extend(os.read(master, 65536))
            except OSError as error:
                if error.errno != errno.EIO:
                    raise

    def expect(value, timeout=20):
        expected = value.encode()
        start = len(transcript)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.2)
            if ready:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    break
                if not data:
                    break
                transcript.extend(data)
                if expected in transcript[start:]:
                    return
        raise AssertionError(f"TUI did not display {value!r}:\n{transcript.decode(errors='replace')}")

    try:
        expect("original session")
        os.write(master, b"no-such-session")
        expect("No matching items")
        os.write(master, b"\x7f" * len("no-such-session"))
        expect("History storage")
        os.write(master, b"\r")
        expect("this run's quota account")
        os.write(master, b"\t")
        expect("Details")
        os.write(master, b"\r")
        expect("Disabled:")
        assert os.waitpid(child, os.WNOHANG) == (0, 0), "Disabled account must not launch or exit"
        exit_output_start = len(transcript)
        os.write(master, b"\x1b")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            # Continue acting as a terminal while waiting. Otherwise queued
            # redraws fill the PTY and macOS can block exit draining its output,
            # even after SIGKILL. A real terminal keeps consuming these bytes.
            drain_pending()
            waited, status = os.waitpid(child, os.WNOHANG)
            if waited:
                exited = True
                assert os.waitstatus_to_exitcode(status) == 0, status
                break
        assert exited, "Escape must cancel and restore the terminal"
        drain_pending(0)
        assert b"\x1b[?1049l" in transcript[exit_output_start:], "Escape must leave the alternate screen"
        print("Resume TUI PTY: search, source/account screens, details, disabled launch, and cancellation passed.")
    finally:
        if not exited:
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass
            # A host can leave a killed process stuck in kernel exit. Preserve
            # the failed acceptance result without hanging the entire CI job.
            cleanup_deadline = time.monotonic() + 2
            while time.monotonic() < cleanup_deadline:
                try:
                    waited, _ = os.waitpid(child, os.WNOHANG)
                except ChildProcessError:
                    break
                if waited:
                    break
                drain_pending()
            else:
                print(f"TUI child {child} could not be reaped after SIGKILL", file=sys.stderr)
        os.close(master)
