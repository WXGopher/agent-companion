#!/usr/bin/env python3
"""Remove Dodex pins and recent-app shortcuts. Read-only by default."""

import argparse
import copy
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
from urllib.parse import unquote, urlsplit
import uuid


DOMAIN = "com.apple.dock"
KEY = "persistent-apps"
APP_ARRAY_KEYS = (KEY, "recent-apps")
APP_PATHS = frozenset({
    "/Applications/Codex B Runtime.app",
    "/Applications/.Dodex/Dodex.app",
    "/Applications/Codex B.app",
    "/Applications/Dodex.app",
})
DEFAULT_BACKUP_DIR = Path.home() / "Library/Application Support/AgentCompanion/DockBackups"


def matching_app_path(value):
    """Return the exact local application path, or None for unfamiliar URLs."""
    if not isinstance(value, str) or not value.startswith("file://"):
        return None
    if any(ord(character) < 32 or ord(character) == 127 for character in value):
        return None
    try:
        url = urlsplit(value)
        if url.scheme != "file" or url.netloc not in ("", "localhost"):
            return None
        if url.query or url.fragment:
            return None
        path = unquote(url.path, encoding="utf-8", errors="strict")
    except (ValueError, UnicodeError):
        return None
    # Dock normally writes a trailing slash. Do not normalize arbitrary paths.
    if path.endswith("/"):
        path = path[:-1]
    return path if path in APP_PATHS else None


def matches_app_url(value):
    """Accept exact, local file URLs; never infer ownership from a label."""
    return matching_app_path(value) is not None


def repaired_tiles(preferences, key=KEY):
    """Remove known Dodex pins, preserving unrelated tiles and their order.

    The launcher and signed runtime have distinct bundle identities. Pinning the
    launcher produces a second icon while the runtime is open; pinning the runtime
    bypasses its isolated environment on the next launch. Leave the running app's
    dynamic Dock icon alone and use Applications/Dodex.app to start it after quit.
    """
    original = preferences.get(key, [])
    if not isinstance(original, list):
        raise ValueError(f"Dock {key} is not an array; no changes made")
    tiles, removed = [], []
    for index, tile in enumerate(original):
        data = tile.get("tile-data") if isinstance(tile, dict) else None
        file_data = data.get("file-data") if isinstance(data, dict) else None
        if isinstance(file_data, dict) and matches_app_url(file_data.get("_CFURLString")):
            removed.append(index)
        else:
            tiles.append(copy.deepcopy(tile))
    return tiles, removed, removed.copy()


def export_preferences():
    result = subprocess.run(
        ["/usr/bin/defaults", "export", DOMAIN, "-"],
        check=True, capture_output=True,
    )
    preferences = plistlib.loads(result.stdout)
    if not isinstance(preferences, dict):
        raise ValueError("Dock preferences are not a dictionary; no changes made")
    return result.stdout, preferences


def serialize_tile(tile):
    # defaults -array accepts each XML property-list value as a separate argument.
    xml = plistlib.dumps(tile, fmt=plistlib.FMT_XML, sort_keys=False).decode("utf-8")
    return xml.split('<plist version="1.0">', 1)[1].rsplit("</plist>", 1)[0].strip()


def save_backup(raw, backup_dir):
    backup_dir = Path(backup_dir).expanduser()
    backup_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    # A pre-existing broad directory must not expose the preference backup.
    backup_dir.chmod(0o700)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    path = backup_dir / f"com.apple.dock-{timestamp}-{uuid.uuid4().hex[:8]}.plist"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as output:
        output.write(raw)
    return path


def repair(*, apply=False, backup_dir=DEFAULT_BACKUP_DIR):
    raw, preferences = export_preferences()
    repairs = {key: repaired_tiles(preferences, key) for key in APP_ARRAY_KEYS}
    changed_arrays = {key: tiles for key, (tiles, _, changed) in repairs.items() if changed}
    removed_by_key = {key: len(changed) for key, (_, _, changed) in repairs.items()}
    report = {
        "mode": "apply" if apply else "check",
        "matched_tiles": sum(len(matched) for _, matched, _ in repairs.values()),
        "changed_tiles": sum(removed_by_key.values()),
        "removed_by_key": removed_by_key,
        "policy": "running-app-only",
        "status": "changes-needed" if changed_arrays else "already-correct",
    }
    if not apply or not changed_arrays:
        return report

    # Re-read before backup and immediately before each array write. No other
    # preference keys are written, including the global recent-apps preference.
    raw, fresh = export_preferences()
    if fresh != preferences:
        raise RuntimeError("Dock preferences changed during inspection; rerun to inspect the new state")
    backup = save_backup(raw, backup_dir)
    expected = copy.deepcopy(preferences)
    written_keys = []
    try:
        for key, tiles in changed_arrays.items():
            _, fresh = export_preferences()
            if fresh != expected:
                raise RuntimeError("Dock preferences changed before writing; stopped without further changes")
            subprocess.run(
                ["/usr/bin/defaults", "write", DOMAIN, key, "-array", *map(serialize_tile, tiles)],
                check=True, capture_output=True,
            )
            written_keys.append(key)
            expected[key] = tiles
            _, actual = export_preferences()
            if any(actual.get(written) != expected[written] for written in written_keys):
                raise RuntimeError("Dock tile readback did not match the requested repair")
    except (subprocess.CalledProcessError, ValueError, RuntimeError, plistlib.InvalidFileException) as error:
        # CalledProcessError includes the command arguments, which here contain
        # unrelated application tiles and their private bookmark data.
        detail = (f"defaults command failed with exit code {error.returncode}"
                  if isinstance(error, subprocess.CalledProcessError) else str(error))
        raise RuntimeError(
            f"Dock repair could not be verified. Backup: {backup}. "
            f"Keys written: {', '.join(written_keys) or 'none'}. {detail}"
        ) from None
    # Dock may be absent, in which case there is nothing to restart.
    restart = subprocess.run(["/usr/bin/killall", "Dock"], capture_output=True)
    report.update(status="repaired", backup=str(backup), dock_restarted=restart.returncode == 0)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="inspect existing tiles without changes (default)")
    mode.add_argument("--apply", action="store_true", help="back up, remove Dodex pins/recent shortcuts, then restart Dock")
    parser.add_argument("--backup-dir", type=Path, default=DEFAULT_BACKUP_DIR)
    args = parser.parse_args(argv)
    if sys.platform != "darwin":
        parser.error("this utility requires macOS")
    try:
        report = repair(apply=args.apply, backup_dir=args.backup_dir)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, plistlib.InvalidFileException) as error:
        print(f"Dodex Dock repair: {error}", file=sys.stderr)
        return 1
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
