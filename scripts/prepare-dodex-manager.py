#!/usr/bin/env python3
"""Prepare the audited legacy Dodex desktop detection fix without installing it.

With no --output, read and validate the manager and report the hashes only.
An explicit --output creates a new file; existing files are never overwritten.
--consolidate prepares canonical App/TUI paths and guarded legacy retirement.
Daily update calls always enter Companion paired maintenance. Historical bytes
are exported only with --recovery-material-only for explicit rollback material.
"""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import sys


SOURCE_SHA256 = "777d6146f9f101c14e9a96afebff824767c5d820c6e3f05a91af23bc792565ae"
PATCHED_SHA256 = "cc38a979239ed95058743c089d97c3c0c89cb73ee8dd2c27a9c8a82ebaf452ee"
CONSOLIDATED_SHA256 = "2c56ce079beab872f01a306b5e7e3b1a0883dfdb86d08dc2d021d54a68cd19df"
RESOURCES = (Path(__file__).resolve().parent.parent / "crates/agent-companion/src/macos_deployment")
PATCHES = ("legacy-desktop-method", "legacy-desktop-launch")
CONSOLIDATION_PATCHES = (
    "consolidated-paths", "consolidated-maintenance", "consolidated-schema",
    "consolidated-snapshot-error", "consolidated-import", "consolidated-environment",
    "consolidated-signature", "consolidated-signature-helper",
    "consolidated-retirement-call", "consolidated-retirement",
    "consolidated-desktop-target", "consolidated-desktop-match", "consolidated-auth",
    "consolidated-report", "consolidated-no-snapshot",
)


def replace_fragments(source, patches=PATCHES):
    """Apply shared literal replacements exactly once each; do not execute code."""
    for name in patches:
        before = (RESOURCES / (name + ".old.txt")).read_bytes()
        after = (RESOURCES / (name + ".new.txt")).read_bytes()
        if source.count(before) != 1:
            raise ValueError("Expected exactly one patch location: " + name)
        source = source.replace(before, after, 1)
    return source


def prepare(source):
    if hashlib.sha256(source).hexdigest() != SOURCE_SHA256:
        raise ValueError("Unrecognized manager SHA-256; refusing to patch.")
    patched = replace_fragments(source)
    if hashlib.sha256(patched).hexdigest() != PATCHED_SHA256:
        raise ValueError("Patched manager SHA-256 differs from the audited result.")
    ast.parse(patched)
    return patched


def consolidate(source):
    """Return the one audited canonical manager, accepting both legacy revisions.

    Installers must reject prepared/committed transaction journals located at
    ~/Library/Application Support/Codex-B-Backups/.transaction-*.json before
    publishing this code. Old snapshots are preserved under their old schema.
    This pure transformation never imports a manager or reads profile data.
    """
    digest = hashlib.sha256(source).hexdigest()
    if digest == CONSOLIDATED_SHA256:
        return source
    if digest == SOURCE_SHA256:
        source = prepare(source)
    elif digest != PATCHED_SHA256:
        raise ValueError("Unrecognized manager SHA-256; refusing consolidation.")
    result = replace_fragments(source, CONSOLIDATION_PATCHES)
    if hashlib.sha256(result).hexdigest() != CONSOLIDATED_SHA256:
        raise ValueError("Consolidated manager SHA-256 differs from the audited result.")
    ast.parse(result)
    return result


def route_maintenance(source, companion):
    """Replace only the known update method; preserve rollback code as material.

    Call after prepare/consolidate has authenticated the complete source bytes.
    Never import the manager: even constructing it can inspect personal state.
    """
    companion = Path(companion)
    if not companion.is_absolute() or ".." in companion.parts:
        raise ValueError("The maintenance executable must be an absolute Companion path.")
    tree = ast.parse(source)
    methods = [method for node in tree.body if isinstance(node, ast.ClassDef) and node.name == "Manager"
               for method in node.body if isinstance(method, ast.FunctionDef) and method.name == "update"]
    if len(methods) != 1 or [argument.arg for argument in methods[0].args.args] != ["self", "app", "dmg"]:
        raise ValueError("Expected exactly one audited Manager.update method.")
    replacement = ("    def update(self, app=None, dmg=None):\n"
        "        if app is not None or dmg is not None:\n"
        "            raise ManagerError('Use Companion paired maintenance for official stable updates; local package overrides are disabled.')\n"
        "        companion = " + repr(str(companion)) + "\n"
        "        def keep(name):\n"
        "            name = name.upper()\n"
        "            if name.startswith('CODEX_'):\n"
        "                return name in {'CODEX_SANDBOX', 'CODEX_SANDBOX_NETWORK_DISABLED', 'CODEX_CA_CERTIFICATE', 'CODEX_PROXY_CERT'} or name.startswith('CODEX_NETWORK_')\n"
        "            return not name.startswith(('OPENAI_', 'CHATGPT_', 'ELECTRON_', 'DYLD_', 'LD_')) and name not in {'NODE_OPTIONS', 'NODE_PATH'}\n"
        "        environment = {name: value for name, value in os.environ.items() if keep(name)}\n"
        "        os.execve(companion, [companion, 'software-maintenance', 'update-all'], environment)\n")
    lines = source.decode("utf-8").splitlines(keepends=True)
    method = methods[0]
    result = ("".join(lines[:method.lineno - 1]) + replacement + "".join(lines[method.end_lineno:])).encode("utf-8")
    ast.parse(result)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path,
                        default=Path.home() / "Library/Application Support/Codex-B/tools/codex_b_manager.py")
    parser.add_argument("--output", type=Path, help="Create a new staged manager file; never install it")
    parser.add_argument("--consolidate", action="store_true", help="Prepare canonical runtime and launcher paths")
    parser.add_argument("--companion", type=Path, default=Path("/Applications/Agent Companion.app/Contents/MacOS/agent-companion"),
                        help="Absolute current Companion executable for paired maintenance")
    parser.add_argument("--recovery-material-only", action="store_true", help="Export historical audited bytes only, for explicit rollback material")
    args = parser.parse_args(argv)
    try:
        source = args.source.read_bytes()
        patched = consolidate(source) if args.consolidate else prepare(source)
        if not args.recovery_material_only:
            patched = route_maintenance(patched, args.companion)
        if args.output is not None:
            # Exclusive creation also rejects existing paths and symlinks.
            with args.output.open("xb") as output:
                output.write(patched)
        print(json.dumps({"source_sha256": hashlib.sha256(source).hexdigest(),
                          "patched_sha256": hashlib.sha256(patched).hexdigest(),
                          "output": str(args.output) if args.output else None,
                          "installed": False, "maintenance_route": "recovery-material" if args.recovery_material_only else "companion"}, indent=2))
    except (OSError, ValueError, SyntaxError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
