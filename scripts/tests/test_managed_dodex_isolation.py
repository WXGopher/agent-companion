"""Exercise the shipped standalone adapter with synthetic binding data only."""
import json
from pathlib import Path
import unittest


TEMPLATE = Path(__file__).resolve().parents[2] / "crates/agent-companion/src/software_updates/dodex-wrapper.py"


class ManagedDodexIsolation(unittest.TestCase):
    def setUp(self):
        self.binding = dict(package="/synthetic/package", profile_home="/synthetic/home",
                            sqlite_home="/synthetic/sqlite", desktop_data="/synthetic/desktop",
                            log_dir="/synthetic/logs")
        self.namespace = {"__name__": "adapter_test"}
        source = TEMPLATE.read_text().replace("__BINDING_JSON__", repr(json.dumps(self.binding)))
        exec(compile(source, str(TEMPLATE), "exec"), self.namespace)

    def test_prompt_after_end_of_options_is_not_configuration(self):
        for args in (["--", "-c", 'sqlite_home="literal prompt"'],
                     ["resume", "synthetic-id", "--", '--config=log_dir="prompt"'],
                     ["exec", "--", '-ccli_auth_credentials_store="keyring"']):
            with self.subTest(args=args):
                self.assertEqual(self.namespace["native_args"](args)[7:], args)

    def test_values_are_not_reinterpreted_as_options(self):
        for args in (["--model", '-clog_dir="literal model"'],
                     ["--model=-clog_dir=literal", "resume", "synthetic-id"],
                     ["exec", "--output-last-message", '-clog_dir="filename"'],
                     ["--config", 'model="sqlite_home=literal"']):
            with self.subTest(args=args):
                self.assertEqual(self.namespace["native_args"](args)[7:], args)

    def test_global_options_after_subcommands_and_profile_overrides_are_blocked(self):
        for args in (["resume", "synthetic-id", "-c", 'log_dir="/bad"'],
                     ["-hcsqlite_home=\"/bad\""],
                     ["--config", 'profiles.work.sqlite_home="/bad"'],
                     ["--config", 'profiles."work".cli_auth_credentials_store="keyring"'],
                     ["--config", 'profiles={work={log_dir="/bad"}}']):
            with self.subTest(args=args):
                with self.assertRaises(ValueError):
                    self.namespace["native_args"](args)

    def test_filter_keeps_containment_and_removes_account_session_runtime(self):
        keep = {"HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "CODEX_SANDBOX",
                "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_NETWORK_PROXY_ACTIVE", "CODEX_CA_CERTIFICATE", "PATH", "TERM"}
        remove = {"CODEX_HOME", "CODEX_THREAD_ID", "CODEX_CONFIG_FILE", "CODEX_CLI_PATH",
                  "OPENAI_API_KEY", "OPENAI_ACCESS_TOKEN", "CHATGPT_ACCESS_TOKEN",
                  "OPENAI_IDENTITY_TOKEN_FILE", "ELECTRON_RUN_AS_NODE", "NODE_OPTIONS",
                  "DYLD_INSERT_LIBRARIES", "LD_PRELOAD"}
        inherited = {name: "synthetic" for name in keep | remove}
        self.assertEqual(set(self.namespace["clean_environment"](inherited)), keep)
        self.assertEqual(set(inherited), keep | remove)

    def test_native_prompt_and_service_configuration_are_preserved(self):
        for args in (["--future-native-option", "value", "app"],
                     ["--", "app"], ["help", "app"], ["exec", "update"]):
            self.assertEqual(self.namespace["route"](args)[0], "native")
        args = ["-c", "profiles.work={mcp_servers={helper={env={log_dir='service'}}}}"]
        self.assertEqual(self.namespace["native_args"](args)[7:], args)


if __name__ == "__main__":
    unittest.main()
