//! Console entry point: PowerShell must wait for the interactive TUI instead
//! of returning its own prompt while a GUI-subsystem agent-companion.exe reads input.
#[cfg(windows)]
mod launcher {
    use super::job;
    use std::{
        io,
        process::{Command, ExitCode, Stdio},
    };

    fn run() -> io::Result<std::process::ExitStatus> {
        let app = std::env::current_exe()?.with_file_name("agent-companion.exe");
        let job = job::Job::new()?;
        let mut child = Command::new(app)
            .arg("codex")
            .args(std::env::args_os().skip(1))
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        if let Err(error) = job.assign(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        child.wait()
    }

    pub fn main() -> ExitCode {
        match run() {
            Ok(status) => ExitCode::from(
                status
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .unwrap_or(1),
            ),
            Err(error) => {
                eprintln!("agent-companion-codex: {error}");
                ExitCode::FAILURE
            }
        }
    }
}
#[cfg(windows)]
#[path = "codex/job.rs"]
mod job;
#[cfg(windows)]
fn main() -> std::process::ExitCode {
    launcher::main()
}
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("agent-companion-codex is available on Windows only. Use agent-companion codex-tui.");
    std::process::ExitCode::FAILURE
}
