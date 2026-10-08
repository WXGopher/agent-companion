//! Session discovery uses TUI identities even when resident monitoring is off.
use agent_companion_core::{resume::Environment, tui_instance::Layout};
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
        environments.push(Environment {
            id: instance.id,
            label: instance.label,
            home: instance.codex_home,
            executable: instance.cli_path,
            database_home: Some(instance.database_dir),
        });
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
            environments.push(Environment {
                id: instance.id,
                label: instance.label,
                home: instance.codex_home,
                executable: instance.cli_path,
                database_home: Some(instance.database_dir),
            });
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

pub fn user_home() -> io::Result<PathBuf> {
    Ok(Layout::current()?.user_home)
}
