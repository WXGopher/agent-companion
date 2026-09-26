#!/usr/bin/env python3
"""Stage a primary Codex CLI wrapper without changing the vendor installation.

The standalone updater may replace the installed entry symlink/wrapper. Re-run
the installation repair after updating if the primary entry is a symlink again.
This generator never edits package internals or installs its output.
"""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import sys


def render_wrapper(user_home: Path, target: Path) -> bytes:
    """Render a deterministic wrapper for the original vendor-managed target."""
    user_home, target = Path(user_home), Path(target)
    expected = user_home / ".codex/packages/standalone/current/bin/codex"
    if (not user_home.is_absolute() or ".." in user_home.parts
            or target != expected
            or any(ord(char) < 32 for char in str(user_home))):
        raise ValueError("The primary CLI target must be the user's original standalone/current/bin/codex.")
    source = '''#!/usr/bin/python3 -B
"""Primary Codex entry: isolate account/state, preserve inherited containment."""
import os
import sys

PRIMARY_HOME = __PRIMARY_HOME__
TARGET = __TARGET__
AUTH_ENV = {
    "OPENAI_API_KEY", "OPENAI_ACCESS_TOKEN", "OPENAI_ORG_ID",
    "OPENAI_ORGANIZATION", "OPENAI_PROJECT_ID", "CHATGPT_ACCESS_TOKEN",
    "OPENAI_BASE_URL", "OPENAI_API_BASE",
    "OPENAI_IDENTITY_TOKEN_FILE", "OPENAI_WORKLOAD_IDENTITY_CONTEXT",
    "OPENAI_FEDERATION_RULE_ID",
}
SAFETY_ENV = {"CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED"}


def primary_environment(inherited):
    # Network-policy/proxy metadata and sandbox markers are containment context,
    # not profile identity. Keep them while dropping the previous Codex session.
    env = {key: value for key, value in inherited.items()
           if (not key.startswith("CODEX_") or key in SAFETY_ENV
               or key.startswith("CODEX_NETWORK_"))
           and key not in AUTH_ENV
           and not key.startswith(("ELECTRON_", "DYLD_"))
           and key not in {"NODE_OPTIONS", "NODE_PATH"}}
    env["CODEX_HOME"] = PRIMARY_HOME
    return env


if __name__ == "__main__":
    try:
        # Preserve the vendor update target, cwd, arguments, exit code and
        # process/signal behavior; do not fork a CLI supervisor or alter PATH.
        os.execve(TARGET, [TARGET] + sys.argv[1:], primary_environment(os.environ))
    except OSError as error:
        print("Codex: the original standalone CLI could not be started: " + str(error), file=sys.stderr)
        sys.exit(127)
'''.replace("__PRIMARY_HOME__", repr(str(user_home / ".codex"))).replace("__TARGET__", repr(str(target)))
    ast.parse(source)
    return source.encode("utf-8")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--user-home", type=Path, default=Path.home())
    parser.add_argument("--target", type=Path)
    parser.add_argument("--output", type=Path, help="Create a new staged file; never install it")
    args = parser.parse_args(argv)
    try:
        target = args.target or args.user_home / ".codex/packages/standalone/current/bin/codex"
        rendered = render_wrapper(args.user_home, target)
        if args.output is not None:
            with args.output.open("xb") as handle:
                handle.write(rendered)
            args.output.chmod(0o755)
        print(json.dumps({"target": str(target), "sha256": hashlib.sha256(rendered).hexdigest(),
                          "output": str(args.output) if args.output else None, "installed": False}, indent=2))
    except (OSError, ValueError, SyntaxError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
