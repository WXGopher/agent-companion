//! Agent Companion's user-facing binary.
//!
//! On macOS, running without a subcommand starts the Codex menu bar app.
//! `codex-tui` opens the independent status bar editor; closing that editor exits
//! its process.
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
mod cli_install;
#[cfg(windows)]
mod codex;
mod codex_tui;
#[cfg(windows)]
mod headless;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_deployment;
#[cfg(target_os = "macos")]
mod macos_primary_app;
#[cfg(target_os = "macos")]
mod managed_tui;
mod out;
mod resume_cli;
#[cfg(target_os = "macos")]
mod software_updates;
#[cfg(any(target_os = "macos", windows))]
mod update_service;
#[cfg(any(target_os = "macos", windows))]
mod usage_service;
#[cfg(windows)]
mod windows_deployment;

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
    /// Continue an original Codex or Dodex session using a chosen quota account.
    Resume(resume_cli::Args),
    /// Install user-level acomp and agent-companion terminal commands.
    InstallCli,
    /// Check Dodex App; --sync copies the locally installed official Codex App.
    #[cfg(target_os = "macos")]
    DodexApp {
        #[arg(long, conflicts_with = "sync")]
        repair: bool,
        #[arg(long, conflicts_with = "repair")]
        sync: bool,
        #[arg(long, hide = true, conflicts_with_all = ["repair", "sync"])]
        sync_on_launch: Option<std::path::PathBuf>,
    },
    /// Open the explicitly deployed, isolated Dodex desktop instance.
    #[cfg(windows)]
    Dodex(DodexArgs),
    /// Open the standalone Codex CLI status bar editor. Apply, close, then restart Codex.
    CodexTui {
        /// Open Codex dual-instance settings and align or update Codex/Dodex.
        #[cfg(target_os = "macos")]
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

#[cfg(windows)]
#[derive(Debug, clap::Args)]
struct DodexArgs {
    /// Create or validate the isolated runtime and repair shell support, without opening it.
    #[arg(long, conflicts_with = "check")]
    deploy: bool,
    /// Check the locally installed official runtime without deploying it.
    #[arg(long)]
    check: bool,
}

/// Only leading deployment flags belong to the wrapper. Help, version, prompts,
/// subcommands and all other arguments belong to the official Codex CLI.
#[cfg(windows)]
#[derive(Debug, Parser)]
#[command(
    name = "dodex",
    disable_version_flag = true,
    about = "Manage the isolated second Codex environment. Run dodex to open its CLI."
)]
struct DodexManagementCli {
    #[command(flatten)]
    options: DodexArgs,
}

#[cfg(windows)]
fn is_dodex_management(arguments: &[std::ffi::OsString]) -> bool {
    arguments
        .first()
        .is_some_and(|argument| argument == "--deploy" || argument == "--check")
}

#[cfg(windows)]
fn is_dodex_executable(executable: &std::path::Path) -> bool {
    executable
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("dodex.exe"))
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
    #[cfg(windows)]
    let standalone_dodex =
        std::env::current_exe().is_ok_and(|executable| is_dodex_executable(&executable));
    // Only the subcommands belong on a terminal. The bare app must never
    // attach: launched from a shell-adjacent parent (a scripted start, a
    // hotkey runner), attaching would spill its startup banner into whatever
    // TUI happens to own that console.
    #[cfg(windows)]
    if standalone_dodex || std::env::args_os().nth(1).is_some() {
        attach_parent_console();
    }
    #[cfg(windows)]
    let cli = if standalone_dodex {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        if !is_dodex_management(&arguments) {
            match windows_deployment::launch_cli(&arguments) {
                // ExitCode only accepts u8; Windows child exit codes use all
                // 32 bits, including Ctrl+C and application-specific errors.
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(error) => {
                    errln!("dodex: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Cli {
            command: Some(Command::Dodex(DodexManagementCli::parse().options)),
        }
    } else {
        Cli::parse()
    };
    #[cfg(not(windows))]
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Resume(args)) => match resume_cli::run(&args) {
            Ok(code) => std::process::exit(code),
            Err(error) => Err(error),
        },
        Some(Command::InstallCli) => cli_install::run(),
        #[cfg(target_os = "macos")]
        Some(Command::DodexApp {
            repair,
            sync,
            sync_on_launch,
        }) => if let Some(app) = sync_on_launch {
            macos_deployment::sync_desktop_on_launch(&app)
        } else {
            (if sync {
                macos_deployment::sync_desktop()
            } else {
                macos_deployment::desktop_entry(repair)
            })
            .map(|message| println!("{message}"))
        }
        .map_err(std::io::Error::other),
        #[cfg(windows)]
        Some(Command::Dodex(DodexArgs { deploy, check })) => {
            let result = if check {
                windows_deployment::check_runtime()
            } else if deploy {
                windows_deployment::deploy().map(|status| status.message)
            } else {
                windows_deployment::launch(None).map(|_| "Dodex 已打开。".into())
            };
            result
                .map(|message| out::outln!("{message}"))
                .map_err(std::io::Error::other)
        }
        #[cfg(windows)]
        None => app::run(),
        #[cfg(target_os = "macos")]
        None | Some(Command::MenuBar) => macos::run_menu_bar(),
        #[cfg(all(not(windows), not(target_os = "macos")))]
        None => codex_tui::run(),
        Some(Command::CodexTui {
            #[cfg(target_os = "macos")]
            software_action,
        }) => {
            #[cfg(target_os = "macos")]
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

#[cfg(all(test, target_os = "macos"))]
mod macos_cli_tests {
    use super::*;

    #[test]
    fn startup_sync_is_hidden_and_cannot_be_combined_with_manual_actions() {
        let app = "/Applications/Dodex.app";
        assert!(matches!(
            Cli::try_parse_from(["agent-companion", "dodex-app", "--sync-on-launch", app])
                .unwrap()
                .command,
            Some(Command::DodexApp {
                repair: false,
                sync: false,
                sync_on_launch: Some(path),
            }) if path == std::path::Path::new(app)
        ));
        for option in ["--repair", "--sync"] {
            assert!(
                Cli::try_parse_from([
                    "agent-companion",
                    "dodex-app",
                    "--sync-on-launch",
                    app,
                    option,
                ])
                .is_err()
            );
        }
        assert!(
            Cli::try_parse_from(["agent-companion", "dodex-app", "--sync-on-launch",]).is_err()
        );
        let help = Cli::try_parse_from(["agent-companion", "dodex-app", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("--sync"));
        assert!(!help.contains("--sync-on-launch"));
    }
}

#[cfg(all(test, windows))]
mod cli_tests {
    use super::*;

    #[test]
    fn installed_dodex_has_its_own_cli_and_keeps_deployment_flags() {
        assert!(is_dodex_executable(std::path::Path::new(
            r"C:\用户\a & b's %tools%!\DoDeX.EXE"
        )));
        assert!(!is_dodex_executable(std::path::Path::new(
            "agent-companion.exe"
        )));
        assert!(!is_dodex_management(&[]));
        for first in ["--help", "--version", "resume", "exec", "-p", "-C", "-c"] {
            assert!(!is_dodex_management(&[first.into(), "--deploy".into()]));
        }
        assert!(is_dodex_management(&["--deploy".into()]));
        assert!(is_dodex_management(&["--check".into()]));
        assert!(
            DodexManagementCli::try_parse_from(["dodex", "--deploy"])
                .unwrap()
                .options
                .deploy
        );
        assert!(
            DodexManagementCli::try_parse_from(["dodex", "--check"])
                .unwrap()
                .options
                .check
        );
        assert!(DodexManagementCli::try_parse_from(["dodex", "--deploy", "--check"]).is_err());
        assert!(DodexManagementCli::try_parse_from(["dodex", "--deploy", "exec"]).is_err());
        assert!(DodexManagementCli::try_parse_from(["dodex", "--check", "-c", "x=1"]).is_err());
        let help = DodexManagementCli::try_parse_from(["dodex", "--deploy", "--help"]).unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(help.to_string().contains("Usage: dodex"));
        assert!(matches!(
            Cli::try_parse_from(["agent-companion", "dodex", "--deploy"])
                .unwrap()
                .command,
            Some(Command::Dodex(DodexArgs {
                deploy: true,
                check: false
            }))
        ));
    }
}
