#!/usr/bin/python3 -B
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
    def keep(key):
        key = key.upper()
        if key.startswith("CODEX_"):
            return key in SAFETY_ENV | {"CODEX_CA_CERTIFICATE", "CODEX_PROXY_CERT"} or key.startswith("CODEX_NETWORK_")
        return not key.startswith(("OPENAI_", "CHATGPT_", "ELECTRON_", "DYLD_", "LD_")) and key not in {"NODE_OPTIONS", "NODE_PATH"}
    env = {key: value for key, value in inherited.items() if keep(key)}
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
