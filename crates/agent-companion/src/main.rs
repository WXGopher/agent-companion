//! Agent Companion's user-facing binary.
//!
//! On macOS, running without a subcommand starts the Codex menu bar app.
//! `codex-tui` opens the independent status bar editor; closing that editor exits
//! its process.
//! On Windows it runs the app: a usage readout in the taskbar, cards
//! for the approvals a session needs, the taskbar readout, and the named-pipe server
//! that feeds them all. The subcommands are the parts that have to
//! work from a terminal — `headless` for watching the raw event stream, `setup`
//! for hook installation, and `statusline` for the usage bridge Claude Code
//! invokes on every turn.
//!
//! Built for the GUI subsystem so that double-clicking `agent-companion.exe` — or running
//! it at login — opens no console window. The subcommands get their terminal
//! back through [`attach_parent_console`].
#![windows_subsystem = "windows"]

#[cfg(windows)]
mod app;
mod cli_install;
#[cfg(windows)]
mod codex;
mod codex_tui;
#[cfg(windows)]
mod headless;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_primary_app;
mod out;
mod resume_cli;
#[cfg(any(target_os = "macos", windows))]
mod settings_update;
#[cfg(any(target_os = "macos", windows))]
mod software_updates;
#[cfg(any(target_os = "macos", windows))]
mod tui_deployment;
#[cfg(any(target_os = "macos", windows))]
mod update_service;
#[cfg(any(target_os = "macos", windows))]
mod usage_service;

pub mod ui {
    slint::include_modules!();
}
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod single;
#[cfg(windows)]
mod statusline;
#[cfg(windows)]
mod usage_cache;
#[cfg(windows)]
mod util;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::out::errln;

/// Codex task and usage companion, with a native CLI status bar editor.
#[derive(Debug, Parser)]
#[command(name = "agent-companion", version = env!("AGENT_COMPANION_VERSION"), about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Update one TUI through its own installation channel.
    #[cfg(any(target_os = "macos", windows))]
    SoftwareMaintenance {
        #[arg(value_enum)]
        action: software_updates::Action,
    },
    /// Continue an original Codex or Dodex session using a chosen quota account.
    Resume(resume_cli::Args),
    /// Install user-level acomp, agent-companion, and dodex terminal commands.
    InstallCli,
    /// Install, repair, or inspect the independent Dodex TUI.
    #[cfg(any(target_os = "macos", windows))]
    DodexTui {
        #[arg(long, conflicts_with = "repair")]
        install: bool,
        #[arg(long)]
        repair: bool,
    },
    /// Open the standalone Codex CLI status bar editor. Apply, close, then restart Codex.
    CodexTui {
        /// Open TUI dual-instance settings and update the selected TUI.
        #[cfg(any(target_os = "macos", windows))]
        #[arg(long, value_enum)]
        software_action: Option<software_updates::Action>,
    },
    /// Show Codex tasks and usage in the menu bar.
    #[cfg(target_os = "macos")]
    MenuBar,
    /// Watch the hook event stream in a terminal, with no windows at all.
    #[cfg(windows)]
    Headless(headless::Args),
    /// Install, remove, or inspect Agent Companion's hook wiring for an agent.
    #[cfg(windows)]
    Setup {
        #[command(subcommand)]
        action: setup::Action,
    },
    /// Render a status line from a payload on stdin (used by Claude Code).
    #[cfg(windows)]
    Statusline,
    /// Launch Codex with Agent Companion question cards (experimental app-server transport).
    #[cfg(windows)]
    Codex(codex::Args),
}

/// A GUI-subsystem process launched from a shell starts with no console and no
/// standard handles, which would make every subcommand silent. Attaching to the
/// parent's console gives them back — but only when the handles are actually
/// missing: when a parent piped them (Claude Code running `statusline`, the
/// test harness running `headless`), attaching would clobber the redirection.
/// With no console to attach to — the double-click case — this is a no-op.
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    let missing = |handle| unsafe {
        !GetStdHandle(handle).is_ok_and(|handle| !handle.is_invalid() && !handle.0.is_null())
    };
    if missing(STD_OUTPUT_HANDLE) && missing(STD_ERROR_HANDLE) {
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

fn main() -> ExitCode {
    // Only the subcommands belong on a terminal. The bare app must never
    // attach: launched from a shell-adjacent parent (a scripted start, a
    // hotkey runner), attaching would spill its startup banner into whatever
    // TUI happens to own that console.
    #[cfg(windows)]
    if std::env::args_os().nth(1).is_some() {
        attach_parent_console();
    }
    let cli = Cli::parse();
    let result = match cli.command {
        #[cfg(any(target_os = "macos", windows))]
        Some(Command::SoftwareMaintenance { action }) => software_updates::run_action(action)
            .map(|snapshot| {
                for row in snapshot.rows {
                    out::outln!("{}: {} — {}", row.name, row.current, row.message);
                }
                out::outln!("{}", snapshot.message);
            })
            .map_err(std::io::Error::other),
        Some(Command::Resume(args)) => match resume_cli::run(&args) {
            Ok(code) => std::process::exit(code),
            Err(error) => Err(error),
        },
        Some(Command::InstallCli) => cli_install::run(),
        #[cfg(any(target_os = "macos", windows))]
        Some(Command::DodexTui { install, repair }) => {
            let result = if install || repair {
                tui_deployment::install_and_configure()
            } else {
                Ok(tui_deployment::status())
            };
            result
                .map(|status| out::outln!("{}", status.message))
                .map_err(std::io::Error::other)
        }
        #[cfg(windows)]
        None => app::run(),
        #[cfg(target_os = "macos")]
        None | Some(Command::MenuBar) => macos::run_menu_bar(),
        #[cfg(all(not(windows), not(target_os = "macos")))]
        None => codex_tui::run(),
        Some(Command::CodexTui {
            #[cfg(any(target_os = "macos", windows))]
            software_action,
        }) => {
            #[cfg(any(target_os = "macos", windows))]
            if let Some(action) = software_action {
                software_updates::request(action);
            }
            codex_tui::run()
        }
        #[cfg(windows)]
        Some(Command::Headless(args)) => headless::run(&args),
        #[cfg(windows)]
        Some(Command::Setup { action }) => setup::run(action),
        #[cfg(windows)]
        Some(Command::Statusline) => statusline::run(),
        #[cfg(windows)]
        Some(Command::Codex(args)) => codex::run(&args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            errln!("agent-companion: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(all(test, any(target_os = "macos", windows)))]
mod cli_tests {
    use super::*;
    #[test]
    fn tui_install_and_repair_replace_desktop_management() {
        assert!(matches!(
            Cli::try_parse_from(["agent-companion", "dodex-tui", "--install"])
                .unwrap()
                .command,
            Some(Command::DodexTui {
                install: true,
                repair: false
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["agent-companion", "dodex-tui", "--repair"])
                .unwrap()
                .command,
            Some(Command::DodexTui {
                install: false,
                repair: true
            })
        ));
        assert!(
            Cli::try_parse_from(["agent-companion", "dodex-tui", "--install", "--repair"]).is_err()
        );
        assert!(Cli::try_parse_from(["agent-companion", "dodex-app"]).is_err());
        for action in ["update-codex", "update-dodex"] {
            assert!(
                Cli::try_parse_from(["agent-companion", "software-maintenance", action]).is_ok()
            );
        }
        for action in ["align", "update-all"] {
            assert!(
                Cli::try_parse_from(["agent-companion", "software-maintenance", action]).is_err()
            );
        }
    }
}
