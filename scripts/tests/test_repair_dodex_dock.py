import copy
import importlib.util
from pathlib import Path
import plistlib
import subprocess
import tempfile
import traceback
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "repair-dodex-dock.py"
spec = importlib.util.spec_from_file_location("repair_dodex_dock", SCRIPT)
dock = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dock)


def tile(url, label="Codex B Runtime"):
    return {
        "GUID": 123456,
        "tile-type": "file-tile",
        "tile-data": {
            "file-data": {"_CFURLString": url, "_CFURLStringType": 15},
            "file-label": label,
            "bundle-identifier": "old.runtime.identifier",
            "book": b"old runtime bookmark",
            "file-mod-date": 1234,
            "parent-mod-date": 5678,
            "file-type": 41,
            "custom": {"keep": b"original"},
        },
    }


class TileTests(unittest.TestCase):
    def test_exact_local_urls_and_percent_encoded_spaces(self):
        for url in (
            "file:///Applications/Codex%20B%20Runtime.app/",
            "file:///Applications/Codex B.app",
            "file:///Applications/.Dodex/Dodex.app/",
            "file://localhost/Applications/Dodex.app",
        ):
            with self.subTest(url=url):
                self.assertTrue(dock.matches_app_url(url))

    def test_boundaries_and_unfamiliar_urls_are_not_owned(self):
        for url in (
            None, 4, "/Applications/Dodex.app", "https://example.com/Applications/Dodex.app",
            "file://example.com/Applications/Dodex.app", "file://localhost:80/Applications/Dodex.app",
            "file:///Applications/Dodex.app.bak", "file:///Applications/Dodex.app/Contents/MacOS/Codex",
            "file:///Users/me/Applications/Dodex.app", "file:///Applications/Dodex.app//",
            "file:///Applications/../Applications/Dodex.app", "file:///Applications/dodex.app",
            "file:///Applications/Dodex.app?x=1", "file:///Applications/Dodex.app#fragment",
            "file:///Applications/Dodex.app%00", "file:///Applications/%FFDodex.app",
            " file:///Applications/Dodex.app", "file:///Applications/Codex%2520B.app",
            "file:///Applications/Do\ndex.app", "file:///Applications/Do\tdex.app",
        ):
            with self.subTest(url=url):
                self.assertFalse(dock.matches_app_url(url))

    def test_preserves_guid_order_and_all_unrelated_fields(self):
        preferences = {"persistent-apps": [
            tile("file:///Applications/Codex.app", "Codex"),
            tile("file:///Applications/Codex%20B%20Runtime.app/"),
            {"tile-type": "spacer-tile", "GUID": 99},
        ], "autohide": True, "persistent-others": [{"untouched": True}]}
        original = copy.deepcopy(preferences)
        actual, matched, changed = dock.repaired_tiles(preferences)
        expected = [original["persistent-apps"][0], original["persistent-apps"][2]]
        self.assertEqual(actual, expected)
        self.assertEqual(matched, [1])
        self.assertEqual(changed, [1])
        self.assertEqual(preferences, original)

    def test_idempotence_and_no_append_when_absent(self):
        actual, _, _ = dock.repaired_tiles({"persistent-apps": [tile("file:///Applications/Dodex.app/")]})
        again, matched, changed = dock.repaired_tiles({"persistent-apps": actual})
        self.assertEqual(again, actual)
        self.assertEqual(matched, [])
        self.assertEqual(changed, [])
        self.assertEqual(dock.repaired_tiles({}), ([], [], []))
        unrelated = [tile("file:///Applications/Codex.app", "Dodex")]
        self.assertEqual(dock.repaired_tiles({"persistent-apps": unrelated}), (unrelated, [], []))

    def test_serialization_preserves_property_list_data(self):
        original = tile("file:///Applications/Dodex.app/")
        value = dock.serialize_tile(original)
        self.assertEqual(plistlib.loads(("<plist>" + value + "</plist>").encode()), original)

    def test_removes_all_launcher_and_runtime_duplicates_with_bookmarks(self):
        urls = ["file:///Applications/Dodex.app", "file:///Applications/Codex%20B.app/",
                "file:///Applications/.Dodex/Dodex.app/", "file:///Applications/Codex%20B%20Runtime.app/"]
        actual, matched, changed = dock.repaired_tiles({"persistent-apps": [tile(url) for url in urls]})
        self.assertEqual(actual, [])
        self.assertEqual(matched, [0, 1, 2, 3])
        self.assertEqual(changed, matched)

    def test_does_not_match_primary_app_by_shared_vendor_bundle_identifier(self):
        primary = tile("file:///Applications/ChatGPT.app/", "Codex")
        primary["tile-data"]["bundle-identifier"] = "com.openai.codex"
        self.assertEqual(dock.repaired_tiles({"persistent-apps": [primary]}), ([primary], [], []))

    def test_malformed_tiles_preserved_and_non_array_rejected(self):
        original = [None, {}, {"tile-data": "unexpected"}]
        self.assertEqual(dock.repaired_tiles({"persistent-apps": original}), (original, [], []))
        with self.assertRaises(ValueError):
            dock.repaired_tiles({"persistent-apps": {}})

    def test_recent_shortcuts_use_the_same_exact_path_policy(self):
        unrelated = tile("file:///Applications/Codex.app/", "Codex")
        original = [unrelated, tile("file:///Applications/.Dodex/Dodex.app/"),
                    tile("file:///Applications/Codex%20B%20Runtime.app/"),
                    tile("file:///Applications/Other.app/", "Dodex")]
        actual, matched, changed = dock.repaired_tiles({"recent-apps": original}, "recent-apps")
        self.assertEqual(actual, [original[0], original[3]])
        self.assertEqual(matched, [1, 2])
        self.assertEqual(changed, matched)
        self.assertEqual(dock.repaired_tiles({"recent-apps": actual}, "recent-apps"), (actual, [], []))


class OperationTests(unittest.TestCase):
    def setUp(self):
        self.preferences = {"persistent-apps": [tile("file:///Applications/Codex%20B.app/")], "autohide": True}
        self.raw = plistlib.dumps(self.preferences)

    def test_check_never_writes_or_creates_backup(self):
        with patch.object(dock, "export_preferences", return_value=(self.raw, self.preferences)), \
                patch.object(dock, "save_backup") as backup, patch.object(dock.subprocess, "run") as run:
            result = dock.repair()
        self.assertEqual(result["status"], "changes-needed")
        backup.assert_not_called()
        run.assert_not_called()

    def test_apply_writes_only_array_and_preserves_exact_private_backup(self):
        repaired, _, _ = dock.repaired_tiles(self.preferences)
        actual = {**self.preferences, "persistent-apps": repaired}
        snapshots = [(self.raw, self.preferences)] * 3 + [(plistlib.dumps(actual), actual)]
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(dock, "export_preferences", side_effect=snapshots), \
                patch.object(dock.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            result = dock.repair(apply=True, backup_dir=Path(directory) / "private")
            backup = Path(result["backup"])
            self.assertEqual(backup.read_bytes(), self.raw)
            self.assertEqual(backup.stat().st_mode & 0o777, 0o600)
            self.assertEqual(backup.parent.stat().st_mode & 0o777, 0o700)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[0][:5], ["/usr/bin/defaults", "write", "com.apple.dock", "persistent-apps", "-array"])
        self.assertEqual(len(commands[0]), 5)
        self.assertEqual(commands[1], ["/usr/bin/killall", "Dock"])
        self.assertEqual(result["status"], "repaired")

    def test_concurrent_edit_aborts_before_write(self):
        changed = {**self.preferences, "autohide": False}
        with patch.object(dock, "export_preferences", side_effect=[(self.raw, self.preferences), (b"new", changed)]), \
                patch.object(dock, "save_backup") as backup, patch.object(dock.subprocess, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "changed during inspection"):
                dock.repair(apply=True)
        backup.assert_not_called()
        run.assert_not_called()

    def test_apply_cleans_both_arrays_and_preserves_global_preferences(self):
        primary = tile("file:///Applications/Codex.app/", "Codex")
        preferences = {**self.preferences, "recent-apps": [primary, tile("file:///Applications/.Dodex/Dodex.app/")],
                       "show-recents": True, "persistent-others": [{"keep": 1}]}
        raw = plistlib.dumps(preferences)
        first = {**preferences, "persistent-apps": []}
        actual = {**first, "recent-apps": [primary]}
        snapshots = [(raw, preferences)] * 3 + [(plistlib.dumps(first), first)] * 2 + [(plistlib.dumps(actual), actual)]
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(dock, "export_preferences", side_effect=snapshots), \
                patch.object(dock.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            result = dock.repair(apply=True, backup_dir=directory)
            self.assertEqual(Path(result["backup"]).read_bytes(), raw)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual([command[3] for command in commands[:-1]], ["persistent-apps", "recent-apps"])
        self.assertEqual(commands[-1], ["/usr/bin/killall", "Dock"])
        self.assertEqual(result["removed_by_key"], {"persistent-apps": 1, "recent-apps": 1})
        self.assertEqual(result["changed_tiles"], 2)
        # The recent array serializes the complete surviving tile including GUID.
        self.assertEqual(commands[1][5:], [dock.serialize_tile(primary)])

    def test_recent_only_repair_does_not_create_missing_persistent_array(self):
        preferences = {"recent-apps": [tile("file:///Applications/Dodex.app/")], "show-recents": True}
        raw = plistlib.dumps(preferences)
        actual = {**preferences, "recent-apps": []}
        snapshots = [(raw, preferences)] * 3 + [(plistlib.dumps(actual), actual)]
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(dock, "export_preferences", side_effect=snapshots), \
                patch.object(dock.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            result = dock.repair(apply=True, backup_dir=directory)
        self.assertEqual(run.call_args_list[0].args[0][3:5], ["recent-apps", "-array"])
        self.assertEqual(run.call_count, 2)
        self.assertEqual(result["removed_by_key"], {"persistent-apps": 0, "recent-apps": 1})

    def test_invalid_recent_array_aborts_before_any_mutation(self):
        preferences = {**self.preferences, "recent-apps": {"unexpected": True}}
        with patch.object(dock, "export_preferences", return_value=(b"original", preferences)), \
                patch.object(dock, "save_backup") as backup, patch.object(dock.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "recent-apps is not an array"):
                dock.repair(apply=True)
        backup.assert_not_called()
        run.assert_not_called()

    def test_concurrent_edit_between_arrays_stops_and_reports_partial_write(self):
        preferences = {**self.preferences, "recent-apps": [tile("file:///Applications/Dodex.app/")]}
        raw = plistlib.dumps(preferences)
        first = {**preferences, "persistent-apps": []}
        concurrent = {**first, "recent-apps": [tile("file:///Applications/New.app/")]}
        snapshots = [(raw, preferences)] * 3 + [(b"first", first), (b"concurrent", concurrent)]
        with patch.object(dock, "export_preferences", side_effect=snapshots), \
                patch.object(dock, "save_backup", return_value=Path("backup.plist")), \
                patch.object(dock.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            with self.assertRaisesRegex(RuntimeError, "Keys written: persistent-apps.*changed before writing"):
                dock.repair(apply=True)
        self.assertEqual(run.call_count, 1)

    def test_edit_after_backup_aborts_before_write(self):
        changed = {**self.preferences, "autohide": False}
        with patch.object(dock, "export_preferences", side_effect=[(self.raw, self.preferences)] * 2 + [(b"new", changed)]), \
                patch.object(dock, "save_backup", return_value=Path("backup.plist")), \
                patch.object(dock.subprocess, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "changed before writing"):
                dock.repair(apply=True)
        run.assert_not_called()

    def test_failed_readback_does_not_restart_dock(self):
        with patch.object(dock, "export_preferences", return_value=(self.raw, self.preferences)), \
                patch.object(dock, "save_backup", return_value=Path("backup.plist")), \
                patch.object(dock.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            with self.assertRaisesRegex(RuntimeError, "could not be verified.*backup.plist"):
                dock.repair(apply=True)
        self.assertEqual(run.call_count, 1)

    def test_failed_defaults_write_does_not_expose_private_tile_or_bookmark(self):
        sentinel = "private-bookmark-sentinel"
        private = tile("file:///Applications/Private.app", sentinel)
        private["tile-data"]["book"] = sentinel.encode()
        preferences = {"persistent-apps": [*self.preferences["persistent-apps"], private]}
        raw = plistlib.dumps(preferences)
        failed_command = ["/usr/bin/defaults", "write", "com.apple.dock", "persistent-apps",
                          "-array", dock.serialize_tile(private)]
        with patch.object(dock, "export_preferences", return_value=(raw, preferences)), \
                patch.object(dock, "save_backup", return_value=Path("backup.plist")), \
                patch.object(dock.subprocess, "run", side_effect=subprocess.CalledProcessError(7, failed_command)) as run:
            with self.assertRaisesRegex(RuntimeError, "defaults command failed with exit code 7") as error:
                dock.repair(apply=True)
        self.assertNotIn(sentinel, str(error.exception))
        self.assertNotIn("Private.app", str(error.exception))
        self.assertNotIn("<dict>", str(error.exception))
        self.assertNotIn(sentinel, "".join(traceback.format_exception(
            type(error.exception), error.exception, error.exception.__traceback__)))
        self.assertEqual(run.call_count, 1)

    def test_default_cli_is_read_only(self):
        with patch.object(dock.sys, "platform", "darwin"), patch.object(dock, "repair", return_value={}) as repair, \
                patch("builtins.print"):
            self.assertEqual(dock.main([]), 0)
        self.assertFalse(repair.call_args.kwargs["apply"])


if __name__ == "__main__":
    unittest.main()
