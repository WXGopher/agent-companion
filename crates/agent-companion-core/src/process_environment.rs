//! Account isolation for child processes, independent of monitoring preferences.
//! Retain the caller's sandbox, network restrictions and proxy configuration.
use std::process::Command;

/// The native bootstrap and standalone adapters implement this same predicate;
/// their process tests exercise the shared policy's containment/identity matrix.
pub fn keep_variable(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if name.starts_with("CODEX_") {
        return matches!(
            name.as_str(),
            "CODEX_SANDBOX"
                | "CODEX_SANDBOX_NETWORK_DISABLED"
                | "CODEX_CA_CERTIFICATE"
                | "CODEX_PROXY_CERT"
        ) || name.starts_with("CODEX_NETWORK_");
    }
    !["OPENAI_", "CHATGPT_", "ELECTRON_", "DYLD_", "LD_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        && !matches!(name.as_str(), "NODE_OPTIONS" | "NODE_PATH")
}

/// Remove inherited credentials, session and executable overrides before a
/// caller sets its own verified account/runtime paths. Does not mutate the
/// process environment, read credentials, or weaken inherited containment.
pub fn isolate_command(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if !keep_variable(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_child_inherits_containment_without_parent_account_or_runtime_overrides() {
        const PHASE: &str = "ACOMP_ISOLATION_TEST_PHASE";
        const TEST: &str = "process_environment::tests::native_child_inherits_containment_without_parent_account_or_runtime_overrides";
        const REMOVED: &[&str] = &[
            "CODEX_CLI_PATH",
            "CODEX_CONFIG",
            "CODEX_CONFIG_FILE",
            "CODEX_PROFILE",
            "CODEX_MANAGED_PACKAGE_ROOT",
            "CODEX_THREAD_ID",
            "OPENAI_API_KEY",
        ];
        const KEPT: &[&str] = &[
            "CODEX_SANDBOX",
            "CODEX_NETWORK_PROXY_ACTIVE",
            "CODEX_SANDBOX_NETWORK_DISABLED",
            "CODEX_CA_CERTIFICATE",
            "HTTPS_PROXY",
        ];
        match std::env::var(PHASE).as_deref() {
            Ok("verify") => {
                for name in REMOVED {
                    assert!(std::env::var_os(name).is_none(), "leaked {name}");
                }
                for name in KEPT {
                    assert_eq!(std::env::var(name).as_deref(), Ok("synthetic"));
                }
            }
            phase => {
                let mut child = Command::new(std::env::current_exe().unwrap());
                child.args(["--exact", TEST]);
                if phase == Ok("launch") {
                    isolate_command(&mut child);
                    child.env(PHASE, "verify");
                } else {
                    child.env_clear().env(PHASE, "launch");
                    if let Some(root) = std::env::var_os("SystemRoot") {
                        child.env("SystemRoot", root);
                    }
                    for name in REMOVED.iter().chain(KEPT) {
                        child.env(name, "synthetic");
                    }
                }
                let output = child.output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
        }
    }

    #[test]
    fn account_isolation_preserves_containment_and_network_access_settings() {
        for name in [
            "HOME",
            "PATH",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "TERM",
            "CODEX_SANDBOX",
            "CODEX_SANDBOX_NETWORK_DISABLED",
            "CODEX_NETWORK_PROXY_ACTIVE",
            "CODEX_NETWORK_ALLOW_LOCAL_BINDING",
            "CODEX_CA_CERTIFICATE",
            "CODEX_PROXY_CERT",
        ] {
            assert!(keep_variable(name), "{name}");
        }
        for name in [
            "CODEX_HOME",
            "CODEX_THREAD_ID",
            "CODEX_CONFIG_FILE",
            "CODEX_CLI_PATH",
            "CODEX_APP_SERVER_URL",
            "CODEX_DAEMON_SOCKET",
            "OPENAI_API_KEY",
            "openai_access_token",
            "OPENAI_IDENTITY_TOKEN_FILE",
            "CHATGPT_ACCESS_TOKEN",
            "ELECTRON_RUN_AS_NODE",
            "NODE_OPTIONS",
            "NODE_PATH",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
        ] {
            assert!(!keep_variable(name), "{name}");
        }
    }
}
