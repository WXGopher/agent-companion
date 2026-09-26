#!/usr/bin/env python3
"""Prepare the audited legacy Dodex desktop detection fix without installing it.

With no --output, read and validate the manager and report the hashes only.
An explicit --output creates a new file; existing files are never overwritten.
--consolidate prepares canonical App/TUI paths and guarded legacy retirement.
Without that flag, only the original desktop process detection is repaired.
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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path,
                        default=Path.home() / "Library/Application Support/Codex-B/tools/codex_b_manager.py")
    parser.add_argument("--output", type=Path, help="Create a new staged manager file; never install it")
    parser.add_argument("--consolidate", action="store_true", help="Prepare canonical runtime and launcher paths")
    args = parser.parse_args(argv)
    try:
        source = args.source.read_bytes()
        patched = consolidate(source) if args.consolidate else prepare(source)
        if args.output is not None:
            # Exclusive creation also rejects existing paths and symlinks.
            with args.output.open("xb") as output:
                output.write(patched)
        print(json.dumps({"source_sha256": hashlib.sha256(source).hexdigest(),
                          "patched_sha256": hashlib.sha256(patched).hexdigest(),
                          "output": str(args.output) if args.output else None,
                          "installed": False}, indent=2))
    except (OSError, ValueError, SyntaxError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
