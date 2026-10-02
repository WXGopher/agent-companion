#!/usr/bin/python3 -B
# __BINDING_MARKER__
"""Dodex terminal entry; account paths are preserved from its original deployment."""
import json
import os
import re
from pathlib import Path
import subprocess
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


def option(args, index, app=False):
    token, end, pairs = args[index], index + 1, []
    if token.startswith("--"):
        name, equals, value = token.partition("=")
        if name in BOOL_OPTIONS and not equals:
            return end, [(name, None)]
        if name not in VALUE_OPTIONS | {"--image"} | ({"--download-url"} if app else set()):
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
            # Unknown arity belongs to the native parser. A later prompt word
            # named app/update must not turn it into a management invocation.
            return "native", None
        prefix.extend(consumed)
    return "native", None


def clean_environment(inherited):
    def keep(key):
        key = key.upper()
        if key.startswith("CODEX_"):
            return key in {"CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED",
                           "CODEX_CA_CERTIFICATE", "CODEX_PROXY_CERT"} or key.startswith("CODEX_NETWORK_")
        return not key.startswith(("OPENAI_", "CHATGPT_", "ELECTRON_", "DYLD_", "LD_")) and key not in {"NODE_OPTIONS", "NODE_PATH"}
    return {key: value for key, value in inherited.items() if keep(key)}


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


# Arity for native subcommands, in addition to root/global options. Consume
# option values once; only real --config/-c options can affect isolation.
NATIVE_VALUES = VALUE_OPTIONS | {"--image", "--output-last-message", "--output-schema",
    "--color", "--env", "--url", "--bearer-token-env-var", "--listen", "--host",
    "--port", "--issuer-base-url", "--client-id", "--title", "--base", "--commit"}
NATIVE_SHORT_VALUES = {**SHORT_VALUES, "o": "--output-last-message"}


def config_values(args):
    index = 0
    while index < len(args):
        token, index = args[index], index + 1
        if token == "--":
            break
        name, value, attached = None, None, False
        if token.startswith("--"):
            name, equals, value = token.partition("=")
            attached = bool(equals)
            if name not in NATIVE_VALUES:
                continue
        elif token.startswith("-"):
            for position, letter in enumerate(token[1:]):
                if letter in NATIVE_SHORT_VALUES:
                    name = NATIVE_SHORT_VALUES[letter]
                    value = token[position + 2:]
                    attached = bool(value)
                    value = value.removeprefix("=")
                    break
                if letter not in SHORT_BOOLS:
                    break
            if name is None:
                continue
        else:
            continue
        if not attached:
            if index == len(args) or args[index] == "--":
                continue
            value, index = args[index], index + 1
        if name == "--config":
            yield value
        if name == "--image":
            while index < len(args) and not args[index].startswith("-"):
                index += 1


def owned_override(value):
    key, _, contents = value.partition("=")
    parts = [part.strip().strip("\"'") for part in key.strip().split(".")]
    if parts == [parts[0]] and parts[0] in OWNED_CONFIG:
        return True
    if parts[0] != "profiles":
        return False
    if len(parts) == 3:
        return parts[-1] in OWNED_CONFIG
    if len(parts) > 3:
        return False
    # The native config grammar permits whole inline profiles. Tokenize strings
    # as single tokens so ordinary text containing 'log_dir=...' stays a value.
    tokens = re.findall(r'"(?:\\.|[^"\\])*"|\'[^\']*\'|[A-Za-z0-9_-]+|[^\s]', contents)
    depth, isolation_depth = 0, 3 - len(parts)
    for index, token in enumerate(tokens[:-1]):
        if token == "{":
            depth += 1
        elif token == "}":
            depth -= 1
        elif depth == isolation_depth and token.strip("\"'") in OWNED_CONFIG and tokens[index + 1] == "=":
            return True
    return False


def native_args(args):
    for value in config_values(args):
        if owned_override(value):
            raise ValueError("Dodex owns its credential, SQLite and log paths; those overrides are disabled.")
    return [os.path.join(BINDING["package"], "bin/codex"), "-c", 'cli_auth_credentials_store="file"',
            "-c", "sqlite_home=" + json.dumps(BINDING["sqlite_home"]),
            "-c", "log_dir=" + json.dumps(BINDING["log_dir"])] + args


def validate_profile(args):
    request = [BINDING["companion"], "dodex-validate-profile",
               "--home", BINDING["profile_home"], "--sqlite", BINDING["sqlite_home"],
               "--logs", BINDING["log_dir"]]
    request.extend("--config=" + value for value in config_values(args))
    result = subprocess.run(request, env=clean_environment(os.environ),
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.PIPE, timeout=20, check=False)
    if result.returncode:
        # The validator's diagnostics never quote configuration values.
        raise ValueError(result.stderr.decode("utf-8", errors="replace").strip()
                         or "Profile isolation could not be verified; native CLI was not started.")


def app_request(args, prefix, cwd):
    """Return (help_only, workspace); reject options with no desktop meaning."""
    index, values, options, separated = 0, [], list(prefix), False
    while index < len(args):
        token = args[index]
        if not separated and token == "--":
            separated = True
            index += 1
        elif not separated and token.startswith("-") and token != "-":
            index, consumed = option(args, index, app=True)
            options.extend(consumed)
        else:
            values.append(token)
            index += 1
    if any(name == "--help" for name, _ in options):
        return True, None
    if any(name != "--cd" for name, _ in options):
        raise ValueError("dodex app supports [PATH] and -C/--cd only. Native model/config, remote and download-url options are not applied to the desktop.")
    if len(values) > 1 or sum(name == "--cd" for name, _ in options) > 1:
        raise ValueError("Use dodex app [PATH], with at most one -C/--cd directory.")
    if not values and not options:
        return False, None
    base = Path(cwd)
    if options:
        directory = Path(options[0][1]).expanduser()
        base = directory if directory.is_absolute() else base / directory
    requested = Path(values[0]).expanduser() if values else Path(".")
    workspace = Path(os.path.abspath(str(requested if requested.is_absolute() else base / requested)))
    if not workspace.is_dir() or str(workspace).strip() != str(workspace):
        raise ValueError("The workspace must be an existing directory without trailing whitespace.")
    return False, workspace


def main(args):
    action, index = route(args)
    if action == "app":
        prefix, cursor = [], 0
        while cursor < index:
            cursor, values = option(args, cursor)
            prefix.extend(values)
        help_only, workspace = app_request(args[index + 1:], prefix, os.getcwd())
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
    validate_profile(args)
    # exec preserves terminal I/O, working directory, arguments, signals, PID
    # and native exit status. Resume continues to use the same session files.
    os.execve(argv[0], argv, environment(os.environ))


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except Exception as error:
        print("Dodex: " + str(error), file=sys.stderr)
        sys.exit(1)
