//! Read-only launch guard shared by standalone terminal adapters.
use agent_companion_core::install::profile_sync::{self, IsolationPaths};
use std::{io, path::PathBuf};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[arg(long)]
    home: PathBuf,
    #[arg(long)]
    sqlite: PathBuf,
    #[arg(long)]
    logs: PathBuf,
    #[arg(long = "config", allow_hyphen_values = true)]
    overrides: Vec<String>,
}

pub(crate) fn run(args: Args) -> io::Result<()> {
    // The managed adapter supplies all isolation settings as native overrides.
    profile_sync::validate_isolated_profile_with_runtime_defaults(
        &args.home,
        &IsolationPaths {
            sqlite_home: args.sqlite,
            log_dir: args.logs,
        },
    )?;
    profile_sync::validate_isolated_overrides(&args.overrides)
}
