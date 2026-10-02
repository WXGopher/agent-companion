#!/usr/bin/python3 -B
"""Dodex: B's native CLI by default; isolate the desktop and updater entry points."""
import importlib.util
import json
import mmap
import os
from pathlib import Path
import plistlib
import subprocess
import sys

sys.dont_write_bytecode = True
MANAGER_PATH = Path(__MANAGER_JSON__)
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


class AdapterError(Exception):
    pass


def load_manager():
    spec = importlib.util.spec_from_file_location("dodex_existing_manager", str(MANAGER_PATH))
    if spec is None or spec.loader is None:
        raise AdapterError("The existing B manager cannot be loaded.")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def option(args, index, app=False):
    """Consume one known option; return its end and normalized name/value pairs.

    --image accepts multiple consecutive values in the installed native CLI.
    Unknown option arity is never guessed when it could hide app/update.
    """
    token = args[index]
    pairs, end = [], index + 1
    if token.startswith("--"):
        name, equals, value = token.partition("=")
        if name in BOOL_OPTIONS and not equals:
            return end, [(name, None)]
        if name not in VALUE_OPTIONS | {"--image"} | ({"--download-url"} if app else set()):
            raise AdapterError("Unknown global option before app/update: " + name)
        attached = bool(equals)
    else:
        letters = token[1:]
        if not letters:
            raise AdapterError("Not an option.")
        for position, letter in enumerate(letters):
            if letter in SHORT_BOOLS:
                pairs.append((SHORT_BOOLS[letter], None))
                continue
            if letter not in SHORT_VALUES:
                raise AdapterError("Unknown short option before app/update: -" + letter)
            name = SHORT_VALUES[letter]
            value = letters[position + 1:]
            attached = bool(value)
            if value.startswith("="):
                value = value[1:]
            break
        else:
            return end, pairs
    if not attached:
        if end >= len(args) or args[end] == "--" or args[end].startswith("-"):
            raise AdapterError("Missing or ambiguous value for " + name)
        value = args[end]
        end += 1
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
            return "native", None, prefix
        if not token.startswith("-") or token == "-":
            if token in {"app", "update"}:
                if any(name in {"--help", "--version"} for name, _ in prefix):
                    return "native", None, prefix
                return token, index, prefix
            # A command (including help/exec/resume), or a prompt, owns its tail.
            return "native", None, prefix
        try:
            index, consumed = option(args, index)
        except AdapterError:
            if any(value in {"app", "update"} for value in args[index + 1:]):
                raise
            return "native", None, prefix
        prefix.extend(consumed)
    return "native", None, prefix


def native_manager(module):
    class NativeManager(module.Manager):
        def cli_arguments(self, args):
            # Reproduce the existing manager's fixed state/config protection, but
            # let the adapter's option-aware dispatcher own update detection.
            # In particular, `-- update` is a prompt, not an updater invocation.
            for index, token in enumerate(args):
                value = None
                if token in {"-c", "--config"} and index + 1 < len(args):
                    value = args[index + 1]
                elif token.startswith("--config="):
                    value = token[len("--config="):]
                elif token.startswith("-c") and not token.startswith("--") and len(token) > 2:
                    value = token[2:]
                    if value.startswith("="):
                        value = value[1:]
                if value and value.split("=", 1)[0].strip().strip("\"'") in OWNED_CONFIG:
                    raise module.ManagerError("Dodex owns its credential, SQLite and log paths; those overrides are disabled.")
            return [str(self.paths.cli), "-c", 'cli_auth_credentials_store="file"',
                    "-c", "sqlite_home=" + json.dumps(str(self.paths.home / "sqlite")),
                    "-c", "log_dir=" + json.dumps(str(self.paths.data / "logs"))] + list(args)
    return NativeManager()


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
        raise AdapterError("dodex app supports [PATH] and -C/--cd only. Native model/config, remote and download-url options are not applied to the desktop.")
    if len(values) > 1 or sum(name == "--cd" for name, _ in options) > 1:
        raise AdapterError("Use dodex app [PATH], with at most one -C/--cd directory.")
    if not values and not options:
        return False, None
    base = Path(cwd)
    if options:
        directory = Path(options[0][1]).expanduser()
        base = directory if directory.is_absolute() else base / directory
    requested = Path(values[0]).expanduser() if values else Path(".")
    workspace = Path(os.path.abspath(str(requested if requested.is_absolute() else base / requested)))
    if not workspace.is_dir() or str(workspace).strip() != str(workspace):
        raise AdapterError("The workspace must be an existing directory without trailing whitespace.")
    return False, workspace


def check_desktop(module, instance):
    instance.isolated_state()
    if instance.pending_journals():
        raise AdapterError("An interrupted B transaction needs recovery before opening Dodex.")
    module.verify_application(instance.paths.runtime)


def launch_custom_app(module, instance):
    with instance.lock():
        check_desktop(module, instance)
        app = instance.paths.launcher
        metadata = app / "Contents/Info.plist"
        module.no_symlink_components(metadata)
        with metadata.open("rb") as handle:
            name = plistlib.load(handle).get("CFBundleExecutable")
        if (not isinstance(name, str) or not name or name in {".", ".."}
                or "/" in name or any(ord(char) < 32 for char in name)):
            raise AdapterError("Invalid Dodex launcher executable in Info.plist.")
        executable = app / "Contents/MacOS" / name
        module.no_symlink_components(executable)
        if not executable.is_file() or not os.access(str(executable), os.X_OK):
            raise AdapterError("The installed Dodex launcher executable is unavailable.")
        # The manager lock fd is close-on-exec. No LaunchServices/open lookup occurs.
        os.execve(str(executable), [str(executable)], module.isolated_environment(instance.paths))
    return 0


def open_workspace(module, instance, workspace):
    with instance.lock():
        check_desktop(module, instance)
        archive = instance.paths.runtime / "Contents/Resources/app.asar"
        with archive.open("rb") as handle, mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as packed:
            for token in (b"--open-project", b"hasExplicitUserDataPath", b"requestSingleInstanceLock", b"queueSecondInstanceArgs"):
                if packed.find(token) < 0:
                    raise AdapterError("This runtime has not passed the workspace/second-instance compatibility check.")
        logs = instance.paths.data / "logs"
        module.private_directory(logs)
        fd = module.safe_open(logs / "desktop.log", os.O_WRONLY | os.O_APPEND | os.O_CREAT)
        with os.fdopen(fd, "ab", buffering=0) as log:
            process = subprocess.Popen(
                [str(instance.paths.runtime / "Contents/MacOS/ChatGPT"),
                 "--user-data-dir=" + str(instance.paths.data), "--open-project=" + str(workspace)],
                env=module.isolated_environment(instance.paths), cwd=str(instance.paths.data),
                stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                start_new_session=True, close_fds=True,
            )
        # Explicit B userdata enables Electron's B-specific single-instance lock.
        # A second B process forwards the workspace request to the existing B.
        instance.audit("dodex-open-workspace", pid=process.pid)
    print("Dodex: workspace request sent to B (process %s)." % process.pid)
    return 0


def main(argv=None):
    args = list(sys.argv[1:] if argv is None else argv)
    action, index, prefix = route(args)
    module = load_manager()
    instance = native_manager(module)
    if action == "native":
        return instance.cli(args)
    if action == "update":
        # Keep every option, including unsupported native globals: argparse will
        # explain them instead of silently discarding their requested behavior.
        return module.main(["update"] + args[:index] + args[index + 1:])
    help_only, workspace = app_request(args[index + 1:], prefix, os.getcwd())
    if help_only:
        return instance.cli(args)
    if workspace is None:
        return launch_custom_app(module, instance)
    return open_workspace(module, instance, workspace)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except Exception as error:
        # Do not print environment, auth files, native argv or tracebacks.
        print("Dodex: " + str(error), file=sys.stderr)
        sys.exit(1)
