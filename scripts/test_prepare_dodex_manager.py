"""Pure regression checks: no live manager import, processes, or app launches."""

import contextlib
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import shutil
import stat
import tempfile
import types
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location(
    "prepare_dodex_manager", Path(__file__).with_name("prepare-dodex-manager.py"))
repair = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(repair)


class RepairTests(unittest.TestCase):
    def setUp(self):
        self.snippets = {
            name: (repair.RESOURCES / (name + ".old.txt")).read_bytes()
            for name in repair.PATCHES
        }
        self.synthetic = b"\n".join(self.snippets.values())

    def desktop_manager(self, stdout=b"", returncode=0):
        method = (repair.RESOURCES / "legacy-desktop-method.new.txt").read_text()
        method = method.removesuffix("    def stopped(self):\n")
        command = Mock(return_value=types.SimpleNamespace(stdout=stdout, returncode=returncode))
        namespace = {"command": command, "ManagerError": RuntimeError,
                     "os": types.SimpleNamespace(getpid=lambda: 99)}
        exec("class Manager:\n" + method, namespace)
        manager = namespace["Manager"]()
        manager.paths = types.SimpleNamespace(runtime=Path("/Applications/Codex B Runtime.app"))
        return manager, command

    def test_tui_helpers_other_profiles_and_prefixes_do_not_block_desktop(self):
        output = b"""10 /Applications/Codex B Runtime.app/Contents/Resources/codex
11 /Applications/Codex B Runtime.app/Contents/Frameworks/Codex Helper.app/Contents/MacOS/Codex Helper
12 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT
13 /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT-backup
14 /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT --some-argument
15 /Applications/Dodex.app/Contents/MacOS/Dodex
"""
        manager, command = self.desktop_manager(output)
        self.assertEqual(manager.desktop_pids(), [])
        command.assert_called_once_with(["/bin/ps", "-axo", "pid=,comm="], timeout=10)

    def test_exact_desktop_executable_with_spaces_blocks_launch(self):
        manager, _ = self.desktop_manager(
            b" 42 /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT\n"
            b"99 /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT\n"
            b"invalid /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT\n")
        self.assertEqual(manager.desktop_pids(), [42])

    def test_failed_ps_fails_closed(self):
        manager, _ = self.desktop_manager(returncode=1)
        with self.assertRaisesRegex(RuntimeError, "Cannot establish"):
            manager.desktop_pids()

    def test_command_exception_fails_closed(self):
        manager, command = self.desktop_manager()
        command.side_effect = RuntimeError("ps timed out")
        with self.assertRaisesRegex(RuntimeError, "ps timed out"):
            manager.desktop_pids()

    def test_shared_transform_changes_launch_only(self):
        guard = b"    def active_pids(self, include_launcher=True):\n        return [1]\n"
        stopped_body = b"        pids = self.active_pids()\n"
        fixture = guard + self.snippets["legacy-desktop-method"] + stopped_body
        fixture += self.snippets["legacy-desktop-launch"]
        output = repair.replace_fragments(fixture)
        self.assertIn(guard, output)
        self.assertIn(self.snippets["legacy-desktop-method"] + stopped_body, output)
        self.assertIn(b"            if self.desktop_pids():\n", output)

    def test_missing_or_ambiguous_fragments_are_rejected(self):
        for fixture in (b"", self.synthetic + self.synthetic):
            with self.assertRaisesRegex(ValueError, "exactly one"):
                repair.replace_fragments(fixture)

    def test_unknown_source_rejected_before_transform(self):
        with patch.object(repair, "replace_fragments") as transform:
            with self.assertRaisesRegex(ValueError, "Unrecognized manager"):
                repair.prepare(self.synthetic)
            transform.assert_not_called()

    def test_patch_hash_mismatch_rejected(self):
        with patch.object(repair, "SOURCE_SHA256", hashlib.sha256(self.synthetic).hexdigest()):
            with self.assertRaisesRegex(ValueError, "audited result"):
                repair.prepare(self.synthetic)

    def test_default_is_read_only_and_output_never_overwrites(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.py"
            output = Path(directory) / "output.py"
            source.write_bytes(b"fixture")
            with patch.object(repair, "prepare", return_value=b"patched"), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(repair.main(["--source", str(source)]), 0)
                self.assertEqual(list(Path(directory).iterdir()), [source])
                self.assertEqual(repair.main(["--source", str(source), "--output", str(output)]), 0)
                self.assertEqual(output.read_bytes(), b"patched")
                with contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(repair.main(["--source", str(source), "--output", str(source)]), 1)
                self.assertEqual(source.read_bytes(), b"fixture")


class ConsolidationTests(unittest.TestCase):
    def resource(self, name):
        return (repair.RESOURCES / (name + ".new.txt")).read_text()

    def methods(self, names, command):
        code = "".join(self.resource(name) for name in names)
        code = code.removesuffix("    def stopped(self):\n")
        code = code.removesuffix("    def audit(self, event, **details):\n")
        namespace = {"command": command, "ManagerError": RuntimeError,
                     "os": types.SimpleNamespace(getpid=lambda: 99), "Path": Path}
        exec("class Manager:\n" + code, namespace)
        manager = namespace["Manager"]()
        manager.paths = types.SimpleNamespace(runtime=Path("/Applications/.Dodex/Dodex.app"),
                                             launcher=Path("/Applications/Dodex.app"))
        return manager, namespace

    def test_paths_are_one_canonical_runtime_and_real_launcher(self):
        paths = eval("[" + self.resource("consolidated-paths") + "]", {"Path": Path})
        self.assertEqual(paths, [Path("/Applications/.Dodex/Dodex.app"), Path("/Applications/Dodex.app")])

    def test_maintenance_sees_canonical_and_draining_tui_and_helpers(self):
        command = Mock(return_value=types.SimpleNamespace(returncode=0, stdout=b"""10 /Applications/.Dodex/Dodex.app/Contents/Resources/codex
11 /Applications/Codex B Runtime.app/Contents/Resources/cua_node/bin/node
12 /Applications/.Dodex/Dodex.app/Contents/MacOS/ChatGPT
13 /Applications/Dodex.app/Contents/MacOS/Dodex
14 /Applications/ChatGPT.app/Contents/Resources/codex
"""))
        manager, _ = self.methods(["consolidated-maintenance"], command)
        self.assertEqual(manager.active_pids(), [10, 11, 12, 13])
        self.assertEqual(manager.active_pids(include_launcher=False), [10, 11, 12])
        command.return_value.returncode = 1
        with self.assertRaisesRegex(RuntimeError, "Cannot establish"):
            manager.active_pids()

    def test_desktop_detects_both_desktops_but_never_tui(self):
        text = self.resource("legacy-desktop-method").encode()
        text = repair.replace_fragments(text, ("consolidated-desktop-target", "consolidated-desktop-match"))
        text = text.decode().removesuffix("    def stopped(self):\n")
        command = Mock(return_value=types.SimpleNamespace(returncode=0, stdout=b"""10 /Applications/.Dodex/Dodex.app/Contents/Resources/codex
11 /Applications/Codex B Runtime.app/Contents/Resources/codex
12 /Applications/.Dodex/Dodex.app/Contents/MacOS/ChatGPT
13 /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT
"""))
        namespace = {"command": command, "ManagerError": RuntimeError,
                     "os": types.SimpleNamespace(getpid=lambda: 99)}
        exec("class Manager:\n" + text, namespace)
        manager = namespace["Manager"]()
        manager.paths = types.SimpleNamespace(runtime=Path("/Applications/.Dodex/Dodex.app"))
        self.assertEqual(manager.desktop_pids(), [12, 13])

    def test_environment_clears_identity_and_preserves_containment_metadata(self):
        namespace = {"os": os}
        exec(self.resource("consolidated-auth"), namespace)
        exec(self.resource("consolidated-environment"), namespace)
        paths = types.SimpleNamespace(home=Path("/second"), data=Path("/data"), cli=Path("/canonical/codex"))
        inherited = {"CODEX_HOME": "/primary", "CODEX_THREAD_ID": "wrong",
                     "OPENAI_API_KEY": "synthetic", "ELECTRON_RUN_AS_NODE": "1",
                     "OPENAI_IDENTITY_TOKEN_FILE": "/synthetic/token",
                     "OPENAI_WORKLOAD_IDENTITY_CONTEXT": "synthetic",
                     "OPENAI_FEDERATION_RULE_ID": "synthetic",
                     "DYLD_INSERT_LIBRARIES": "synthetic", "NODE_OPTIONS": "synthetic",
                     "CODEX_SANDBOX": "seatbelt", "CODEX_SANDBOX_NETWORK_DISABLED": "1",
                     "CODEX_NETWORK_POLICY": "brokered", "PATH": "/test/bin"}
        result = namespace["isolated_environment"](paths, inherited)
        self.assertEqual(result["CODEX_HOME"], "/second")
        self.assertEqual(result["CODEX_CLI_PATH"], "/canonical/codex")
        for key in ("CODEX_THREAD_ID", "ELECTRON_RUN_AS_NODE", "DYLD_INSERT_LIBRARIES", "NODE_OPTIONS", *namespace["AUTH_ENV"]):
            self.assertNotIn(key, result)
        for key in ("CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_NETWORK_POLICY", "PATH"):
            self.assertEqual(result[key], inherited[key])

    def test_hash_gates_both_known_revisions_and_idempotent_output(self):
        old, narrow, canonical = b"old", b"narrow", b"pass\n"
        with patch.multiple(repair, SOURCE_SHA256=hashlib.sha256(old).hexdigest(),
                            PATCHED_SHA256=hashlib.sha256(narrow).hexdigest(),
                            CONSOLIDATED_SHA256=hashlib.sha256(canonical).hexdigest()), \
                patch.object(repair, "prepare", return_value=narrow) as prepare, \
                patch.object(repair, "replace_fragments", return_value=canonical) as transform:
            self.assertEqual(repair.consolidate(old), canonical)
            prepare.assert_called_once_with(old)
            self.assertEqual(repair.consolidate(narrow), canonical)
            self.assertEqual(repair.consolidate(canonical), canonical)
            self.assertEqual(transform.call_count, 2)
            with self.assertRaisesRegex(ValueError, "Unrecognized"):
                repair.consolidate(b"unknown")

    def signature_namespace(self):
        namespace = {"Path": Path, "stat": stat, "tempfile": tempfile,
                     "ManagerError": RuntimeError, "command": Mock()}
        exec(self.resource("consolidated-signature-helper").removesuffix("def verify_application(app):\n"), namespace)
        return namespace

    def test_icon_verification_authenticates_identity_and_closes_clone_on_tamper(self):
        for final_valid in (True, False):
            with tempfile.TemporaryDirectory() as directory:
                app = Path(directory) / "Original.app"
                app.mkdir()
                (app / "Icon\r").touch()
                (app / "Contents").mkdir()
                (app / "Contents/nested-fork-sentinel").write_text("preserve")
                namespace = self.signature_namespace()
                calls, clones = [], []

                def command(arguments):
                    calls.append(arguments)
                    if arguments[0] == "/bin/cp":
                        clone = Path(arguments[-1])
                        clones.append(clone)
                        shutil.copytree(app, clone)
                    return types.SimpleNamespace(returncode=0)

                def official(path, strict):
                    if path == app:
                        return not strict
                    self.assertTrue(strict)
                    self.assertFalse((path / "Icon\r").exists())
                    self.assertEqual((path / "Contents/nested-fork-sentinel").read_text(), "preserve")
                    return final_valid

                namespace.update(command=command, official_signature_valid=Mock(side_effect=official),
                                 standard_custom_icon=Mock(return_value=True))
                self.assertEqual(namespace["verify_original_signature"](app), final_valid)
                self.assertTrue((app / "Icon\r").is_file())
                self.assertFalse(clones[0].parent.exists())
                self.assertEqual(calls[0][:2], ["/bin/cp", "-cR"])
                self.assertEqual(calls[1][:3], ["/usr/bin/xattr", "-d", "com.apple.FinderInfo"])
                self.assertEqual(len(calls), 2)

    def test_unsigned_or_nonstandard_icon_never_gets_a_clone(self):
        for metadata, ordinary in ((False, True), (True, False)):
            namespace = self.signature_namespace()
            namespace["standard_custom_icon"] = Mock(return_value=metadata)
            namespace["official_signature_valid"] = Mock(side_effect=[False, ordinary])
            self.assertFalse(namespace["verify_original_signature"](Path("/synthetic")))
            namespace["command"].assert_not_called()

    def test_strict_signature_command_requires_official_identity(self):
        namespace = self.signature_namespace()
        namespace["command"].return_value.returncode = 0
        self.assertTrue(namespace["official_signature_valid"](Path("/synthetic"), True))
        arguments = namespace["command"].call_args.args[0]
        self.assertIn("--strict", arguments)
        self.assertIn("-R", arguments)
        self.assertIn("2DC432GLL2", arguments[arguments.index("-R") + 1])

    def test_clone_failure_removes_temporary_directory(self):
        namespace = self.signature_namespace()
        namespace["standard_custom_icon"] = Mock(return_value=True)
        namespace["official_signature_valid"] = Mock(side_effect=[False, True])
        namespace["command"].return_value.returncode = 1
        with self.assertRaisesRegex(RuntimeError, "APFS"):
            namespace["verify_original_signature"](Path("/synthetic"))
        clone = Path(namespace["command"].call_args.args[0][-1])
        self.assertFalse(clone.parent.exists())

    def test_custom_icon_rejects_bad_metadata_links_and_nonempty_data(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory)
            icon = app / "Icon\r"
            icon.touch()
            namespace = self.signature_namespace()
            flags = ["00"] * 32
            flags[8] = "04"

            def attributes(arguments):
                value = b"com.apple.ResourceFork\n" if "-px" not in arguments else " ".join(flags).encode()
                return types.SimpleNamespace(returncode=0, stdout=value)

            namespace["command"].side_effect = attributes
            self.assertTrue(namespace["standard_custom_icon"](app))
            flags[8] = "00"
            self.assertFalse(namespace["standard_custom_icon"](app))
            flags[8] = "04"
            flags.pop()
            self.assertFalse(namespace["standard_custom_icon"](app))
            flags.append("00")
            icon.write_text("data")
            self.assertFalse(namespace["standard_custom_icon"](app))
            icon.write_bytes(b"")
            os.link(icon, app / "hardlink")
            self.assertFalse(namespace["standard_custom_icon"](app))
            icon.unlink()
            icon.symlink_to(app / "hardlink")
            self.assertFalse(namespace["standard_custom_icon"](app))

    def retirement_manager(self, root, ps_output=b"", pending=False):
        paths = {"/Applications/.Dodex/legacy-runtime-retirement.json": root / "marker.json",
                 "/Applications/Codex B Runtime.app": root / "Legacy.app",
                 "/Applications/.Dodex/Retired Runtime.app": root / "Retired.app"}
        command = Mock(return_value=types.SimpleNamespace(returncode=0, stdout=ps_output))
        manager, namespace = self.methods(["consolidated-retirement"], command)
        namespace.update(Path=lambda text: paths[text], exists=lambda path: path.exists(),
                         read_json=lambda path: __import__("json").loads(path.read_text()),
                         no_symlink_components=Mock(), verify_application=Mock(), os=os,
                         ctypes=types.SimpleNamespace(c_char_p=object, c_uint=object, c_int=object))
        rename = Mock(side_effect=lambda source, destination, flags: (os.rename(source, destination), 0)[1])
        namespace["DARWIN"] = types.SimpleNamespace(renamex_np=rename)
        manager.pending_journals = Mock(return_value=["journal"] if pending else [])
        marker, source, destination = paths.values()
        marker.write_text(__import__("json").dumps({"schema": 1, "source": str(source), "destination": str(destination)}))
        source.mkdir()
        return manager, namespace, marker, source, destination

    def test_retirement_leaves_active_cli_untouched_then_archives_exclusively(self):
        with tempfile.TemporaryDirectory() as directory:
            manager, namespace, marker, source, destination = self.retirement_manager(Path(directory))
            namespace["command"].return_value.stdout = (str(source) + "/Contents/Resources/codex\n").encode()
            manager.retire_legacy_runtime()
            self.assertTrue(marker.exists() and source.exists())
            namespace["verify_application"].assert_not_called()
            namespace["DARWIN"].renamex_np.assert_not_called()
            namespace["command"].return_value.stdout = b""
            manager.retire_legacy_runtime()
            self.assertTrue(destination.exists())
            self.assertFalse(source.exists() or marker.exists())
            self.assertEqual(namespace["DARWIN"].renamex_np.call_args.args[2], 4)

    def test_retirement_conflicts_and_pending_transactions_preserve_files(self):
        for pending in (True, False):
            with tempfile.TemporaryDirectory() as directory:
                manager, namespace, marker, source, destination = self.retirement_manager(Path(directory), pending=pending)
                if not pending:
                    destination.mkdir()
                with self.assertRaises(RuntimeError):
                    manager.retire_legacy_runtime()
                self.assertTrue(marker.exists() and source.exists())
                namespace["DARWIN"].renamex_np.assert_not_called()

    def test_retirement_recovers_after_successful_rename(self):
        with tempfile.TemporaryDirectory() as directory:
            manager, namespace, marker, source, destination = self.retirement_manager(Path(directory))
            source.rename(destination)
            manager.retire_legacy_runtime()
            self.assertFalse(marker.exists())
            self.assertTrue(destination.exists())
            namespace["verify_application"].assert_called_once_with(destination)

    def test_retirement_ps_and_signature_failure_never_move_files(self):
        for ps_failure in (False, True):
            with tempfile.TemporaryDirectory() as directory:
                manager, namespace, marker, source, destination = self.retirement_manager(Path(directory))
                if ps_failure:
                    namespace["command"].return_value.returncode = 1
                else:
                    namespace["verify_application"].side_effect = RuntimeError("Invalid signature")
                with self.assertRaises(RuntimeError):
                    manager.retire_legacy_runtime()
                self.assertTrue(marker.exists() and source.exists())
                self.assertFalse(destination.exists())
                namespace["DARWIN"].renamex_np.assert_not_called()


if __name__ == "__main__":
    unittest.main()
