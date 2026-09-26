"""Offline CLI entry tests; no credentials, network, or interactive sessions."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "prepare-codex-cli.py"
SPEC = importlib.util.spec_from_file_location("prepare_primary_codex_cli", SCRIPT)
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)


class PrimaryCliIsolation(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.user_home = Path(self.temporary.name) / "user with spaces"
        self.target = self.user_home / ".codex/packages/standalone/current/bin/codex"
        self.target.parent.mkdir(parents=True)
        self.target.write_text('''#!/usr/bin/python3 -B
import json, os, signal, sys
print(json.dumps({"argv":sys.argv,"env":dict(os.environ),"cwd":os.getcwd(),"pid":os.getpid()}), flush=True)
if os.environ.get("TEST_WAIT_SIGNAL"):
    signal.pause()
sys.exit(int(os.environ.get("TEST_EXIT", "0")))
''')
        self.target.chmod(0o755)
        self.wrapper = self.user_home / "codex-entry"
        self.wrapper.write_bytes(PREPARE.render_wrapper(self.user_home, self.target))
        self.wrapper.chmod(0o755)
        self.workspace = self.user_home / "shared workspace"
        self.workspace.mkdir()
        self.env = {
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(self.user_home),
            "TERM": "xterm-256color", "HTTPS_PROXY": "http://127.0.0.1:12345",
            "CODEX_HOME": "/synthetic/second", "CODEX_SQLITE_HOME": "/synthetic/second/sqlite",
            "CODEX_CLI_PATH": "/synthetic/second/codex", "CODEX_THREAD_ID": "synthetic-session",
            "CODEX_MANAGED_PACKAGE_ROOT": "/synthetic/second/package",
            "CODEX_INSTALL_DIR": "/synthetic/second/bin", "CODEX_API_KEY": "synthetic-key",
            "OPENAI_API_KEY": "synthetic-key", "OPENAI_ACCESS_TOKEN": "synthetic-token",
            "OPENAI_BASE_URL": "https://example.invalid", "CHATGPT_ACCESS_TOKEN": "synthetic-token",
            "OPENAI_IDENTITY_TOKEN_FILE": "/synthetic/identity-token",
            "OPENAI_WORKLOAD_IDENTITY_CONTEXT": "synthetic-workload",
            "OPENAI_FEDERATION_RULE_ID": "synthetic-federation",
            "CODEX_SANDBOX": "seatbelt", "CODEX_SANDBOX_NETWORK_DISABLED": "1",
            "CODEX_NETWORK_PROXY_ACTIVE": "1", "CODEX_NETWORK_ALLOW_LOCAL_BINDING": "0",
            "ELECTRON_RUN_AS_NODE": "1", "DYLD_INSERT_LIBRARIES": "/synthetic/library",
            "NODE_OPTIONS": "--synthetic", "NODE_PATH": "/synthetic/node",
            "UNRELATED_SETTING": "kept",
        }

    def test_native_arguments_cwd_path_and_exit_are_preserved(self):
        args = ["resume", "--last", "--", "literal prompt with spaces", "-c", "model=example"]
        self.env["TEST_EXIT"] = "37"
        result = subprocess.run([str(self.wrapper)] + args, cwd=self.workspace, env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 37, result.stderr)
        captured = json.loads(result.stdout)
        self.assertEqual(captured["argv"], [str(self.target)] + args)
        self.assertEqual(Path(captured["cwd"]), self.workspace.resolve())
        for key in ("PATH", "HTTPS_PROXY", "TERM", "UNRELATED_SETTING", "CODEX_SANDBOX",
                    "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_NETWORK_PROXY_ACTIVE",
                    "CODEX_NETWORK_ALLOW_LOCAL_BINDING"):
            self.assertEqual(captured["env"][key], self.env[key])
        self.assertEqual(captured["env"]["CODEX_HOME"], str(self.user_home / ".codex"))
        for key in ("CODEX_SQLITE_HOME", "CODEX_THREAD_ID", "CODEX_CLI_PATH", "CODEX_API_KEY",
                    "CODEX_MANAGED_PACKAGE_ROOT", "CODEX_INSTALL_DIR", "OPENAI_API_KEY",
                    "OPENAI_ACCESS_TOKEN", "OPENAI_BASE_URL", "CHATGPT_ACCESS_TOKEN",
                    "OPENAI_IDENTITY_TOKEN_FILE", "OPENAI_WORKLOAD_IDENTITY_CONTEXT",
                    "OPENAI_FEDERATION_RULE_ID",
                    "ELECTRON_RUN_AS_NODE", "DYLD_INSERT_LIBRARIES", "NODE_OPTIONS", "NODE_PATH"):
            self.assertNotIn(key, captured["env"])

    def test_exec_keeps_pid_and_signal_termination(self):
        self.env["TEST_WAIT_SIGNAL"] = "1"
        process = subprocess.Popen([str(self.wrapper)], cwd=self.workspace, env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            captured = json.loads(process.stdout.readline())
            self.assertEqual(captured["pid"], process.pid)
            process.terminate()
            self.assertEqual(process.wait(timeout=10), -signal.SIGTERM)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdout.close()
            process.stderr.close()

    def test_only_original_vendor_target_is_accepted(self):
        for target in (self.user_home / ".codex-second/bin/codex", Path("relative/codex"),
                       self.user_home / ".codex/packages/standalone/current/bin/other"):
            with self.assertRaises(ValueError):
                PREPARE.render_wrapper(self.user_home, target)
        self.assertEqual(PREPARE.render_wrapper(self.user_home, self.target), self.wrapper.read_bytes())

    def test_staging_never_overwrites_existing_entry(self):
        original = self.wrapper.read_bytes()
        result = subprocess.run(["/usr/bin/python3", str(SCRIPT), "--user-home", str(self.user_home),
                                 "--output", str(self.wrapper)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.wrapper.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
