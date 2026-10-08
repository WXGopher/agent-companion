#!/usr/bin/env python3
"""Retire the recognized macOS Dodex desktop after a successful TUI migration.

The bundle goes to Trash; original paths and Dock preferences are journaled.
Account, database, history, logs and live terminal processes are untouched.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
from urllib.parse import unquote, urlparse


def plain(path):
    if not path.is_absolute() or path.resolve(strict=False) != path:
        raise RuntimeError("Recovery path contains a redirect")


def save(path, value):
    plain(path.parent)
    temporary = path.with_name(path.name + ".pending")
    plain(temporary)
    with temporary.open("wb") as file:
        os.chmod(temporary, 0o600)
        file.write(json.dumps(value, ensure_ascii=False, indent=2).encode())
        file.flush()
        os.fsync(file.fileno())
    temporary.replace(path)


def filtered_dock(preferences, app):
    result = dict(preferences)
    result["persistent-apps"] = [tile for tile in preferences.get("persistent-apps", [])
        if Path(unquote(urlparse(tile.get("tile-data", {}).get("file-data", {})
            .get("_CFURLString", "")).path)).resolve(strict=False) != app]
    return result


def validate(home):
    plain(home)
    support = home / "Library/Application Support/AgentCompanion"
    plain(support)
    record = json.loads((support / "tui-instances.json").read_text())
    instance = record["dodex"]
    profile = Path(instance["codex_home"])
    plain(profile)
    if (record["schema"] != 2 or instance["id"] != "dodex"
            or instance["channel"] != "standalone" or not profile.is_relative_to(support)
            or profile == support):
        raise RuntimeError("A published, independent Dodex TUI record is required")
    native = Path(instance["cli_path"]).resolve(strict=True)
    if not native.is_relative_to(profile / "packages/standalone/releases"):
        raise RuntimeError("Dodex must already use its own standalone package")
    manifest = native.parent.parent / "codex-package.json"
    if json.loads(manifest.read_text())["entrypoint"] != "bin/codex":
        raise RuntimeError("The native TUI package is incomplete")
    entry = Path(instance["command_path"])
    plain(entry)
    if entry != home / ".local/bin/dodex" or not os.access(entry, os.X_OK):
        raise RuntimeError("The new terminal entry is unavailable")
    version = subprocess.run([str(entry), "--version"], capture_output=True, text=True, timeout=20)
    if version.returncode or not version.stdout.startswith("codex-cli "):
        raise RuntimeError("The migrated Dodex terminal entry failed verification")
    backup = support / "TuiMigration/dual-instance.json.before-tui"
    legacy = json.loads(backup.read_text())
    app = home / "Applications/Dodex.app"
    if (Path(legacy["instance"]["runtime_app"]) != app
            or Path(legacy["instance"]["codex_home"]) != profile):
        raise RuntimeError("The old desktop and migrated profile are not the same managed instance")
    return support, app


def retire(home):
    support, app = validate(home)
    journal = support / "TuiMigration/desktop-retirement.json"
    previous = json.loads(journal.read_text()) if journal.exists() else None
    if previous and previous["phase"] == "complete" and not app.exists():
        print("Dodex desktop is already in Trash; recovery information is preserved.")
        return
    if app.exists():
        plain(app)
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        if (info.get("CFBundleIdentifier") != "local.agent-companion.dodex"
                or info.get("CFBundleExecutable") != "DodexLauncher"):
            raise RuntimeError("The desktop bundle is not the managed Dodex launcher")
        processes = subprocess.check_output(["/bin/ps", "-axo", "comm="], text=True)
        if any(str(app / "Contents/MacOS") in line for line in processes.splitlines()):
            raise RuntimeError("Close the Dodex desktop before retirement; no process was terminated")
        trash = home / ".Trash"
        plain(trash)
        trash.mkdir(mode=0o700, exist_ok=True)
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        destination = trash / ("Dodex-" + stamp + ".app")
        if destination.exists():
            raise RuntimeError("The recovery destination already exists")
        recovery = {"schema": 1, "phase": "pending", "source": str(app),
                    "trash": str(destination), "retired_at": stamp,
                    "profile_preserved": json.loads((support / "tui-instances.json").read_text())["dodex"]["codex_home"]}
        save(journal, recovery)
        registration = Path("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        subprocess.run([str(registration), "-u", str(app)], check=True, capture_output=True)
        app.rename(destination)
    else:
        if not previous or not Path(previous["trash"]).is_dir():
            raise RuntimeError("No recognized desktop bundle or recovery journal exists")
        recovery = previous
    dock = subprocess.check_output(["/usr/bin/defaults", "export", "com.apple.dock", "-"])
    original = plistlib.loads(dock)
    updated = filtered_dock(original, app)
    if updated != original:
        backup = support / "TuiMigration/Dock.before-tui.plist"
        if not backup.exists():
            backup.write_bytes(dock)
            os.chmod(backup, 0o600)
        filtered = support / "TuiMigration/Dock.without-dodex.plist"
        filtered.write_bytes(plistlib.dumps(updated))
        os.chmod(filtered, 0o600)
        recovery["dock_backup"] = str(backup)
        save(journal, recovery)
        subprocess.run(["/usr/bin/defaults", "import", "com.apple.dock", str(filtered)], check=True, capture_output=True)
        subprocess.run(["/usr/bin/killall", "Dock"], check=False, capture_output=True)
    recovery["phase"] = "complete"
    save(journal, recovery)
    print("Dodex desktop moved to Trash; terminal profiles and recovery records are preserved.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, default=Path.home())
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Desktop retirement is macOS-only")
    retire(args.home)
