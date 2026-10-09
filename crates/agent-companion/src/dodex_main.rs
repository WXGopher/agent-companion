//! Console entry shared by the installed `codex` and `dodex` aliases.
//! Normal launches require only the TUI registry, never the Companion GUI.
use agent_companion_core::{
    codex_args,
    tui_instance::{self, Channel, Layout},
};
use std::{ffi::OsString, io, process::Command};

fn command() -> io::Result<Command> {
    let primary = std::env::current_exe()?
        .file_stem()
        .is_some_and(|name| name.eq_ignore_ascii_case("codex"));
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if !primary && codex_args::is_app_command(&arguments) {
        return Err(io::Error::other(tui_instance::APP_UNSUPPORTED));
    }
    let layout = Layout::current()?;
    let registry = layout.read()?.ok_or_else(|| {
        io::Error::other("TUI is not installed. Run agent-companion dodex-tui --install.")
    })?;
    let instance = if primary {
        registry
            .primary
            .ok_or_else(|| io::Error::other("The original Codex TUI installation is unavailable"))?
    } else {
        registry.dodex
    };
    // Homebrew remains Homebrew. Updating the primary alias never invokes the
    // standalone installer, copies its profile, or changes the Dodex selection.
    if primary
        && matches!(instance.channel, Channel::Homebrew | Channel::Npm)
        && codex_args::is_update_command(&arguments)
    {
        let mut command = Command::new(instance.updater.as_ref().ok_or_else(|| {
            io::Error::other("The original Codex package manager is unavailable")
        })?);
        instance.environment(&mut command);
        if instance.channel == Channel::Homebrew {
            command.args(["upgrade", "--cask", "codex"]);
        } else {
            command.args(["install", "--global", "@openai/codex@latest"]);
        }
        return Ok(command);
    }
    instance.command(&arguments)
}

fn main() -> std::process::ExitCode {
    match command().and_then(tui_instance::run_native) {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            let entry = std::env::current_exe()
                .ok()
                .and_then(|path| {
                    path.file_stem()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "dodex".into());
            eprintln!("{entry}: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
