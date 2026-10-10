//! Command-line Codex hook installation and diagnostics.
use crate::out::outln;
use agent_companion_core::install;
use clap::{Subcommand, ValueEnum};
use std::io;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Add Companion hooks to Codex's configuration.
    Install { agent: Agent },
    /// Remove Companion hooks while preserving user hooks.
    Uninstall { agent: Agent },
    /// Report the installed hook configuration.
    Status { agent: Agent },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Agent {
    Codex,
}

pub fn run(action: Action) -> io::Result<()> {
    let home = install::codex_home()?;
    match action {
        Action::Install {
            agent: Agent::Codex,
        } => {
            let stable = install::install_binaries()?;
            print_codex(&install::install_codex(&home, &stable.hook)?);
            outln!(
                "Open /hooks in Codex to review and trust the new hooks, then start a new session."
            );
        }
        Action::Uninstall {
            agent: Agent::Codex,
        } => print_codex(&install::uninstall_codex(&home)?),
        Action::Status {
            agent: Agent::Codex,
        } => {
            print_codex(&install::status_codex(&home)?);
            outln!(
                "Trust is managed by Codex: inspect /hooks to check whether these hooks can run."
            );
        }
    }
    Ok(())
}

fn print_codex(report: &install::CodexReport) {
    outln!("config   : {}", report.config_path.display());
    outln!("hooks    : {}", report.hooks_path.display());
    outln!(
        "feature  : {}",
        if report.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    for entry in &report.entries {
        outln!(
            "  {} {}",
            if entry.installed { "ok" } else { "--" },
            entry.event
        );
    }
    for backup in &report.backups {
        outln!("backup   : {}", backup.display());
    }
    outln!(
        "{}",
        if report.changed {
            "updated."
        } else {
            "no changes written."
        }
    );
}
