#!/usr/bin/env python3
"""Consolidate the audited repaired Dodex installation; default is read-only.

Keeps a live legacy CLI in place. A durable journal and exact code fingerprints
allow --apply to finish interrupted publication without overwriting other work.
No credentials, profiles, conversations, signed app resources or running
processes are changed. The manager retires the old runtime after its last exit.
"""
import argparse
import contextlib
import ctypes
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import uuid


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def digest(data):
    return hashlib.sha256(data).hexdigest()


def exists(path):
    return os.path.lexists(path)


def no_links(path):
    for part in (path, *path.parents):
        if part.is_symlink():
            raise ValueError("Unexpected symbolic link: " + str(part))


def read_regular(path):
    no_links(path)
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError("Expected a private regular file: " + str(path))
    return path.read_bytes()


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def rename_exclusive(source, destination):
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin":
        rename = library.renamex_np
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        result = rename(os.fsencode(source), os.fsencode(destination), 4)
    elif sys.platform.startswith("linux"):
        rename = library.renameat2
        rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int,
                           ctypes.c_char_p, ctypes.c_uint]
        result = rename(-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    else:
        raise OSError("Exclusive publication requires macOS or Linux")
    if result != 0:
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number), str(destination))
    sync_directory(destination.parent)
    if source.parent != destination.parent:
        sync_directory(source.parent)


def atomic_write(path, data, mode=0o600, replace=False):
    no_links(path.parent)
    temporary = path.with_name("." + path.name + "." + uuid.uuid4().hex)
    try:
        fd = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, mode)
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        if replace:
            os.replace(temporary, path)
            sync_directory(path.parent)
        else:
            rename_exclusive(temporary, path)
    finally:
        if exists(temporary):
            temporary.unlink()


@contextlib.contextmanager
def locked(path):
    no_links(path)
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)


def launcher_hashes(bundle):
    return {name: digest(read_regular(bundle / name)) for name in
            ("Contents/Info.plist", "Contents/MacOS/Dodex")}


def publish_launcher(applications, backup, expected):
    """Recover either side of alias-backup / real-bundle publication."""
    public = applications / "Dodex.app"
    old = applications / "Codex B.app"
    saved_alias = backup / "Dodex.app.alias"
    for path in (applications, backup):
        no_links(path)
    if public.is_symlink():
        target = Path(os.readlink(public))
        if not target.is_absolute():
            target = applications / target
        if target != old or exists(saved_alias) or launcher_hashes(old) != expected:
            raise ValueError("Dodex launcher alias or its original bundle changed")
        rename_exclusive(public, saved_alias)
    if not saved_alias.is_symlink():
        raise ValueError("Missing original Dodex alias backup")
    original_target = Path(os.readlink(saved_alias))
    if not original_target.is_absolute():
        original_target = applications / original_target
    if original_target != old:
        raise ValueError("Original Dodex alias backup changed")
    if exists(public):
        if exists(old) or launcher_hashes(public) != expected:
            raise ValueError("Conflicting Dodex launcher; existing files preserved")
        return
    if not saved_alias.is_symlink() or launcher_hashes(old) != expected:
        raise ValueError("Cannot resume Dodex launcher publication")
    # Both names are inside the checked application directory; no replacement.
    rename_exclusive(old, public)


def replace_manager(path, backup, new, original_sha):
    current = read_regular(path)
    saved = backup / "codex_b_manager.py"
    if digest(current) == digest(new):
        if not exists(saved) or digest(read_regular(saved)) != original_sha:
            raise ValueError("Missing original manager backup")
        return
    if digest(current) != original_sha:
        raise ValueError("Manager changed concurrently; no overwrite")
    if exists(saved):
        if read_regular(saved) != current:
            raise ValueError("Conflicting manager backup")
    else:
        atomic_write(saved, current)
    if read_regular(path) != current:
        raise ValueError("Manager changed during backup; no overwrite")
    atomic_write(path, new, 0o755, replace=True)


def publish_primary(path, backup, target, new):
    saved = backup / "codex.original-link"
    no_links(path.parent)
    if path.is_symlink():
        if Path(os.readlink(path)) != target:
            raise ValueError("Primary Codex entry or backup changed")
        if exists(saved):
            if not saved.is_symlink() or Path(os.readlink(saved)) != target:
                raise ValueError("Original primary Codex entry backup changed")
            # The vendor updater can restore the same original symlink. Retain
            # its entry separately and keep the first backup immutable. Exclusive
            # publication below also preserves a new entry appearing meanwhile.
            restored = backup / ("codex.vendor-restored-" + uuid.uuid4().hex + ".link")
            rename_exclusive(path, restored)
            if not restored.is_symlink() or Path(os.readlink(restored)) != target:
                rename_exclusive(restored, path)
                raise ValueError("Primary Codex entry changed during restoration")
        else:
            rename_exclusive(path, saved)
    if exists(path):
        if read_regular(path) != new:
            raise ValueError("Unrecognized primary Codex entry; no overwrite")
        if not saved.is_symlink() or Path(os.readlink(saved)) != target:
            raise ValueError("Missing original Codex entry backup")
        return
    if not saved.is_symlink() or Path(os.readlink(saved)) != target:
        raise ValueError("Cannot resume primary Codex entry publication")
    atomic_write(path, new, 0o755)


def process_paths():
    completed = subprocess.run(["/bin/ps", "-axo", "pid=,comm="], check=True,
                               stdout=subprocess.PIPE, text=True)
    rows = []
    for line in completed.stdout.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) != 2 or not fields[0].isdigit():
            raise ValueError("Cannot establish current process paths")
        rows.append((int(fields[0]), fields[1]))
    return rows


def pending_transactions(backups):
    for path in backups.glob(".transaction-*.json"):
        record = json.loads(read_regular(path))
        if record.get("phase") in {"prepared", "committed"}:
            raise ValueError("Finish the pending legacy transaction before consolidation: " + path.name)


def check_primary_interpreter():
    # The generated wrapper uses this absolute interpreter, even when the
    # installer itself was launched with Homebrew or another Python installation.
    try:
        subprocess.run(["/usr/bin/python3", "--version"], check=True, timeout=10,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError("A working /usr/bin/python3 is required for the primary CLI wrapper; no entries were changed") from error


def run(apply=False, *, home=None, applications=None):
    home = Path.home() if home is None else Path(home)
    apps = Path("/Applications") if applications is None else Path(applications)
    root = apps / ".Dodex"
    backup = root / "Consolidation Backup"
    journal = root / "consolidation.json"
    manager = home / "Library/Application Support/Codex-B/tools/codex_b_manager.py"
    legacy_backups = home / "Library/Application Support/Codex-B-Backups"
    support = home / "Library/Application Support/AgentCompanion"
    primary = home / ".local/bin/codex"
    target = home / ".codex/packages/standalone/current/bin/codex"
    runtime = root / "Dodex.app"
    old_runtime = apps / "Codex B Runtime.app"
    retired = root / "Retired Runtime.app"
    prep = module("prepare-dodex-manager")
    primary_prep = module("prepare-codex-cli")
    for path in (root, manager, legacy_backups, support, primary.parent):
        no_links(path)
    pending_transactions(legacy_backups)
    existing = json.loads(read_regular(journal)) if exists(journal) else None
    if existing is not None and (not isinstance(existing, dict)
            or existing.get("schema") != 1
            or existing.get("phase") not in {"prepared", "complete"}):
        raise ValueError("Unknown consolidation journal phase")
    source = read_regular(backup / "codex_b_manager.py") if existing is not None and exists(backup / "codex_b_manager.py") else read_regular(manager)
    new_manager = prep.consolidate(source)
    new_primary = primary_prep.render_wrapper(home, target)
    if not target.is_file() or not os.access(target, os.X_OK):
        raise ValueError("Original standalone Codex executable is unavailable")
    public = apps / "Dodex.app"
    bundle = apps / "Codex B.app" if public.is_symlink() else public
    if not exists(bundle) and existing is not None:
        bundle = apps / "Codex B.app"
    expected = {"schema": 1, "manager_before": digest(source),
                "manager_after": digest(new_manager), "primary_target": str(target),
                "primary_after": digest(new_primary), "launcher": launcher_hashes(bundle)}
    if existing is not None:
        if {k: existing.get(k) for k in expected} != expected:
            raise ValueError("Consolidation journal or prepared code changed; preserve backups")
    elif not public.is_symlink() or not primary.is_symlink():
        raise ValueError("Expected the audited legacy aliases before first consolidation")
    if existing is None:
        alias = Path(os.readlink(public))
        if not alias.is_absolute():
            alias = apps / alias
        if alias != apps / "Codex B.app" or Path(os.readlink(primary)) != target:
            raise ValueError("An original entry points to an unrecognized target")
    rows = process_paths()
    desktop_paths = {str(p / "Contents/MacOS/ChatGPT") for p in (runtime, old_runtime)}
    if any(command in desktop_paths for _, command in rows):
        raise ValueError("Quit Dodex desktop first; current CLI may remain running")
    legacy_pids = [pid for pid, command in rows if command.startswith(str(old_runtime) + "/")]
    # A known interrupted publication can temporarily lack the public entry.
    # Before its first mutation, the existing installed Companion authenticates
    # the repaired launcher/runtime/profile isolation without opening an app.
    if existing is None or existing.get("phase") == "complete":
        subprocess.run([str(apps / "Agent Companion.app/Contents/MacOS/agent-companion"),
                        "dodex-app"], check=True, stdout=subprocess.DEVNULL)
    report = {"mode": "apply" if apply else "check", "entry": str(public),
              "runtime": str(runtime), "legacy_runtime_pids": legacy_pids,
              "primary_cli_isolated": not primary.is_symlink() and exists(primary)
              and read_regular(primary) == new_primary, "backup": str(backup)}
    if not apply:
        return report
    check_primary_interpreter()
    with locked(support / "deployment.lock"), locked(legacy_backups / ".manager.lock"):
        pending_transactions(legacy_backups)
        if existing is None:
            if exists(backup):
                raise ValueError("Unrecognized consolidation backup directory")
            expected["phase"] = "prepared"
            atomic_write(journal, json.dumps(expected, indent=2).encode())
        if not exists(backup):
            backup.mkdir(mode=0o700)
            sync_directory(backup.parent)
        no_links(backup)
        rows = process_paths()
        if any(command in desktop_paths for _, command in rows):
            raise ValueError("Dodex desktop started during validation; retry after quitting it")
        publish_launcher(apps, backup, expected["launcher"])
        replace_manager(manager, backup, new_manager, expected["manager_before"])
        publish_primary(primary, backup, target, new_primary)
        marker = root / "legacy-runtime-retirement.json"
        marker_data = {"schema": 1, "source": str(old_runtime), "destination": str(retired)}
        if exists(marker) and json.loads(read_regular(marker)) != marker_data:
            raise ValueError("Conflicting legacy retirement record")
        if exists(old_runtime):
            no_links(old_runtime)
            if exists(retired):
                raise ValueError("Both old and archived runtimes exist; no overwrite")
            # BSD hidden flag does not add FinderInfo or change signed contents.
            os.chflags(old_runtime, old_runtime.stat().st_flags | stat.UF_HIDDEN)
            if not exists(marker):
                atomic_write(marker, json.dumps(marker_data).encode())
        expected["phase"] = "complete"
        atomic_write(journal, json.dumps(expected, indent=2).encode(), replace=True)
    subprocess.run([str(apps / "Agent Companion.app/Contents/MacOS/agent-companion"),
                    "dodex-app"], check=True, stdout=subprocess.DEVNULL)
    report["primary_cli_isolated"] = read_regular(primary) == new_primary
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    try:
        print(json.dumps(run(args.apply), indent=2))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print("Dodex consolidation: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
