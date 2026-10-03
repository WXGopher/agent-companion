"""Opt-in native Codex contract tests using disposable state and a loopback server.

Run with ACOMP_TEST_CODEX_BINARY=/absolute/path/to/codex python3 -m unittest
discover -s scripts/tests -p test_resume_native.py -v. The expected version is
0.159.3; upgrades must pass these contracts before changing the runtime gate.
No real credentials are read, and all model/auth responses are deterministic
fixtures. These tests establish native persistence/auth plumbing, not billing.
The shared-usage contract also accepts the legacy bundled 0.155.0-alpha.16.4;
select that one test explicitly when checking that runtime.
Set ACOMP_TEST_LEGACY_CODEX_BINARY to a native 0.155.0-alpha.16.4 executable
to also verify that its history can be resumed by 0.159.3 and reopened by 0.155.
Additionally set ACOMP_TEST_ACOMP_BINARY to an absolute production acomp binary
on macOS for its denied-preflight cleanup smoke (requires sandbox-exec).
"""

import base64
from contextlib import closing
import datetime
import http.server
import json
import os
from pathlib import Path
import queue
import shlex
import sqlite3
import subprocess
import struct
import sys
import tempfile
import threading
import time
import unittest
import zlib


NATIVE = os.environ.get("ACOMP_TEST_CODEX_BINARY")
LEGACY_NATIVE = os.environ.get("ACOMP_TEST_LEGACY_CODEX_BINARY")
ACOMP = os.environ.get("ACOMP_TEST_ACOMP_BINARY")
VERSION = "codex-cli 0.159.3"


def verify_native_executable(value, variable):
    path = Path(value)
    if not path.is_absolute():
        raise ValueError(variable + " must be an absolute native executable path")
    with path.open("rb") as executable:
        magic = executable.read(4)
    if magic not in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\x7fELF") and magic[:2] != b"MZ":
        raise ValueError("Use the native binary: wrappers may reset the disposable CODEX_HOME")


def fake_token(account, marker="initial"):
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")

    return ".".join([
        encode({"alg": "none"}),
        encode({
            "exp": int(time.time()) + 86400,
            "email": account + "@example.invalid",
            "fixture": marker,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account,
                "chatgpt_user_id": account + "-user",
                "chatgpt_plan_type": "plus",
            },
        }),
        "not-a-real-signature",
    ])


def fake_auth(account):
    token = fake_token(account)
    return {
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": token,
            "access_token": token,
            "refresh_token": "fixture-refresh-" + account,
            "account_id": account,
        },
        "last_refresh": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }


def directory_link(target, link):
    if os.name == "nt":
        # Junctions require no elevation and work on the same disposable volume.
        # Keep paths out of command text: user/temp paths can contain &, %, !,
        # spaces or parentheses, which cmd.exe would otherwise reinterpret.
        environment = os.environ.copy()
        environment["ACOMP_NATIVE_JUNCTION_LINK"] = str(link)
        environment["ACOMP_NATIVE_JUNCTION_TARGET"] = str(target)
        powershell = (Path(os.environ["SystemRoot"]) / "System32" / "WindowsPowerShell" /
                      "v1.0" / "powershell.exe")
        result = subprocess.run(
            [str(powershell), "-NoProfile", "-NonInteractive", "-Command",
             "New-Item -ItemType Junction -Path $env:ACOMP_NATIVE_JUNCTION_LINK "
             "-Value $env:ACOMP_NATIVE_JUNCTION_TARGET -ErrorAction Stop | Out-Null"],
            capture_output=True, text=True, check=False, env=environment,
        )
        if result.returncode:
            raise RuntimeError(result.stderr)
    else:
        link.symlink_to(target, target_is_directory=True)


def file_link(target, link):
    if os.name == "nt":
        os.link(target, link)
    else:
        link.symlink_to(target)


class LoopbackServer:
    def __init__(self):
        self.requests = []
        self.responses = []
        self.refresh_account = "selected-account"
        self.started = threading.Event()
        self.release = threading.Event()
        self.hold_next_response = False
        self.hold_usage_history = False
        self.usage_history_started = threading.Event()
        self.usage_history_release = threading.Event()
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_CONNECT(self):
                # The production-acomp negative smoke uses this as a denying
                # proxy. Never tunnel fixture credentials to the public service.
                fixture.requests.append((self.path, dict(self.headers), None))
                self.send_error(403, "External network is disabled in native contract tests")

            def do_GET(self):
                fixture.requests.append((self.path, dict(self.headers), None))
                if "accounts/check" in self.path:
                    self.reply({"accounts": [{
                        "id": account,
                        "workspace_backend_origin": "https://chatgpt.com",
                        "account_routing_override": "NO_CONSTRAINT",
                    } for account in {"source-account", "selected-account", fixture.refresh_account}]})
                elif "models" in self.path:
                    self.reply({"models": []})
                elif self.path.endswith("/wham/usage"):
                    self.reply({"plan_type": "plus", "rate_limit": {
                        "allowed": True, "limit_reached": False,
                        "primary_window": {"used_percent": 23, "limit_window_seconds": 18000,
                                           "reset_after_seconds": 3600, "reset_at": 1900000000},
                        "secondary_window": {"used_percent": 61, "limit_window_seconds": 604800,
                                             "reset_after_seconds": 7200, "reset_at": 1900003600},
                    }})
                elif self.path.endswith("/wham/profiles/me"):
                    fixture.usage_history_started.set()
                    if fixture.hold_usage_history:
                        fixture.usage_history_release.wait(timeout=10)
                    self.reply({"stats": {"lifetime_tokens": 1000, "daily_usage_buckets": []}})
                else:
                    self.reply({})

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                try:
                    body = json.loads(raw)
                except (ValueError, UnicodeError):
                    body = {"unparsed": raw.decode("utf-8", errors="replace")}
                fixture.requests.append((self.path, dict(self.headers), body))
                if self.path == "/oauth/token":
                    token = fake_token(fixture.refresh_account, "refreshed")
                    self.reply({"id_token": token, "access_token": token,
                                "refresh_token": "fixture-rotated-refresh"})
                    return
                if not self.path.endswith("/responses"):
                    self.reply({})
                    return
                if fixture.hold_next_response:
                    fixture.hold_next_response = False
                    fixture.started.set()
                    fixture.release.wait(timeout=30)
                index = len(fixture.model_requests())
                items = (fixture.responses.pop(0) if fixture.responses else [{
                    "type": "message", "role": "assistant", "id": "message-" + str(index),
                    "content": [{"type": "output_text", "text": "fixture-answer-" + str(index)}],
                }])
                events = [{"type": "response.created", "response": {"id": "response-" + str(index)}}]
                events.extend({"type": "response.output_item.done", "item": item} for item in items)
                events.append({"type": "response.completed", "response": {
                    "id": "response-" + str(index),
                    "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12},
                }})
                data = "".join("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n"
                               for event in events).encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def reply(self, value):
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = "http://127.0.0.1:" + str(self.server.server_port)

    def model_requests(self):
        return [request for request in self.requests if request[0].endswith("/responses")]

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class AppServer:
    def __init__(self, fixture, home, arguments=None, native=None):
        self.messages = queue.Queue()
        self.process = fixture.start(home, arguments or ["app-server"], native=native)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        self.call(1, "initialize", {
            "clientInfo": {"name": "acomp_native_contract_test", "version": "1"},
            "capabilities": {"experimentalApi": True},
        })
        self.send({"method": "initialized", "params": {}})

    def read(self):
        for line in self.process.stdout:
            self.messages.put(json.loads(line))
        self.messages.put(None)

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def call(self, request_id, method, params):
        self.send({"id": request_id, "method": method, "params": params})
        end = time.monotonic() + 25
        while time.monotonic() < end:
            message = self.messages.get(timeout=max(0.01, end - time.monotonic()))
            if message is None:
                raise AssertionError("app-server exited before answering " + method)
            if message.get("id") == request_id:
                if "error" in message:
                    raise AssertionError(message["error"])
                return message["result"]
        raise AssertionError("app-server did not answer " + method)

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=10)
        self.reader.join(timeout=5)
        self.process.stdin.close()
        self.process.stdout.close()

    def wait_notification(self, method):
        end = time.monotonic() + 25
        while time.monotonic() < end:
            message = self.messages.get(timeout=max(0.01, end - time.monotonic()))
            if message is None:
                raise AssertionError("app-server exited before " + method)
            if message.get("method") == method:
                return message["params"]
        raise AssertionError("app-server did not emit " + method)


@unittest.skipUnless(NATIVE, "set ACOMP_TEST_CODEX_BINARY to opt in to isolated native contract tests")
class NativeResumeContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        verify_native_executable(NATIVE, "ACOMP_TEST_CODEX_BINARY")

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="acomp-native-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.source = self.root / "source"
        self.selected = self.root / "selected"
        self.overlay = self.root / "overlay"
        self.project = self.root / "project"
        for path in (self.source, self.selected, self.overlay, self.project):
            path.mkdir()
        self.server = LoopbackServer()
        self.addCleanup(self.server.close)
        self.stderr = tempfile.TemporaryFile(mode="w+")
        self.addCleanup(self.stderr.close)
        self.write_auth(self.source, "source-account")
        self.write_auth(self.selected, "selected-account")
        self.write_config(self.source)
        self.write_config(self.selected)
        version = self.run_native(self.source, ["--version"])
        versions = {VERSION}
        if self._testMethodName == "test_shared_usage_worker_publishes_quota_before_slow_history_without_daemon":
            versions.add("codex-cli 0.155.0-alpha.16.4")
        self.assertIn(version.stdout.strip(), versions, "native runtime version has not been verified")

    def write_auth(self, home, account):
        (home / "auth.json").write_text(json.dumps(fake_auth(account)), encoding="utf-8")

    def test_shared_usage_worker_publishes_quota_before_slow_history_without_daemon(self):
        # app-server --listen stdio:// is the direct server path, not daemon or
        # proxy. Match production flags without the newer daemon_auto_start
        # config key, which 0.155 rejects even when explicitly set to false.
        config = self.selected / "config.toml"
        config.write_text(config.read_text().replace("daemon_auto_start=false\n", ""))
        self.server.hold_usage_history = True
        self.addCleanup(self.server.usage_history_release.set)
        daemon = self.selected / "app-server-daemon"
        daemon.mkdir()
        sentinel = daemon / "fixture-owner"
        sentinel.write_text("existing-daemon-material")
        auth_before = (self.selected / "auth.json").read_bytes()
        worker = AppServer(self, self.selected, ["app-server", "--listen", "stdio://", "--strict-config",
                                               "-c", "features.remote_control=false",
                                               "-c", "sqlite_home=" + json.dumps(str(self.source))])
        self.addCleanup(worker.close)
        effective = worker.call(2, "config/read", {"includeLayers": False})["config"]
        self.assertEqual(effective["cli_auth_credentials_store"], "file")
        self.assertEqual(effective["sqlite_home"], str(self.source))
        self.assertEqual(effective["chatgpt_base_url"].rstrip("/"),
                         self.server.url.replace("127.0.0.1", "localhost") + "/backend-api")
        worker.send({"id": 20, "method": "account/usage/read"})
        self.assertTrue(self.server.usage_history_started.wait(10), "native history route was not reached")
        worker.send({"id": 21, "method": "account/rateLimits/read"})
        deadline = time.monotonic() + 10
        while True:
            message = worker.messages.get(timeout=max(0.01, deadline - time.monotonic()))
            self.assertIsNotNone(message)
            self.assertNotEqual(message.get("id"), 20, "quota waited for the blocked history request")
            if message.get("id") == 21:
                self.assertNotIn("error", message)
                self.assertEqual(message["result"]["rateLimits"]["primary"]["usedPercent"], 23)
                break
        self.server.usage_history_release.set()
        while True:
            message = worker.messages.get(timeout=10)
            self.assertIsNotNone(message)
            if message.get("id") == 20:
                self.assertNotIn("error", message)
                self.assertEqual(message["result"]["summary"]["lifetimeTokens"], 1000)
                break
        self.assertEqual((self.selected / "auth.json").read_bytes(), auth_before)
        self.assertEqual(sentinel.read_text(), "existing-daemon-material")
        reads = [(path, headers) for path, headers, _ in self.server.requests
                 if path.endswith(("/wham/usage", "/wham/profiles/me"))]
        self.assertEqual(len(reads), 2)
        for _, headers in reads:
            self.assertEqual({key.lower(): value for key, value in headers.items()}["chatgpt-account-id"],
                             "selected-account")
        self.assertEqual(self.server.model_requests(), [])

    def test_usage_worker_strict_config_rejects_legacy_profile_without_falling_back(self):
        path = self.selected / "config.toml"
        path.write_text("profile='legacy'\n" + path.read_text() + "\n[profiles.legacy]\nmodel='fixture'\n")
        auth_before = (self.selected / "auth.json").read_bytes()
        result = self.run_native(self.selected, ["app-server", "--listen", "stdio://", "--strict-config"],
                                 input_text="", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("legacy", result.stderr)
        self.assertEqual(self.server.requests, [])
        self.assertEqual((self.selected / "auth.json").read_bytes(), auth_before)

    def write_config(self, home):
        (home / "config.toml").write_text("\n".join([
            'model="gpt-5.2"',
            'model_provider="fixture"',
            'cli_auth_credentials_store="file"',
            'approval_policy="never"',
            'sandbox_mode="read-only"',
            'web_search="disabled"',
            'check_for_update_on_startup=false',
            'chatgpt_base_url=' + json.dumps(self.server.url.replace("127.0.0.1", "localhost") + "/backend-api"),
            '[model_providers.fixture]',
            'name="Loopback native contract fixture"',
            'base_url=' + json.dumps(self.server.url + "/v1"),
            'wire_api="responses"',
            'requires_openai_auth=true',
            'supports_websockets=false',
            '[features]',
            'enable_request_compression=false',
            'shell_snapshot=false',
            'remote_control=false',
            'daemon_auto_start=false',
            '[analytics]',
            'enabled=false',
            '[feedback]',
            'enabled=false',
            '',
        ]), encoding="utf-8")

    def environment(self, home):
        # Build a fresh environment: do not inherit auth, provider, daemon or
        # agent-session variables from the real user running this test.
        environment = {name: os.environ[name] for name in ("PATH", "SystemRoot", "WINDIR")
                       if name in os.environ}
        environment.update({
            "HOME": str(self.root), "USERPROFILE": str(self.root),
            "LOCALAPPDATA": str(self.root / "localappdata"),
            "APPDATA": str(self.root / "appdata"),
            "XDG_CONFIG_HOME": str(self.root / "xdg"),
            "CODEX_HOME": str(home), "CODEX_SQLITE_HOME": str(self.source),
            "CODEX_REFRESH_TOKEN_URL_OVERRIDE": self.server.url + "/oauth/token",
            "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
            "NO_PROXY": "127.0.0.1,localhost", "TERM": "xterm-256color",
        })
        return environment

    def run_native(self, home, arguments, input_text=None, check=True, native=None):
        result = subprocess.run(
            [native or NATIVE] + arguments, env=self.environment(home), cwd=self.project,
            input=input_text, capture_output=True, text=True, timeout=40,
        )
        if check:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        return result

    def start(self, home, arguments, native=None):
        environment = self.environment(home)
        environment["CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED"] = "1"
        return subprocess.Popen(
            [native or NATIVE] + arguments, env=environment, cwd=self.project,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True,
        )

    def make_overlay(self):
        # The complete namespace, including its coordination lock, is shared.
        for name in ("sessions", "archived_sessions", "thread-writer-locks", "memories"):
            (self.source / name).mkdir(exist_ok=True)
        for entry in self.source.iterdir():
            if entry.name == "auth.json" or entry.name.startswith("state_"):
                continue
            if entry.is_dir():
                directory_link(entry, self.overlay / entry.name)
            else:
                file_link(entry, self.overlay / entry.name)
        file_link(self.selected / "auth.json", self.overlay / "auth.json")

    def create_thread(self, prompt="first-user-marker", extra_args=None):
        result = self.run_native(self.source, ["exec", "--skip-git-repo-check", "--json"]
                                 + (extra_args or []) + [prompt])
        events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        thread_id = next(event["thread_id"] for event in events if event["type"] == "thread.started")
        return thread_id

    def test_native_file_save_preserves_symlink_and_hardlink_targets(self):
        for kind in ("symlink", "hardlink"):
            if kind == "symlink" and os.name == "nt":
                continue
            with self.subTest(link=kind):
                directory = self.root / ("save-" + kind)
                directory.mkdir()
                (directory / "config.toml").write_text('cli_auth_credentials_store="file"\n')
                target = self.selected / (kind + "-auth.json")
                target.write_text('{}')
                link = directory / "auth.json"
                if kind == "symlink":
                    link.symlink_to(target)
                else:
                    os.link(target, link)
                before = target.stat()
                self.run_native(directory, ["login", "--with-api-key"], "fixture-api-key\n")
                self.assertEqual(json.loads(target.read_text())["OPENAI_API_KEY"], "fixture-api-key")
                self.assertEqual((before.st_dev, before.st_ino), (target.stat().st_dev, target.stat().st_ino))
                self.assertTrue(os.path.samefile(target, link))

    def test_native_account_read_and_refresh_use_selected_link(self):
        source_before = (self.source / "auth.json").read_bytes()
        self.make_overlay()
        target = self.selected / "auth.json"
        before = target.stat()
        app = AppServer(self, self.overlay)
        self.addCleanup(app.close)
        account = app.call(2, "account/read", {"refreshToken": True})
        self.assertTrue(account["requiresOpenaiAuth"])
        self.assertEqual(account["account"]["type"], "chatgpt")
        self.assertEqual(account["account"]["email"], "selected-account@example.invalid")
        self.assertEqual(account["workspaceRouting"]["chatgptAccountId"], "selected-account")
        self.assertEqual(json.loads(target.read_text())["tokens"]["refresh_token"], "fixture-rotated-refresh")
        self.assertEqual((before.st_dev, before.st_ino), (target.stat().st_dev, target.stat().st_ino))
        self.assertTrue(os.path.samefile(target, self.overlay / "auth.json"))
        self.assertEqual((self.source / "auth.json").read_bytes(), source_before)

    def test_explicit_resume_reuses_paginated_history_and_selected_account(self):
        (self.source / "AGENTS.md").write_text("global-instruction-marker\n")
        (self.project / "AGENTS.md").write_text("project-instruction-marker\n")
        thread_id = self.create_thread()
        rollouts = list((self.source / "sessions").rglob("*.jsonl"))
        self.assertEqual(len(rollouts), 1)
        rollout = rollouts[0]
        metadata = json.loads(rollout.read_text().splitlines()[0])["payload"]
        self.assertEqual(metadata["id"], thread_id)
        self.assertEqual(metadata["history_mode"], "paginated")
        self.make_overlay()
        second = self.run_native(self.overlay, ["exec", "resume", "--skip-git-repo-check", "--json",
                                                thread_id, "second-user-marker"])
        self.assertIn(thread_id, second.stdout)
        request = self.server.model_requests()[-1]
        body = json.dumps(request[2])
        for marker in ("first-user-marker", "fixture-answer-1", "second-user-marker",
                       "global-instruction-marker", "project-instruction-marker"):
            self.assertIn(marker, body)
        headers = {key.lower(): value for key, value in request[1].items()}
        self.assertEqual(headers["chatgpt-account-id"], "selected-account")
        selected_token = json.loads((self.selected / "auth.json").read_text())["tokens"]["access_token"]
        self.assertEqual(headers["authorization"], "Bearer " + selected_token)
        self.run_native(self.source, ["exec", "resume", "--skip-git-repo-check", "--json",
                                     thread_id, "third-user-marker"])
        final_body = json.dumps(self.server.model_requests()[-1][2])
        for marker in ("first-user-marker", "second-user-marker", "third-user-marker", "fixture-answer-2"):
            self.assertIn(marker, final_body)
        self.assertEqual(list((self.source / "sessions").rglob("*.jsonl")), [rollout])
        self.assertTrue(os.path.samefile(self.source / "sessions", self.overlay / "sessions"))
        self.assertFalse(list(self.overlay.glob("state_*.sqlite")))

    def test_native_writer_blocks_resume_across_shared_lock_directory(self):
        thread_id = self.create_thread()
        self.make_overlay()
        self.server.hold_next_response = True
        owner = self.start(self.source, ["exec", "resume", "--skip-git-repo-check", "--json",
                                        thread_id, "writer-owner-marker"])
        try:
            self.assertTrue(self.server.started.wait(timeout=20), "native writer did not reach mock server")
            other = self.run_native(self.overlay, ["exec", "resume", "--skip-git-repo-check", "--json",
                                                   thread_id, "must-not-be-written"], check=False)
            self.assertNotEqual(other.returncode, 0)
            self.assertIn("active writer", other.stderr + other.stdout)
            self.assertNotIn("must-not-be-written", json.dumps(self.server.model_requests()))
        finally:
            self.server.release.set()
            owner.communicate(timeout=20)

    @unittest.skipUnless(LEGACY_NATIVE, "set ACOMP_TEST_LEGACY_CODEX_BINARY for cross-version history acceptance")
    def test_legacy_history_survives_pinned_resume_and_reopen_with_shared_writer_locks(self):
        verify_native_executable(LEGACY_NATIVE, "ACOMP_TEST_LEGACY_CODEX_BINARY")
        version = self.run_native(self.source, ["--version"], native=LEGACY_NATIVE)
        self.assertEqual(version.stdout.strip(), "codex-cli 0.155.0-alpha.16.4")
        # Both versions use the explicit standalone app-server path. The legacy
        # strict parser does not know the newer daemon_auto_start feature key.
        for home in (self.source, self.selected):
            config = home / "config.toml"
            config.write_text(config.read_text().replace("daemon_auto_start=false\n", ""))
        config_before = (self.source / "config.toml").read_bytes()
        auth_before = (self.source / "auth.json").read_bytes()
        arguments = ["app-server", "--listen", "stdio://", "--strict-config"]

        def complete_turn(app, thread_id, marker, request_id):
            app.call(request_id, "turn/start", {
                "threadId": thread_id, "input": [{"type": "text", "text": marker}],
            })
            self.assertEqual(app.wait_notification("turn/completed")["turn"]["status"], "completed")

        def paginated_ids(app):
            cursor = None
            ids = []
            for request_id in range(30, 35):
                page = app.call(request_id, "thread/list", {"limit": 1, "cursor": cursor})
                ids.extend(thread["id"] for thread in page["data"])
                cursor = page.get("nextCursor")
                if cursor is None:
                    return ids
            self.fail("legacy history pagination did not terminate")

        def database_schema():
            result = {}
            columns = {}
            for path in self.source.glob("*.sqlite"):
                with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as database:
                    result[path.name] = database.execute(
                        "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
                    ).fetchall()
                    for table in ("threads", "thread_items"):
                        values = database.execute("PRAGMA table_info(" + table + ")").fetchall()
                        if values:
                            columns[path.name, table] = values
            return result, columns

        with closing(AppServer(self, self.source, arguments, native=LEGACY_NATIVE)) as old:
            ids = []
            for index in range(2):
                thread = old.call(10 + index * 2, "thread/start", {"cwd": str(self.project)})["thread"]
                ids.append(thread["id"])
                self.assertEqual(thread["historyMode"], "paginated")
                complete_turn(old, thread["id"], "legacy-seed-" + str(index), 11 + index * 2)
                if index == 0:
                    # The legacy list cursor has second precision. Distinct
                    # timestamps exercise two pages without its same-second tie.
                    time.sleep(1.1)
            self.assertCountEqual(paginated_ids(old), ids)
            schema_before, columns_before = database_schema()
            self.assertIn("state_5.sqlite", schema_before)
            self.assertIn("thread_history_1.sqlite", schema_before)
            rollouts_before = sorted((self.source / "sessions").rglob("*.jsonl"))
            self.assertEqual(len(rollouts_before), 2)
            self.make_overlay()
            coordination = self.source / "thread-writer-locks" / ".coordination.lock"
            self.assertTrue(coordination.is_file())
            self.assertTrue(os.path.samefile(
                coordination, self.overlay / "thread-writer-locks" / ".coordination.lock"))
            self.server.hold_next_response = True
            old.send({"id": 20, "method": "turn/start", "params": {
                "threadId": ids[0], "input": [{"type": "text", "text": "legacy-held-marker"}],
            }})
            try:
                self.assertTrue(self.server.started.wait(10), "legacy writer did not reach the fixture")
                with closing(AppServer(self, self.overlay, arguments)) as new:
                    with self.assertRaisesRegex(AssertionError, "active writer"):
                        new.call(2, "thread/resume", {"threadId": ids[0]})
            finally:
                self.server.release.set()
            self.assertEqual(old.wait_notification("turn/completed")["turn"]["status"], "completed")

        with closing(AppServer(self, self.overlay, arguments)) as new:
            resumed = new.call(2, "thread/resume", {"threadId": ids[0]})
            self.assertEqual(resumed["thread"]["id"], ids[0])
            self.server.started.clear()
            self.server.release.clear()
            self.server.hold_next_response = True
            new.send({"id": 3, "method": "turn/start", "params": {
                "threadId": ids[0], "input": [{"type": "text", "text": "pinned-resume-marker"}],
            }})
            try:
                self.assertTrue(self.server.started.wait(10), "pinned writer did not reach the fixture")
                with closing(AppServer(self, self.source, arguments, native=LEGACY_NATIVE)) as old:
                    with self.assertRaisesRegex(AssertionError, "active writer"):
                        old.call(2, "thread/resume", {"threadId": ids[0]})
            finally:
                self.server.release.set()
            self.assertEqual(new.wait_notification("turn/completed")["turn"]["status"], "completed")
        schema_after, columns_after = database_schema()
        # Opening legacy databases invokes native migrations. This audited pair
        # adds nullable columns and archive indexes; old clients must continue
        # to list, read and append the same paginated histories afterward.
        additions = {
            ("state_5.sqlite", "threads"): [("creator_user_id", "TEXT"), ("creator_account_id", "TEXT")],
            ("thread_history_1.sqlite", "thread_items"): [("started_at_ms", "INTEGER"), ("completed_at_ms", "INTEGER")],
        }
        self.assertEqual(schema_after.keys(), schema_before.keys())
        self.assertEqual(columns_after.keys(), columns_before.keys())
        for key, before in columns_before.items():
            added = [(len(before) + index, name, kind, 0, None, 0)
                     for index, (name, kind) in enumerate(additions.get(key, []))]
            self.assertEqual(columns_after[key], before + added)
        archive_indexes = {"idx_threads_archive_" + field
                           for field in ("created_at_ms", "recency_at_ms", "updated_at_ms")}
        for database, before in schema_before.items():
            old_objects = {(kind, name): (table, sql) for kind, name, table, sql in before}
            new_objects = {(kind, name): (table, sql) for kind, name, table, sql in schema_after[database]}
            expected_added = {("index", name) for name in archive_indexes} if database == "state_5.sqlite" else set()
            self.assertEqual(new_objects.keys() - old_objects.keys(), expected_added)
            self.assertFalse(old_objects.keys() - new_objects.keys())
            for key, value in old_objects.items():
                if key[0] != "table" or (database, key[1]) not in additions:
                    self.assertEqual(new_objects[key], value)

        with closing(AppServer(self, self.source, arguments, native=LEGACY_NATIVE)) as old:
            self.assertCountEqual(paginated_ids(old), ids)
            read = old.call(40, "thread/read", {"threadId": ids[0], "includeTurns": True})
            self.assertEqual(read["thread"]["historyMode"], "paginated")
            for marker in ("legacy-seed-0", "legacy-held-marker", "pinned-resume-marker"):
                self.assertIn(marker, json.dumps(read))
            self.assertEqual(old.call(41, "thread/resume", {"threadId": ids[0]})["thread"]["id"], ids[0])
            complete_turn(old, ids[0], "legacy-reopen-marker", 42)
        body = json.dumps(self.server.model_requests()[-1][2])
        for marker in ("legacy-seed-0", "legacy-held-marker", "pinned-resume-marker", "legacy-reopen-marker"):
            self.assertIn(marker, body)
        self.assertEqual(sorted((self.source / "sessions").rglob("*.jsonl")), rollouts_before)
        self.assertEqual(database_schema(), (schema_after, columns_after))
        self.assertFalse(list(self.overlay.glob("state_*.sqlite")))
        self.assertEqual((self.source / "config.toml").read_bytes(), config_before)
        self.assertEqual((self.source / "auth.json").read_bytes(), auth_before)

    def test_tool_results_and_image_attachment_survive_native_resume(self):
        image = self.project / "fixture.png"

        def chunk(kind, data):
            return struct.pack("!I", len(data)) + kind + data + struct.pack("!I", zlib.crc32(kind + data))

        image.write_bytes(b"\x89PNG\r\n\x1a\n" +
                          chunk(b"IHDR", struct.pack("!2I5B", 1, 1, 8, 2, 0, 0, 0)) +
                          chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00")) + chunk(b"IEND", b""))
        self.server.responses.append([{
            "type": "function_call", "call_id": "fixture-tool-call", "name": "exec_command",
            "arguments": json.dumps({"cmd": "printf native-tool-result", "yield_time_ms": 1000}),
        }])
        thread_id = self.create_thread("attachment-user-marker", ["--image", str(image), "--"])
        initial_requests = self.server.model_requests()
        self.assertEqual(len(initial_requests), 2)
        tool_outputs = [item for item in initial_requests[-1][2]["input"]
                        if item.get("type") == "function_call_output"]
        self.assertEqual(len(tool_outputs), 1)
        self.assertIn("native-tool-result", json.dumps(tool_outputs[0]))
        initial_images = [part["image_url"] for item in initial_requests[0][2]["input"]
                          for part in item.get("content", []) if part.get("type") == "input_image"]
        self.assertEqual(len(initial_images), 1)
        image.unlink()  # The persisted attachment must remain self-contained.
        self.make_overlay()
        self.run_native(self.overlay, ["exec", "resume", "--skip-git-repo-check", "--json",
                                       thread_id, "after-image-marker"])
        resumed = self.server.model_requests()[-1][2]
        self.assertIn("native-tool-result", json.dumps(resumed))
        self.assertIn("fixture-tool-call", json.dumps(resumed))
        resumed_images = [part["image_url"] for item in resumed["input"]
                          for part in item.get("content", []) if part.get("type") == "input_image"]
        self.assertEqual(resumed_images, initial_images)

    def test_fork_dependencies_remain_native_when_resuming_overlay(self):
        parent_id = self.create_thread("parent-history-marker")
        parent_rollout = next((self.source / "sessions").rglob("*.jsonl"))
        parent_before = parent_rollout.read_bytes()
        fork = self.run_native(self.source, ["exec", "fork", "--skip-git-repo-check", "--json",
                                            parent_id, "fork-history-marker"])
        fork_events = [json.loads(line) for line in fork.stdout.splitlines() if line.startswith("{")]
        fork_id = next(event["thread_id"] for event in fork_events if event["type"] == "thread.started")
        self.assertNotEqual(fork_id, parent_id)
        self.make_overlay()
        resumed = self.run_native(self.overlay, ["exec", "resume", "--skip-git-repo-check", "--json",
                                                fork_id, "after-fork-marker"])
        self.assertIn(fork_id, resumed.stdout)
        body = json.dumps(self.server.model_requests()[-1][2])
        for marker in ("parent-history-marker", "fork-history-marker", "after-fork-marker"):
            self.assertIn(marker, body)
        self.assertEqual(parent_rollout.read_bytes(), parent_before)
        self.assertEqual(len(list((self.source / "sessions").rglob("*.jsonl"))), 2)

    def test_encrypted_compaction_checkpoint_survives_native_resume(self):
        config = self.source / "config.toml"
        config.write_text(config.read_text().replace('name="Loopback native contract fixture"', 'name="OpenAI"'))
        thread_id = self.create_thread("before-compaction-marker")
        app = AppServer(self, self.source)
        try:
            resumed = app.call(2, "thread/resume", {"threadId": thread_id, "excludeTurns": True})
            self.assertEqual(resumed["thread"]["id"], thread_id)
            self.server.responses.append([{"type": "compaction",
                                           "encrypted_content": "native-opaque-compaction-fixture"}])
            app.call(3, "thread/compact/start", {"threadId": thread_id})
            completed = app.wait_notification("turn/completed")
            self.assertEqual(completed["turn"]["status"], "completed")
        finally:
            app.close()
        self.make_overlay()
        resumed = self.run_native(self.overlay, ["exec", "resume", "--skip-git-repo-check", "--json",
                                                thread_id, "after-compaction-marker"])
        self.assertIn(thread_id, resumed.stdout)
        request = self.server.model_requests()[-1][2]
        checkpoints = [item for item in request["input"] if item.get("type") == "compaction"]
        self.assertEqual(len(checkpoints), 1)
        self.assertEqual(checkpoints[0]["encrypted_content"], "native-opaque-compaction-fixture")
        self.assertIn("after-compaction-marker", json.dumps(request))
        self.assertEqual(len(list((self.source / "sessions").rglob("*.jsonl"))), 1)

    def test_preflight_config_read_honors_project_trust_and_runtime_overrides(self):
        (self.project / ".git").mkdir()
        (self.project / ".codex").mkdir()
        project_config = self.project / ".codex" / "config.toml"
        project_config.write_text('model="gpt-5.1"\n')
        self.make_overlay()
        arguments = ["-c", 'cli_auth_credentials_store="file"',
                     "-c", "sqlite_home=" + json.dumps(str(self.source)),
                     "app-server", "--listen", "stdio://", "--strict-config"]
        app = AppServer(self, self.overlay, arguments)
        try:
            untrusted = app.call(2, "config/read", {"includeLayers": True, "cwd": str(self.project)})
            self.assertEqual(untrusted["config"]["model"], "gpt-5.2")
            disabled = [layer for layer in untrusted["layers"] if layer.get("disabledReason")]
            self.assertTrue(disabled, "untrusted project layer was not identified")
        finally:
            app.close()
        with (self.source / "config.toml").open("a") as source_config:
            source_config.write("[projects." + json.dumps(str(self.project)) + "]\ntrust_level=\"trusted\"\n")
        app = AppServer(self, self.overlay, arguments)
        try:
            trusted = app.call(2, "config/read", {"includeLayers": True, "cwd": str(self.project)})
            self.assertEqual(trusted["config"]["model"], "gpt-5.1")
            self.assertEqual(trusted["config"]["sqlite_home"], str(self.source))
            self.assertEqual(trusted["config"]["cli_auth_credentials_store"], "file")
            self.assertEqual(trusted["origins"]["model"]["name"]["type"], "project")
        finally:
            app.close()

    @unittest.skipUnless(os.name == "posix", "disabled MCP executable probe uses a POSIX fixture")
    def test_disabled_relative_mcp_config_survives_overlay_without_starting_the_tool(self):
        relative = Path("Codex Computer Use.app/Contents/SharedSupport/"
                        "SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient")
        marker = self.root / "disabled-mcp-must-not-run"
        executable = self.project / relative
        executable.parent.mkdir(parents=True)
        executable.write_text("#!/bin/sh\nprintf started >> " + shlex.quote(str(marker)) + "\nexit 42\n")
        executable.chmod(0o755)
        config = self.source / "config.toml"
        config.write_text(config.read_text() + '\n[mcp_servers.computer-use]\ncommand=' +
                          json.dumps("./" + str(relative)) + '\ncwd="."\nenabled=false\n')
        original_config = config.read_bytes()
        self.make_overlay()
        effective = []
        for home in (self.source, self.overlay):
            with self.subTest(home=home.name):
                app = AppServer(self, home, ["app-server", "--listen", "stdio://", "--strict-config"])
                try:
                    observed = app.call(2, "config/read", {"includeLayers": True, "cwd": str(self.project)})
                    mcp = observed["config"]["mcp_servers"]["computer-use"]
                    self.assertIs(mcp["enabled"], False)
                    self.assertEqual(mcp["command"], "./" + str(relative))
                    self.assertEqual(mcp["cwd"], ".")
                    effective.append(observed["config"])
                    user = [layer for layer in observed["layers"] if layer["name"]["type"] == "user"]
                    self.assertEqual(user[0]["name"]["file"], str(home / "config.toml"))
                    self.assertFalse(user[0].get("disabledReason"))
                    thread = app.call(3, "thread/start", {"cwd": str(self.project)})["thread"]["id"]
                    app.call(4, "turn/start", {"threadId": thread, "input": [{
                        "type": "text", "text": "disabled-mcp-config-contract",
                    }]})
                    self.assertEqual(app.wait_notification("turn/completed")["turn"]["status"], "completed")
                finally:
                    app.close()
                self.assertFalse(marker.exists(), "an explicitly disabled MCP process was started")
        self.assertEqual(effective[0], effective[1])
        self.assertEqual(config.read_bytes(), original_config)
        self.assertEqual(len(self.server.model_requests()), 2)

    def test_git_root_excludes_primary_home_config_from_secondary_project_layers(self):
        # Reproduce a Dodex home inside the same user's home as the primary
        # .codex/config.toml, with a Git checkout farther down that user's tree.
        primary = self.root / ".codex"
        primary.mkdir()
        primary_config = primary / "config.toml"
        primary_config.write_text('model="primary-home-must-not-load"\n'
                                  'approval_policy="untrusted"\nsandbox_mode="workspace-write"\n'
                                  '[mcp_servers.primary-only]\ncommand="must-not-run"\nenabled=false\n')
        repository = self.root / "Github" / "project"
        self.project = repository / "nested"
        self.project.mkdir(parents=True)
        (repository / ".git").mkdir()
        (repository / ".git" / "HEAD").write_text("ref: refs/heads/fixture\n")
        # An untrusted config at the real repository root proves that discovery
        # walks above cwd, while stopping before the primary home farther up.
        project_settings = repository / ".codex"
        project_settings.mkdir()
        (project_settings / "config.toml").write_text('model="untrusted-project-must-not-load"\n')
        self.make_overlay()
        original_primary = primary_config.read_bytes()
        effective = []
        for home in (self.source, self.overlay):
            with self.subTest(home=home.name):
                app = AppServer(self, home, ["app-server", "--listen", "stdio://", "--strict-config"])
                try:
                    observed = app.call(2, "config/read", {"includeLayers": True, "cwd": str(self.project)})
                    effective.append(observed["config"])
                    self.assertEqual(observed["config"]["model"], "gpt-5.2")
                    self.assertEqual(observed["config"]["approval_policy"], "never")
                    self.assertEqual(observed["config"]["sandbox_mode"], "read-only")
                    self.assertNotIn("primary-only", observed["config"]["mcp_servers"])
                    self.assertNotIn(str(primary_config), [layer["name"].get("file")
                                                          for layer in observed["layers"]])
                    project_layers = [layer for layer in observed["layers"] if layer["name"]["type"] == "project"]
                    self.assertEqual(len(project_layers), 1)
                    self.assertIn(str(project_settings), project_layers[0]["name"].values())
                    self.assertTrue(project_layers[0].get("disabledReason"))
                finally:
                    app.close()
        self.assertEqual(effective[0], effective[1])
        self.assertEqual(primary_config.read_bytes(), original_primary)
        self.assertEqual(self.server.model_requests(), [])

    @unittest.skipUnless(os.name == "posix", "native Windows TUI needs ConPTY acceptance")
    def test_native_tui_restores_history_uses_current_model_and_exits_cleanly(self):
        import fcntl
        import pty
        import select
        import termios

        thread_id = self.create_thread("native-tui-history-marker")
        config = self.source / "config.toml"
        config.write_text(config.read_text().replace('model="gpt-5.2"', 'model="gpt-5.1"'))
        with config.open("a") as source_config:
            source_config.write("[projects." + json.dumps(str(self.project)) + "]\ntrust_level=\"trusted\"\n")
        self.make_overlay()
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))
        process = subprocess.Popen(
            [NATIVE, "--no-daemon", "--no-alt-screen", "--strict-config", "-c", 'model="gpt-5.1"',
             "-c", 'model_provider="fixture"', "-c", "sqlite_home=" + json.dumps(str(self.source)),
             "resume", thread_id, "native-tui-resume-marker"],
            cwd=self.project, env=self.environment(self.overlay), stdin=slave, stdout=slave,
            stderr=slave, start_new_session=True,
        )
        os.close(slave)
        transcript = b""
        sent_exit = False
        deadline = time.monotonic() + 30
        try:
            while process.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], 0.1)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    transcript += chunk
                    # Answer native terminal capability probes; this is a real
                    # PTY and real Codex TUI, with only its remote model mocked.
                    if b"\x1b[6n" in chunk:
                        os.write(master, b"\x1b[1;1R")
                    if b"\x1b[c" in chunk:
                        os.write(master, b"\x1b[?1;2c")
                    if b"\x1b]11;?" in chunk:
                        os.write(master, b"\x1b]11;rgb:0000/0000/0000\x1b\\")
                if b"fixture-answer-2" in transcript and not sent_exit:
                    os.write(master, b"\x04")
                    sent_exit = True
            self.assertTrue(sent_exit, "native TUI did not render its completed resumed turn")
            process.wait(timeout=5)
            self.assertEqual(process.returncode, 0)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            os.close(master)
        self.assertIn(b"native-tui-history-marker", transcript)
        self.assertIn(b"fixture-answer-1", transcript)
        self.assertEqual(len(self.server.model_requests()), 2)
        request = self.server.model_requests()[-1]
        self.assertEqual(request[2]["model"], "gpt-5.1")
        self.assertIn("native-tui-history-marker", json.dumps(request[2]))
        self.assertIn("native-tui-resume-marker", json.dumps(request[2]))
        headers = {key.lower(): value for key, value in request[1].items()}
        self.assertEqual(headers["chatgpt-account-id"], "selected-account")
        self.assertFalse((self.overlay / "daemon").exists())

    @unittest.skipUnless(ACOMP and sys.platform == "darwin",
                         "set ACOMP_TEST_ACOMP_BINARY on macOS for real CLI preflight smoke")
    def test_acomp_native_preflight_denial_preserves_original_history_and_auth(self):
        """Real prepare must clean up when native authenticated discovery fails.

        A positive production launch needs a real backend-validated account.
        This deliberately keeps the endpoint compatibility checks intact.
        """
        import pty
        import select

        acomp = Path(ACOMP)
        self.assertTrue(acomp.is_absolute(), "ACOMP_TEST_ACOMP_BINARY must be absolute")
        user = self.root / "synthetic-user"
        self.source = user / ".codex"
        self.source.mkdir(parents=True)
        self.write_auth(self.source, "source-account")
        self.write_config(self.source)
        # App-server sessions are native interactive history. Exec sessions
        # deliberately do not appear in the production session picker.
        app = AppServer(self, self.source)
        try:
            started = app.call(2, "thread/start", {"cwd": str(self.project)})
            thread_id = started["thread"]["id"]
            app.call(3, "turn/start", {
                "threadId": thread_id,
                "input": [{"type": "text", "text": "production-preflight-history-marker"}],
            })
            completed = app.wait_notification("turn/completed")
            self.assertEqual(completed["turn"]["status"], "completed")
        finally:
            app.close()
        native_entry = self.source / "packages/standalone/current/bin/codex"
        native_entry.parent.mkdir(parents=True)
        native_entry.symlink_to(NATIVE)
        support = user / "Library/Application Support/AgentCompanion"
        support.mkdir(parents=True)
        (support / "dual-instance.json").write_text(json.dumps({
            "schema": 1, "enabled": False,
            "instance": {"codex_home": str(self.selected), "database_dir": str(self.selected),
                         "cli_path": NATIVE},
        }))
        # Both source and quota config are ordinary production-compatible
        # configs. There is no custom endpoint, provider, or test-only bypass.
        plain_config = ('model="gpt-5.2"\ncli_auth_credentials_store="file"\n'
                        'approval_policy="never"\nsandbox_mode="read-only"\n'
                        'check_for_update_on_startup=false\n[features]\n'
                        'daemon_auto_start=false\nremote_control=false\nshell_snapshot=false\n'
                        '[analytics]\nenabled=false\n[feedback]\nenabled=false\n')
        (self.source / "config.toml").write_text(plain_config)
        (self.selected / "config.toml").write_text(plain_config)
        source_auth = (self.source / "auth.json").read_bytes()
        selected_auth = (self.selected / "auth.json").read_bytes()
        rollouts = {path: path.read_bytes() for path in (self.source / "sessions").rglob("*.jsonl")}
        history_db = self.source / "thread_history_1.sqlite"

        history_before = history_db.read_bytes()
        environment = self.environment(self.source)
        for key in ("CODEX_HOME", "CODEX_SQLITE_HOME", "CODEX_REFRESH_TOKEN_URL_OVERRIDE"):
            environment.pop(key, None)
        environment.update({"HOME": str(user), "USERPROFILE": str(user),
                            "HTTP_PROXY": self.server.url, "HTTPS_PROXY": self.server.url,
                            "http_proxy": self.server.url, "https_proxy": self.server.url})
        # Even if a native HTTP client ignores proxy environment variables,
        # macOS enforces that this subprocess tree can only connect to loopback.
        sandbox = ('(version 1)(allow default)(deny network-outbound)'
                   '(allow network-outbound (remote ip "localhost:*"))')
        master, slave = pty.openpty()
        process = subprocess.Popen(
            ["/usr/bin/sandbox-exec", "-p", sandbox, str(acomp), "resume", thread_id,
             "--source", "codex", "--account", "dodex"],
            cwd=self.project, env=environment, stdin=slave, stdout=slave, stderr=slave,
            start_new_session=True,
        )
        os.close(slave)
        transcript = b""
        deadline = time.monotonic() + 50
        try:
            while process.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], 0.1)[0]:
                    try:
                        transcript += os.read(master, 65536)
                    except OSError:
                        break
            process.wait(timeout=5)
            self.assertNotEqual(process.returncode, 0)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            os.close(master)
        output = transcript.decode(errors="replace")
        self.assertIn("account/read", output, output)
        self.assertNotIn("Resume is disabled:", output, output)
        self.assertNotIn("Resuming session", output)
        self.assertTrue((support / "Resume/launch-locks").is_dir())
        self.assertFalse(list((support / "Resume").glob("run-*")))
        self.assertEqual((self.source / "auth.json").read_bytes(), source_auth)
        self.assertEqual((self.selected / "auth.json").read_bytes(), selected_auth)
        self.assertEqual({path: path.read_bytes() for path in (self.source / "sessions").rglob("*.jsonl")}, rollouts)
        self.assertEqual(history_db.read_bytes(), history_before)
        self.assertEqual(len(self.server.model_requests()), 1, "preflight must not start a model turn")
        self.assertNotIn("fixture-refresh", output)


if __name__ == "__main__":
    unittest.main()
