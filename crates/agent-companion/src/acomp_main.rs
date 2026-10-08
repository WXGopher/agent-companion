//! A console-subsystem entry point. Windows shells must wait while a resume
//! picker or native Codex process owns the terminal.
mod cli_install;
mod resume_cli;
#[allow(dead_code)]
mod tui_deployment;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "acomp",
    version = env!("AGENT_COMPANION_VERSION"),
    about = "Resume Codex and Dodex sessions with a chosen quota account",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Continue an original session, preserving its history and local settings.
    Resume(resume_cli::Args),
    /// Install user-level acomp, agent-companion, and dodex terminal commands.
    InstallCli,
    /// Install or repair the independent Dodex TUI.
    DodexTui {
        #[arg(long, conflicts_with = "repair")]
        install: bool,
        #[arg(long)]
        repair: bool,
    },
}

fn main() -> std::process::ExitCode {
    // The Windows installer provides both aliases from this console binary.
    let name = if std::env::current_exe()
        .ok()
        .and_then(|path| path.file_stem().map(|name| name.to_owned()))
        .is_some_and(|name| name.eq_ignore_ascii_case("agent-companion"))
    {
        "agent-companion"
    } else {
        "acomp"
    };
    let matches = Cli::command().name(name).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let result = match cli.command {
        Command::Resume(args) => resume_cli::run(&args),
        Command::InstallCli => cli_install::run().map(|()| 0),
        Command::DodexTui { install, repair } => {
            let status = if install || repair {
                tui_deployment::install_and_configure()
            } else {
                Ok(tui_deployment::status())
            };
            status
                .map(|status| {
                    println!("{}", status.message);
                    0
                })
                .map_err(std::io::Error::other)
        }
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "acomp: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
