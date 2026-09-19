//! Agent Companion's user-facing binary.
//!
//! On macOS, no subcommand opens the compact Codex notch. `codex-tui` opens
//! the independent status bar editor; closing that editor exits its process.
//! On Windows it runs the app: a usage readout in the taskbar, cards
//! for the approvals a session needs, a tray icon, and the named-pipe server
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
#[cfg(windows)]
mod codex;
mod codex_tui;
#[cfg(windows)]
mod headless;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_deployment;
mod out;

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
#[command(name = "agent-companion", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the standalone Codex CLI status bar editor. Apply, close, then restart Codex.
    CodexTui,
    /// Show the compact Codex task and weekly usage notch.
    #[cfg(target_os = "macos")]
    Notch,
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
        #[cfg(windows)]
        None => app::run(),
        #[cfg(target_os = "macos")]
        None | Some(Command::Notch) => macos::run(),
        #[cfg(all(not(windows), not(target_os = "macos")))]
        None => codex_tui::run(),
        Some(Command::CodexTui) => codex_tui::run(),
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
