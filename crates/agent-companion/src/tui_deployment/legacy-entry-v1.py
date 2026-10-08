#!/usr/bin/python3 -B
# __BINDING_MARKER__
"""Dodex terminal entry; account paths are preserved from its original deployment."""
import json
import os
import runpy
import sys

BINDING = json.loads(__BINDING_JSON__)
VALUE_OPTIONS = {
    "--config", "--enable", "--disable", "--remote", "--remote-auth-token-env",
    "--model", "--local-provider", "--profile", "--sandbox", "--cd",
    "--add-dir", "--ask-for-approval",
}
BOOL_OPTIONS = {
    "--help", "--version", "--yolo", "--strict-config", "--oss", "--approve-for-me",
    "--dangerously-bypass-approvals-and-sandbox", "--dangerously-bypass-hook-trust",
    "--worktree", "--search", "--no-alt-screen", "--full-auto",
}
SHORT_VALUES = {"c": "--config", "i": "--image", "m": "--model", "p": "--profile",
                "s": "--sandbox", "C": "--cd", "a": "--ask-for-approval"}
SHORT_BOOLS = {"h": "--help", "V": "--version"}
OWNED_CONFIG = {"cli_auth_credentials_store", "sqlite_home", "log_dir"}
AUTH_ENV = {
    "OPENAI_API_KEY", "OPENAI_ACCESS_TOKEN", "OPENAI_ORG_ID", "OPENAI_ORGANIZATION",
    "OPENAI_PROJECT_ID", "CHATGPT_ACCESS_TOKEN", "OPENAI_BASE_URL", "OPENAI_API_BASE",
    "OPENAI_IDENTITY_TOKEN_FILE", "OPENAI_WORKLOAD_IDENTITY_CONTEXT", "OPENAI_FEDERATION_RULE_ID",
}


def option(args, index):
    token, end, pairs = args[index], index + 1, []
    if token.startswith("--"):
        name, equals, value = token.partition("=")
        if name in BOOL_OPTIONS and not equals:
            return end, [(name, None)]
        if name not in VALUE_OPTIONS | {"--image"}:
            raise ValueError("Unknown global option before app/update: " + name)
        attached = bool(equals)
    else:
        for position, letter in enumerate(token[1:]):
            if letter in SHORT_BOOLS:
                pairs.append((SHORT_BOOLS[letter], None))
                continue
            if letter not in SHORT_VALUES:
                raise ValueError("Unknown short option before app/update: -" + letter)
            name, value = SHORT_VALUES[letter], token[position + 2:]
            attached = bool(value)
            if value.startswith("="):
                value = value[1:]
            break
        else:
            return end, pairs
    if not attached:
        if end >= len(args) or args[end] == "--" or args[end].startswith("-"):
            raise ValueError("Missing or ambiguous option value")
        value, end = args[end], end + 1
    pairs.append((name, value))
    if name == "--image":
        while end < len(args) and not args[end].startswith("-"):
            pairs.append((name, args[end]))
            end += 1
    return end, pairs


def route(args):
    index, prefix = 0, []
    while index < len(args):
        token = args[index]
        if token == "--":
            return "native", None
        if not token.startswith("-") or token == "-":
            if token in {"app", "update"} and not any(name in {"--help", "--version"} for name, _ in prefix):
                return token, index
            return "native", None
        try:
            index, consumed = option(args, index)
        except ValueError:
            if any(value in {"app", "update"} for value in args[index + 1:]):
                raise
            return "native", None
        prefix.extend(consumed)
    return "native", None


def clean_environment(inherited):
    return {key: value for key, value in inherited.items()
           if (not key.startswith("CODEX_") or key in {"CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED"}
               or key.startswith("CODEX_NETWORK_")) and key not in AUTH_ENV
           and not key.startswith(("ELECTRON_", "DYLD_")) and key not in {"NODE_OPTIONS", "NODE_PATH"}}


def environment(inherited):
    env = clean_environment(inherited)
    env.update({
        "CODEX_HOME": BINDING["profile_home"],
        "CODEX_INSTALL_DIR": os.path.join(BINDING["profile_home"], "bin"),
        "CODEX_ELECTRON_USER_DATA_PATH": BINDING["desktop_data"],
        "CODEX_SQLITE_HOME": BINDING["sqlite_home"],
        "CODEX_CLI_PATH": os.path.join(BINDING["package"], "bin/codex"),
        "CODEX_APP_SERVER_FORCE_CLI": "1", "CODEX_APP_SERVER_USE_LOCAL_DAEMON": "0",
        "CODEX_SPARKLE_ENABLED": "false",
    })
    return env


def native_args(args):
    for index, token in enumerate(args):
        value = None
        if token in {"-c", "--config"} and index + 1 < len(args):
            value = args[index + 1]
        elif token.startswith("--config="):
            value = token[len("--config="):]
        elif token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            value = token[2:].removeprefix("=")
        if value and value.split("=", 1)[0].strip().strip("\"'") in OWNED_CONFIG:
            raise ValueError("Dodex owns its credential, SQLite and log paths; those overrides are disabled.")
    return [os.path.join(BINDING["package"], "bin/codex"), "-c", 'cli_auth_credentials_store="file"',
            "-c", "sqlite_home=" + json.dumps(BINDING["sqlite_home"]),
            "-c", "log_dir=" + json.dumps(BINDING["log_dir"])] + args


def main(args):
    action, index = route(args)
    if action == "app":
        # Reuse only the audited adapter's pure option parser. Its old launch
        # functions target a hidden runtime; the public mirror is current.
        adapter = runpy.run_path(BINDING["original"], run_name="dodex_saved_parser")
        _, index, prefix = adapter["route"](args)
        help_only, workspace = adapter["app_request"](args[index + 1:], prefix, os.getcwd())
        if not help_only:
            argv = ["/usr/bin/open", "-a", BINDING["app"]]
            if workspace is not None:
                argv = ["/usr/bin/open", "-n", "-a", BINDING["app"], "--args", "--open-project", str(workspace)]
            os.execve(argv[0], argv, clean_environment(os.environ))
    if action == "update" and args[index + 1:] not in (["--help"], ["-h"]):
        if index != 0 or len(args) != 1:
            raise ValueError("Use `dodex update` without options to open Companion's update controls.")
        os.execve(BINDING["companion"], [BINDING["companion"], "codex-tui", "--software-action", "update-all"], clean_environment(os.environ))
    argv = native_args(args)
    # exec preserves terminal I/O, working directory, arguments, signals, PID
    # and native exit status. Resume continues to use the same session files.
    os.execve(argv[0], argv, environment(os.environ))


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except Exception as error:
        print("Dodex: " + str(error), file=sys.stderr)
        sys.exit(1)
