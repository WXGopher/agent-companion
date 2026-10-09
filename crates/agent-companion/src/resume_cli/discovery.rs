//! Session discovery uses TUI identities even when resident monitoring is off.
use agent_companion_core::{
    resume::Environment,
    tui_instance::{InstanceConfig, Layout},
};
use std::{io, path::PathBuf};

pub struct Discovery {
    pub environments: Vec<Environment>,
    pub managed_root: PathBuf,
    pub warnings: Vec<String>,
}

pub fn discover() -> io::Result<Discovery> {
    let layout = Layout::current()?;
    let mut environments = Vec::new();
    let mut warnings = Vec::new();
    let primary = crate::tui_deployment::primary_instance();
    if let Some(instance) = primary {
        environments.push(environment(instance));
    } else {
        // Local history remains inspectable when its native program is missing.
        // Keep the original identity; launch validation reports the missing TUI.
        let home = layout.user_home.join(".codex");
        environments.push(Environment {
            id: "codex".into(),
            label: "Codex".into(),
            executable: agent_companion_core::tui_instance::standalone_entry(&home),
            database_home: Some(agent_companion_core::dashboard::database_home(&home)),
            home,
        });
    }
    match layout.read() {
        Ok(Some(record)) => {
            let instance = record.dodex;
            // Runtime verification is performed by resume inspection. Never use
            // the primary binary as a substitute for a different Dodex version.
            environments.push(environment(instance));
        }
        Ok(None) => (),
        Err(error) => warnings.push(error.to_string()),
    }
    Ok(Discovery {
        environments,
        managed_root: layout.support.join("Resume"),
        warnings,
    })
}

fn environment(instance: InstanceConfig) -> Environment {
    // Registry entries remain stable across vendor updates and can be npm
    // scripts. Resume must execute the resolved native binary so the selected
    // account environment cannot be replaced by a wrapper.
    let executable = instance
        .runtime()
        .unwrap_or_else(|_| instance.cli_path.clone());
    Environment {
        id: instance.id,
        label: instance.label,
        home: instance.codex_home,
        executable,
        database_home: Some(instance.database_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_companion_core::tui_instance::{Channel, executable_name};
    use std::fs;

    #[test]
    fn npm_resume_discovery_resolves_native_runtime_and_keeps_history_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let package = root.join("node_modules/@openai/codex");
        let entry = package.join("bin/codex.js");
        let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => "x86_64-pc-windows-msvc",
            ("windows", "aarch64") => "aarch64-pc-windows-msvc",
            ("macos", "x86_64") => "x86_64-apple-darwin",
            ("macos", "aarch64") => "aarch64-apple-darwin",
            ("linux", "x86_64") => "x86_64-unknown-linux-musl",
            ("linux", "aarch64") => "aarch64-unknown-linux-musl",
            _ => return,
        };
        let native = package
            .join("vendor")
            .join(triple)
            .join("bin")
            .join(executable_name());
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&entry, "#!/usr/bin/env node\n").unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name":"@openai/codex","bin":{"codex":"bin/codex.js"}}"#,
        )
        .unwrap();
        fs::write(&native, b"MZ\0\0fixture, never executed").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let home = root.join("original-history");
        let database = root.join("original-database");
        let instance = InstanceConfig {
            id: "codex".into(),
            label: "Codex".into(),
            codex_home: home.clone(),
            database_dir: database.clone(),
            log_dir: home.join("log"),
            install_dir: root.join("bin"),
            cli_path: entry.clone(),
            command_path: root.join(executable_name()),
            channel: Channel::Npm,
            updater: None,
        };
        let found = environment(instance.clone());
        assert_eq!(found.executable, native.canonicalize().unwrap());
        assert_eq!(found.home, home);
        assert_eq!(found.database_home, Some(database.clone()));
        fs::remove_file(native).unwrap();
        let missing = environment(instance);
        assert_eq!(missing.executable, entry);
        assert_eq!(missing.home, home);
        assert_eq!(missing.database_home, Some(database));
    }
}
