"""Consolidation publication/recovery tests in isolated temporary directories."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "consolidate-dodex.py"
SPEC = importlib.util.spec_from_file_location("consolidate_dodex_under_test", SCRIPT)
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)


class PublicationRecovery(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.apps = self.root / "Applications"
        self.backup = self.apps / ".Dodex/Consolidation Backup"
        self.backup.mkdir(parents=True)
        self.old = self.apps / "Codex B.app"
        (self.old / "Contents/MacOS").mkdir(parents=True)
        (self.old / "Contents/Info.plist").write_bytes(b"original plist")
        (self.old / "Contents/MacOS/Dodex").write_bytes(b"original executable")
        self.public = self.apps / "Dodex.app"
        self.public.symlink_to(self.old)
        self.hashes = INSTALL.launcher_hashes(self.old)
        self.manager = self.root / "manager.py"
        self.original_manager = b"original manager\n"
        self.new_manager = b"new manager\n"
        self.manager.write_bytes(self.original_manager)
        self.manager_sha = INSTALL.digest(self.original_manager)
        self.primary = self.root / "bin/codex"
        self.primary.parent.mkdir()
        self.target = self.root / "vendor/standalone/current/bin/codex"
        self.primary.symlink_to(self.target)
        self.new_primary = b"#!/usr/bin/python3\n# new primary wrapper\n"

    def test_launcher_publication_and_repeat_preserve_original_contents(self):
        INSTALL.publish_launcher(self.apps, self.backup, self.hashes)
        self.assertFalse(self.public.is_symlink())
        self.assertFalse(self.old.exists())
        self.assertEqual(INSTALL.launcher_hashes(self.public), self.hashes)
        self.assertTrue((self.backup / "Dodex.app.alias").is_symlink())
        INSTALL.publish_launcher(self.apps, self.backup, self.hashes)

    def test_resume_after_alias_backup_before_bundle_publication(self):
        self.public.rename(self.backup / "Dodex.app.alias")
        INSTALL.publish_launcher(self.apps, self.backup, self.hashes)
        self.assertEqual(INSTALL.launcher_hashes(self.public), self.hashes)

    def test_launcher_repeat_requires_alias_backup(self):
        INSTALL.publish_launcher(self.apps, self.backup, self.hashes)
        (self.backup / "Dodex.app.alias").unlink()
        with self.assertRaises(ValueError):
            INSTALL.publish_launcher(self.apps, self.backup, self.hashes)

    def test_changed_launcher_does_not_move_alias(self):
        (self.old / "Contents/MacOS/Dodex").write_bytes(b"unrelated change")
        with self.assertRaises(ValueError):
            INSTALL.publish_launcher(self.apps, self.backup, self.hashes)
        self.assertTrue(self.public.is_symlink())
        self.assertFalse((self.backup / "Dodex.app.alias").is_symlink())

    def test_foreign_public_launcher_is_preserved(self):
        self.public.unlink()
        self.public.mkdir()
        foreign = self.public / "foreign"
        foreign.write_bytes(b"unrelated")
        with self.assertRaises(ValueError):
            INSTALL.publish_launcher(self.apps, self.backup, self.hashes)
        self.assertEqual(foreign.read_bytes(), b"unrelated")
        self.assertTrue(self.old.is_dir())

    def test_resume_manager_after_backup_before_replacement(self):
        original_atomic = INSTALL.atomic_write
        def interrupt(path, data, *args, **kwargs):
            if path == self.manager:
                raise OSError("simulated interruption before manager publication")
            return original_atomic(path, data, *args, **kwargs)
        with mock.patch.object(INSTALL, "atomic_write", side_effect=interrupt):
            with self.assertRaises(OSError):
                INSTALL.replace_manager(self.manager, self.backup, self.new_manager, self.manager_sha)
        self.assertEqual(self.manager.read_bytes(), self.original_manager)
        self.assertEqual((self.backup / "codex_b_manager.py").read_bytes(), self.original_manager)
        INSTALL.replace_manager(self.manager, self.backup, self.new_manager, self.manager_sha)
        INSTALL.replace_manager(self.manager, self.backup, self.new_manager, self.manager_sha)
        self.assertEqual(self.manager.read_bytes(), self.new_manager)

    def test_unknown_manager_and_backup_are_preserved(self):
        self.manager.write_bytes(b"unknown manager")
        with self.assertRaises(ValueError):
            INSTALL.replace_manager(self.manager, self.backup, self.new_manager, self.manager_sha)
        self.assertEqual(self.manager.read_bytes(), b"unknown manager")
        self.manager.write_bytes(self.original_manager)
        saved = self.backup / "codex_b_manager.py"
        saved.write_bytes(b"unknown backup")
        with self.assertRaises(ValueError):
            INSTALL.replace_manager(self.manager, self.backup, self.new_manager, self.manager_sha)
        self.assertEqual(saved.read_bytes(), b"unknown backup")
        self.assertEqual(self.manager.read_bytes(), self.original_manager)

    def test_resume_primary_after_alias_backup_before_wrapper_publication(self):
        self.primary.rename(self.backup / "codex.original-link")
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.read_bytes(), self.new_primary)
        self.assertTrue((self.backup / "codex.original-link").is_symlink())

    def test_primary_publication_and_repeat_keep_vendor_target(self):
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.read_bytes(), self.new_primary)
        self.assertEqual((self.backup / "codex.original-link").readlink(), self.target)

    def test_vendor_restored_same_target_can_be_repaired_repeatedly(self):
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        saved = self.backup / "codex.original-link"
        original_inode = saved.lstat().st_ino
        for _ in range(2):
            self.primary.unlink()
            self.primary.symlink_to(self.target)
            INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
            self.assertEqual(self.primary.read_bytes(), self.new_primary)
            self.assertEqual(saved.readlink(), self.target)
            self.assertEqual(saved.lstat().st_ino, original_inode)
        restored = list(self.backup.glob("codex.vendor-restored-*.link"))
        self.assertEqual(len(restored), 2)
        self.assertTrue(all(path.is_symlink() and path.readlink() == self.target for path in restored))

    def test_vendor_restored_unknown_target_or_changed_backup_is_preserved(self):
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.primary.unlink()
        foreign = self.root / "foreign-codex"
        self.primary.symlink_to(foreign)
        with self.assertRaises(ValueError):
            INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.readlink(), foreign)
        self.primary.unlink()
        self.primary.symlink_to(self.target)
        saved = self.backup / "codex.original-link"
        saved.unlink()
        saved.symlink_to(foreign)
        with self.assertRaises(ValueError):
            INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.readlink(), self.target)
        self.assertEqual(saved.readlink(), foreign)

    def test_resume_after_vendor_restored_link_was_archived(self):
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.primary.unlink()
        self.primary.symlink_to(self.target)
        with mock.patch.object(INSTALL, "atomic_write", side_effect=OSError("interrupted wrapper publication")):
            with self.assertRaises(OSError):
                INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertFalse(INSTALL.exists(self.primary))
        self.assertTrue((self.backup / "codex.original-link").is_symlink())
        INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.read_bytes(), self.new_primary)

    def test_unknown_primary_entry_is_preserved(self):
        self.primary.unlink()
        self.primary.write_bytes(b"foreign primary")
        with self.assertRaises(ValueError):
            INSTALL.publish_primary(self.primary, self.backup, self.target, self.new_primary)
        self.assertEqual(self.primary.read_bytes(), b"foreign primary")

    def test_symlinked_backup_parent_is_rejected(self):
        redirected = self.root / "redirected"
        redirected.symlink_to(self.backup)
        with self.assertRaises(ValueError):
            INSTALL.publish_launcher(self.apps, redirected, self.hashes)
        self.assertTrue(self.public.is_symlink())


class FullRunRecovery(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.home, self.apps = self.base / "user", self.base / "Applications"
        self.root = self.apps / ".Dodex"
        self.root.mkdir(parents=True)
        self.backup = self.root / "Consolidation Backup"
        self.journal = self.root / "consolidation.json"
        self.public, self.old = self.apps / "Dodex.app", self.apps / "Codex B.app"
        (self.old / "Contents/MacOS").mkdir(parents=True)
        (self.old / "Contents/Info.plist").write_bytes(b"original plist")
        (self.old / "Contents/MacOS/Dodex").write_bytes(b"original launcher")
        self.public.symlink_to(self.old)
        self.manager = self.home / "Library/Application Support/Codex-B/tools/codex_b_manager.py"
        self.manager.parent.mkdir(parents=True)
        self.manager.write_bytes(b"original manager")
        (self.home / "Library/Application Support/Codex-B-Backups").mkdir()
        (self.home / "Library/Application Support/AgentCompanion").mkdir()
        self.primary = self.home / ".local/bin/codex"
        self.primary.parent.mkdir(parents=True)
        self.target = self.home / ".codex/packages/standalone/current/bin/codex"
        self.target.parent.mkdir(parents=True)
        self.target.write_bytes(b"untouched vendor executable")
        self.target.chmod(0o755)
        self.primary.symlink_to(self.target)
        self.profile = self.home / ".codex-second"
        self.profile.mkdir()
        (self.profile / "sentinel").write_bytes(b"untouched profile")
        (self.root / "Dodex.app").mkdir()
        self.old_runtime = self.apps / "Codex B Runtime.app"
        self.old_runtime.mkdir()
        self.prep = SimpleNamespace(consolidate=lambda source: b"new manager"
                                    if source in (b"original manager", b"new manager")
                                    else (_ for _ in ()).throw(ValueError("unknown source")))
        self.primary_prep = SimpleNamespace(render_wrapper=lambda home, target: b"primary wrapper")
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(INSTALL, "module", side_effect=lambda name:
                          self.prep if name == "prepare-dodex-manager" else self.primary_prep).start()
        self.commands = mock.patch.object(INSTALL.subprocess, "run").start()
        self.processes = mock.patch.object(INSTALL, "process_paths", return_value=[
            (1234, str(self.old_runtime / "Contents/Resources/codex"))]).start()
        self.flags = mock.patch.object(INSTALL.os, "chflags", create=True).start()

    def run_installer(self, apply=True):
        return INSTALL.run(apply, home=self.home, applications=self.apps)

    def assert_complete(self):
        self.assertEqual(json.loads(self.journal.read_bytes())["phase"], "complete")
        self.assertFalse(self.public.is_symlink())
        self.assertEqual(self.manager.read_bytes(), b"new manager")
        self.assertEqual(self.primary.read_bytes(), b"primary wrapper")
        self.assertEqual((self.backup / "codex_b_manager.py").read_bytes(), b"original manager")
        self.assertEqual((self.profile / "sentinel").read_bytes(), b"untouched profile")
        self.assertEqual(self.target.read_bytes(), b"untouched vendor executable")
        self.assertTrue(self.old_runtime.is_dir())

    def test_read_only_reports_actual_primary_and_makes_no_publication(self):
        result = self.run_installer(False)
        self.assertFalse(result["primary_cli_isolated"])
        self.assertFalse(self.journal.exists())
        self.assertFalse(self.backup.exists())
        self.assertTrue(self.public.is_symlink())
        self.assertTrue(self.primary.is_symlink())
        self.flags.assert_not_called()

    def test_apply_and_repeat_keep_profile_vendor_and_existing_cli_untouched(self):
        self.assertTrue(self.run_installer()["primary_cli_isolated"])
        self.assert_complete()
        self.assertTrue(self.run_installer()["primary_cli_isolated"])
        self.assert_complete()

    def test_completed_installer_repairs_vendor_restored_primary_link(self):
        self.run_installer()
        self.primary.unlink()
        self.primary.symlink_to(self.target)
        self.assertFalse(self.run_installer(False)["primary_cli_isolated"])
        self.assertTrue(self.run_installer()["primary_cli_isolated"])
        self.assert_complete()

    def test_missing_primary_interpreter_rejects_apply_before_mutation(self):
        real_commands = self.commands

        def fail_interpreter(arguments, **kwargs):
            if arguments == ["/usr/bin/python3", "--version"]:
                self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
                self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
                self.assertEqual(kwargs["timeout"], 10)
                raise subprocess.CalledProcessError(1, arguments, stderr=b"hidden interpreter diagnostic")
            return SimpleNamespace(returncode=0)

        real_commands.side_effect = fail_interpreter
        with self.assertRaisesRegex(ValueError, "working /usr/bin/python3") as error:
            self.run_installer()
        self.assertNotIn("hidden interpreter diagnostic", str(error.exception))
        self.assertFalse(self.journal.exists())
        self.assertFalse(self.backup.exists())
        self.assertTrue(self.primary.is_symlink())
        self.assertTrue(self.public.is_symlink())
        self.assertEqual(self.manager.read_bytes(), b"original manager")
        self.flags.assert_not_called()

    def test_interpreter_timeout_and_missing_executable_fail_clearly(self):
        for failure in (subprocess.TimeoutExpired(["/usr/bin/python3", "--version"], 10),
                        FileNotFoundError("missing interpreter")):
            with mock.patch.object(INSTALL.subprocess, "run", side_effect=failure):
                with self.assertRaisesRegex(ValueError, "working /usr/bin/python3"):
                    INSTALL.check_primary_interpreter()

    def test_resume_after_journal_before_backup_directory(self):
        real = INSTALL.atomic_write
        def interrupt(path, data, *args, **kwargs):
            result = real(path, data, *args, **kwargs)
            if path == self.journal and json.loads(data)["phase"] == "prepared":
                raise OSError("interruption after durable journal")
            return result
        with mock.patch.object(INSTALL, "atomic_write", side_effect=interrupt):
            with self.assertRaises(OSError):
                self.run_installer()
        self.assertTrue(self.journal.is_file())
        self.assertFalse(self.backup.exists())
        self.run_installer()
        self.assert_complete()

    def test_resume_after_launcher_alias_was_backed_up(self):
        real = INSTALL.rename_exclusive
        def interrupt(source, destination):
            result = real(source, destination)
            if source == self.public:
                raise OSError("interruption after alias backup")
            return result
        with mock.patch.object(INSTALL, "rename_exclusive", side_effect=interrupt):
            with self.assertRaises(OSError):
                self.run_installer()
        self.assertFalse(INSTALL.exists(self.public))
        self.run_installer()
        self.assert_complete()

    def test_resume_after_manager_replacement(self):
        real = INSTALL.atomic_write
        def interrupt(path, data, *args, **kwargs):
            result = real(path, data, *args, **kwargs)
            if path == self.manager:
                raise OSError("interruption after manager replacement")
            return result
        with mock.patch.object(INSTALL, "atomic_write", side_effect=interrupt):
            with self.assertRaises(OSError):
                self.run_installer()
        self.assertEqual(self.manager.read_bytes(), b"new manager")
        self.run_installer()
        self.assert_complete()

    def test_empty_or_unknown_journal_is_rejected_without_mutation(self):
        for value in ({}, [], {"schema": 1, "phase": "unknown"}):
            self.journal.write_text(json.dumps(value))
            with self.assertRaises(ValueError):
                self.run_installer()
            self.assertTrue(self.public.is_symlink())
            self.assertTrue(self.primary.is_symlink())
            self.assertEqual(self.manager.read_bytes(), b"original manager")
            self.assertFalse(self.backup.exists())


if __name__ == "__main__":
    unittest.main()
