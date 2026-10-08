#!/usr/bin/env python3
"""Real standalone install/update and two native daemons in disposable profiles.

Downloads official packages. Uses synthetic accounts and a loopback Responses
server; never reads the developer's accounts or sends fixture tokens externally.
Runs on native macOS and Windows, including Windows console-entry packaging.
"""
import argparse
import hashlib
from contextlib import nullcontext, redirect_stdout, redirect_stderr
import importlib.util
import json
import os
import base64
import socket
import struct
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.request


def load_contracts():
    spec = importlib.util.spec_from_file_location(
        "native_contracts", Path(__file__).parent / "tests/test_resume_native.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(command, environment, cwd, timeout=180, check=True):
    result = subprocess.run([str(arg) for arg in command], env=environment, cwd=cwd,
                            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
    if check and result.returncode:
        raise AssertionError(f"{Path(command[0]).name} {command[1:]} failed: "
                             + result.stderr[-5000:] + result.stdout[-2000:])
    return result


def tree_hash(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if path.is_file():
            digest.update(str(path.relative_to(root)).encode())
            with path.open("rb") as file:
                while chunk := file.read(1024 * 1024):
                    digest.update(chunk)
    return digest.hexdigest()


class SocketClient:
    """The native control socket speaks WebSocket, not direct-server JSONL."""
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(25)
        if os.name == "nt":
            # Windows already validates its short canonical native address.
            self.socket.connect(str(path))
        else:
            # Native macOS supports deep homes. Python's sockaddr construction
            # has a shorter absolute-string limit; use the existing endpoint
            # relative to its directory without changing any Codex routing.
            current = Path.cwd()
            try:
                os.chdir(Path(path).parent)
                self.socket.connect(Path(path).name)
            finally:
                os.chdir(current)
        nonce = base64.b64encode(os.urandom(16))
        self.socket.sendall(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: " + nonce + b"\r\n\r\n")
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            part = self.socket.recv(1)
            if not part:
                raise AssertionError("Native daemon closed its WebSocket handshake")
            header.extend(part)
        assert header.startswith(b"HTTP/1.1 101"), header
        self.call(1, "initialize", {"clientInfo": {"name": "acomp_native_instances", "version": "1"},
                                   "capabilities": {"experimentalApi": True}})
        self.send({"method": "initialized", "params": {}})

    def frame(self, payload, opcode=1):
        mask = os.urandom(4)
        size = len(payload)
        header = bytes([0x80 | opcode, 0x80 | size]) if size < 126 else (
            bytes([0x80 | opcode, 0x80 | 126]) + struct.pack(">H", size) if size < 65536 else
            bytes([0x80 | opcode, 0x80 | 127]) + struct.pack(">Q", size))
        self.socket.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

    def send(self, message):
        self.frame(json.dumps(message).encode())

    def receive(self, size):
        result = bytearray()
        while len(result) < size:
            part = self.socket.recv(size - len(result))
            if not part:
                raise AssertionError("Native daemon closed its control connection")
            result.extend(part)
        return bytes(result)

    def message(self):
        fragments = bytearray()
        while True:
            first, second = self.receive(2)
            size = second & 127
            if size == 126:
                size = struct.unpack(">H", self.receive(2))[0]
            elif size == 127:
                size = struct.unpack(">Q", self.receive(8))[0]
            assert size <= 16 * 1024 * 1024
            mask = self.receive(4) if second & 128 else None
            payload = self.receive(size)
            if mask:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            opcode = first & 15
            if opcode == 9:
                self.frame(payload, 10)
                continue
            if opcode == 8:
                raise AssertionError("Native daemon sent a close frame")
            if opcode == 10:
                continue
            fragments.extend(payload)
            if first & 128:
                return json.loads(fragments)

    def call(self, request_id, method, params):
        self.send({"id": request_id, "method": method, "params": params})
        end = time.monotonic() + 25
        while time.monotonic() < end:
            message = self.message()
            if message.get("id") == request_id:
                assert "error" not in message, message
                return message["result"]
        raise AssertionError("Native daemon did not answer " + method)

    def close(self):
        self.socket.close()


class Acceptance:
    def __init__(self, companion, root, primary_release):
        self.companion = companion
        self.root = root
        self.home = root / ("u" if os.name == "nt" else "user")
        self.project = root / "project"
        self.home.mkdir()
        self.project.mkdir()
        self.support = (self.home / "Library/Application Support/AgentCompanion"
                        if sys.platform == "darwin" else self.home / "l/AgentCompanion")
        self.environment = {name: os.environ[name] for name in (
            "PATH", "SystemRoot", "WINDIR", "OS", "PATHEXT", "ComSpec", "SystemDrive",
            "ProgramData", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432",
            "TEMP", "TMP", "LANG") if name in os.environ}
        self.environment.update(HOME=str(self.home), USERPROFILE=str(self.home),
                                LOCALAPPDATA=str(self.home / ("l" if os.name == "nt" else "local")),
                                APPDATA=str(self.home / "roaming"),
                                XDG_CONFIG_HOME=str(self.home / "xdg"),
                                CODEX_NON_INTERACTIVE="1", TERM="xterm-256color",
                                NO_PROXY="localhost,127.0.0.1", no_proxy="localhost,127.0.0.1")
        self.primary = self.home / ".codex"
        self.primary.mkdir()
        self.native_name = "codex.exe" if os.name == "nt" else "codex"
        self.contracts = load_contracts()
        self.server = self.contracts.LoopbackServer()
        self.clients = []
        self.sockets = {}
        self.stderr = tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace")
        print("Installing the native primary TUI " + primary_release, flush=True)
        self.install(self.primary, primary_release)
        self.environment["PATH"] = str(self.primary / "native-bin") + os.pathsep + self.environment["PATH"]

    def execute(self, entry, arguments, **kwargs):
        return run([entry] + arguments, self.environment, self.project, **kwargs)

    def install(self, home, release):
        windows = os.name == "nt"
        name = "install.ps1" if windows else "install.sh"
        script = self.root / name
        if not script.is_file():
            with urllib.request.urlopen(urllib.request.Request("https://chatgpt.com/codex/" + name, headers={"User-Agent": "agent-companion-tui-installer"}), timeout=60) as response:
                script.write_bytes(response.read(2 * 1024 * 1024))
        prefix = home / "native-bin"
        environment = dict(self.environment, CODEX_HOME=str(home), CODEX_INSTALL_DIR=str(prefix),
                           CODEX_SQLITE_HOME=str(home / "sqlite"))
        if windows:
            system = Path(os.environ["SystemRoot"]) / "System32"
            command = [system / "WindowsPowerShell/v1.0/powershell.exe", "-NoLogo", "-NoProfile",
                       "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script,
                       "-Release", release]
            environment["PATH"] = os.pathsep.join(map(str, [prefix, system]))
        else:
            command = ["/bin/sh", script, "--release", release]
            environment["PATH"] = os.pathsep.join(map(str, [prefix, "/usr/bin", "/bin", "/usr/sbin", "/sbin"]))
        run(command, environment, self.project, timeout=900)

    def configure(self, home, account):
        config = home / "config.toml"
        original = "\n".join(line for line in config.read_text().splitlines()
                              if line.startswith(("cli_auth_credentials_store", "sqlite_home", "log_dir"))) if config.exists() else ""
        config.write_text(original + "\n" + "\n".join([
            'model="gpt-5.2"', 'model_provider="fixture"',
            'approval_policy="never"', 'sandbox_mode="read-only"',
            'web_search="disabled"', 'check_for_update_on_startup=false',
            'chatgpt_base_url=' + json.dumps(self.server.url.replace("127.0.0.1", "localhost") + "/backend-api"),
            '[model_providers.fixture]', 'name="Local TUI acceptance fixture"',
            'base_url=' + json.dumps(self.server.url + "/v1"), 'wire_api="responses"',
            'requires_openai_auth=true', 'supports_websockets=false',
            '[features]', 'enable_request_compression=false', 'remote_control=false',
            'shell_snapshot=false', '[analytics]', 'enabled=false', '[feedback]', 'enabled=false', '',
        ]), encoding="utf-8")
        (home / "auth.json").write_text(json.dumps(self.contracts.fake_auth(account)), encoding="utf-8")

    def start(self, home, arguments, native=None):
        entry = self.codex if home == self.primary else self.dodex
        return subprocess.Popen([str(entry)] + arguments, env=self.environment, cwd=self.project,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True)

    def client(self, home):
        client = SocketClient(self.sockets[home])
        self.clients.append(client)
        return client

    def check(self):
        print("Installing the fresh Dodex TUI with Companion", flush=True)
        self.execute(self.companion, ["dodex-tui", "--install"], timeout=900)
        record = json.loads((self.support / "tui-instances.json").read_text())
        assert record["primary"]["channel"] == "standalone"
        self.secondary = Path(record["dodex"]["codex_home"])
        self.codex = Path(record["primary"]["command_path"])
        self.dodex = Path(record["dodex"]["command_path"])
        assert not (self.secondary / "auth.json").exists()
        self.execute(self.companion, ["dodex-tui", "--repair"], timeout=900)
        assert record == json.loads((self.support / "tui-instances.json").read_text())
        assert not (self.secondary / "auth.json").exists()
        # Installing Companion's own terminal aliases must leave a repairable,
        # independent TUI entry rather than a link into its desktop bundle.
        self.execute(self.companion, ["install-cli"])
        assert not self.dodex.is_symlink()
        self.execute(self.companion, ["dodex-tui", "--repair"], timeout=900)
        assert record == json.loads((self.support / "tui-instances.json").read_text())
        assert not (self.secondary / "auth.json").exists()
        print("Verifying native command parity and independent versions", flush=True)
        for arguments in [["--help"], ["resume", "--help"], ["fork", "--help"],
                          ["exec", "--help"], ["login", "--help"], ["mcp", "--help"],
                          ["plugin", "--help"], ["agents", "--help"], ["queue", "--help"],
                          ["app-server", "daemon", "--help"], ["--no-daemon", "--help"]]:
            alias = self.execute(self.dodex, arguments)
            native_environment = dict(self.environment, CODEX_HOME=str(self.secondary),
                                      CODEX_SQLITE_HOME=record["dodex"]["database_dir"],
                                      CODEX_INSTALL_DIR=record["dodex"]["install_dir"])
            native = run([record["dodex"]["cli_path"]] + arguments, native_environment, self.project)
            assert alias.stdout == native.stdout, arguments
            assert alias.returncode == native.returncode
        assert self.execute(self.dodex, ["app"], check=False).returncode != 0
        primary_version = self.execute(self.codex, ["--version"]).stdout.strip()
        # Select an older real release, then exercise the public native update.
        self.install(self.secondary, "0.160.0")
        before_packages = tree_hash(self.primary / "packages/standalone/releases")
        print("Updating only Dodex through its private native installer", flush=True)
        self.execute(self.dodex, ["update"], timeout=900)
        latest_package = Path(record["dodex"]["cli_path"]).resolve().parent.parent
        assert self.execute(self.codex, ["--version"]).stdout.strip() == primary_version
        assert tree_hash(self.primary / "packages/standalone/releases") == before_packages
        assert not (self.secondary / "auth.json").exists()
        # A current release can correctly return success from its update cache
        # while offline. Select the cached older release to require an update.
        self.install(self.secondary, "0.160.0")
        selected_version = self.execute(self.dodex, ["--version"]).stdout
        failed_environment = dict(self.environment, HTTPS_PROXY="http://127.0.0.1:1",
                                  https_proxy="http://127.0.0.1:1", HTTP_PROXY="http://127.0.0.1:1",
                                  http_proxy="http://127.0.0.1:1")
        if os.name == "nt":
            # Windows PowerShell 5 uses its system proxy, not these HTTP_PROXY
            # variables. Exercise a real bootstrap failure without modifying
            # the user's proxy or firewall: native update lacks bootstrap tools.
            failed_environment["PATH"] = str(self.root / "missing-updater-tools")
            # A cached package can be selected without invoking extraction.
            # Remove only the unselected latest package in this disposable home.
            assert latest_package.is_relative_to(self.secondary / "packages/standalone/releases")
            assert latest_package != Path(record["dodex"]["cli_path"]).resolve().parent.parent
            shutil.rmtree(latest_package)
        failed = run([self.dodex, "update"], failed_environment, self.project, timeout=90, check=False)
        # Some vendor releases report the curl failure but return 0 from their
        # curl|sh bootstrap. The public entry must preserve that native status.
        assert failed.returncode != 0 or (os.name != "nt" and "curl:" in failed.stderr), "The unavailable updater unexpectedly succeeded"
        assert self.execute(self.dodex, ["--version"]).stdout == selected_version
        self.execute(self.companion, ["dodex-tui", "--repair"], timeout=900)
        assert tree_hash(self.primary / "packages/standalone/releases") == before_packages
        self.execute(self.dodex, ["update"], timeout=900)
        print("Starting two native daemons with two synthetic accounts", flush=True)
        self.configure(self.primary, "source-account")
        self.configure(self.secondary, "selected-account")
        # The primary keeps its own file credentials and configured database.
        primary_config = self.primary / "config.toml"
        if "cli_auth_credentials_store" not in primary_config.read_text():
            primary_config.write_text('cli_auth_credentials_store="file"\n' + primary_config.read_text())
        for entry in [self.codex, self.dodex]:
            self.execute(entry, ["login", "status"])
            result = self.execute(entry, ["app-server", "daemon", "start"], timeout=900)
            home = self.primary if entry == self.codex else self.secondary
            self.sockets[home] = json.loads(result.stdout)["socketPath"]
        primary = self.client(self.primary)
        secondary = self.client(self.secondary)
        other_terminal = self.client(self.secondary)
        for client, account in [(primary, "source-account"), (secondary, "selected-account"),
                                (other_terminal, "selected-account")]:
            result = client.call(2, "account/read", {})
            assert account + "@example.invalid" in json.dumps(result)
        thread = secondary.call(3, "thread/start", {"cwd": str(self.project), "model": "gpt-5.2",
                                "modelProvider": "fixture", "approvalPolicy": "never", "sandbox": "read-only"})["thread"]["id"]
        # Native start persists its rollout on the first turn. Hold that turn
        # at the loopback server while a second terminal resumes the live task.
        self.server.hold_next_response = True
        secondary.call(10, "turn/start", {"threadId": thread,
                                          "input": [{"type": "text", "text": "first-fixture-marker"}]})
        assert self.server.started.wait(25), "Native turn did not reach the loopback server"
        other_terminal.call(4, "thread/resume", {"threadId": thread})
        assert thread not in json.dumps(primary.call(5, "thread/list", {}))
        print("Checking queue, multi-terminal resume and per-instance daemon stop", flush=True)
        self.execute(self.dodex, ["queue", "--thread", thread, "--message", "queued-fixture-marker"])
        self.server.release.set()
        end = time.monotonic() + 30
        while len(self.server.model_requests()) < 2 and time.monotonic() < end:
            time.sleep(0.1)
        assert len(self.server.model_requests()) >= 2, "Native queue never reached the loopback server"
        requests = self.server.model_requests()
        assert all({k.lower(): v for k, v in request[1].items()}["chatgpt-account-id"] == "selected-account" for request in requests)
        for client in [secondary, other_terminal]:
            client.close()
            self.clients.remove(client)
        self.execute(self.dodex, ["app-server", "daemon", "stop"])
        assert "source-account" in json.dumps(primary.call(6, "account/read", {}))
        self.execute(self.dodex, ["app-server", "daemon", "start"], timeout=900)
        recovered = self.client(self.secondary)
        recovered.call(7, "thread/resume", {"threadId": thread})
        assert thread not in json.dumps(primary.call(8, "thread/list", {}))
        primary.close()
        self.clients.remove(primary)
        self.execute(self.codex, ["app-server", "daemon", "stop"])
        assert "selected-account" in json.dumps(recovered.call(9, "account/read", {}))
        print("PASS: fresh install, repeat repair, native parity, target-only update, two accounts, "
              "independent daemons, multi-terminal resume and queue", flush=True)

    def close(self):
        for client in self.clients:
            try:
                client.close()
            except OSError:
                pass
        for name in ["codex", "dodex"]:
            entry = getattr(self, name, None)
            if entry:
                self.execute(entry, ["app-server", "daemon", "stop"], timeout=30, check=False)
        self.server.close()
        self.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--companion", type=Path, required=True)
    parser.add_argument("--primary-release", default="0.159.3")
    parser.add_argument("--work-dir", type=Path, help="Keep the disposable synthetic fixture for diagnostics")
    parser.add_argument("--preflight-only", action="store_true", help="Check the native Windows host without downloads")
    parser.add_argument("--ci-log-dir", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    saved_path = None
    if os.name == "nt":
        import ctypes
        assert not ctypes.windll.shell32.IsUserAnAdmin(), "Native Windows acceptance requires a standard user"
        # Native daemon lifecycle needs a host that permits detached children.
        # Check the disposable host before spending time downloading packages.
        probe = subprocess.run([sys.executable, "-c", "pass"],
                               creationflags=subprocess.CREATE_BREAKAWAY_FROM_JOB | subprocess.DETACHED_PROCESS,
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               timeout=15)
        assert probe.returncode == 0, "The Windows native acceptance host cannot launch detached children"
        print("PASS: standard-user native host permits detached children", flush=True)
        if args.preflight_only:
            return
        saved_path = subprocess.check_output(["powershell.exe", "-NoProfile", "-Command",
            "[Environment]::GetEnvironmentVariable('Path','User')"], text=True).rstrip("\r\n")
    elif args.preflight_only:
        parser.error("The host preflight is Windows-only")
    try:
        if args.work_dir:
            args.work_dir.mkdir(parents=True, exist_ok=True)
        short_temp = Path(os.environ["SystemRoot"]) / "Temp" if os.name == "nt" else Path("/tmp")
        context = nullcontext(str(args.work_dir)) if args.work_dir else tempfile.TemporaryDirectory(prefix="ac-tui-", dir=short_temp)
        with context as directory:
            acceptance = Acceptance(args.companion.resolve(strict=True), Path(directory).resolve(), args.primary_release)
            try:
                acceptance.check()
            except Exception:
                acceptance.stderr.seek(0)
                print(acceptance.stderr.read()[-6000:], file=sys.stderr)
                raise
            finally:
                acceptance.close()
    finally:
        if saved_path is not None:
            environment = dict(os.environ, AC_TUI_RESTORE_PATH=saved_path)
            run(["powershell.exe", "-NoProfile", "-Command", "[Environment]::SetEnvironmentVariable('Path',$env:AC_TUI_RESTORE_PATH,'User')"], environment, Path.cwd())


if __name__ == "__main__":
    if "--ci-log-dir" in sys.argv:
        index = sys.argv.index("--ci-log-dir")
        directory = Path(sys.argv[index + 1]).resolve(strict=True)
        if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true":
            raise RuntimeError("File logging is only for the disposable Windows CI host")
        result = 0
        with (directory / "stdout.log").open("w", encoding="utf-8") as stdout, (directory / "stderr.log").open("w", encoding="utf-8") as stderr:
            with redirect_stdout(stdout), redirect_stderr(stderr):
                try:
                    main()
                except Exception:
                    traceback.print_exc()
                    result = 1
        sys.exit(result)
    else:
        main()
