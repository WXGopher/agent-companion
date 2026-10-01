//! A console-subsystem entry point. Windows shells must wait while a resume
//! picker or native Codex process owns the terminal.
mod cli_install;
mod resume_cli;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "acomp",
    version,
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
    /// Install user-level acomp and agent-companion terminal commands.
    InstallCli,
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
