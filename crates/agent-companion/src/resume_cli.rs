//! One original session, explicit quota choice, and inspectable local provenance.
mod discovery;
mod tui;

use agent_companion_core::resume::{self, Environment, Provenance, ResumeInspection, Session};
use std::io::{self, IsTerminal, Write};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Original session ID. Omit to search sessions in the exact current directory.
    pub session: Option<String>,
    /// History storage environment; required only if the same ID appears in both.
    #[arg(long, value_parser = ["codex", "dodex"])]
    pub source: Option<String>,
    /// Quota account for this run. Local settings still come from the source.
    #[arg(long, value_parser = ["codex", "dodex"])]
    pub account: Option<String>,
    /// Named profile from the source configuration.
    #[arg(short, long)]
    pub profile: Option<String>,
    /// List sessions in the exact current directory without starting Codex.
    #[arg(long, conflicts_with_all = ["session", "account", "details"])]
    pub list: bool,
    /// Show actual local provenance and compatibility reasons without launching.
    #[arg(long)]
    pub details: bool,
}

pub fn run(args: &Args) -> io::Result<i32> {
    let discovered = discovery::discover()?;
    for warning in &discovered.warnings {
        writeln!(io::stderr(), "acomp: {}", tui::clean(warning))?;
    }
    let cwd = std::env::current_dir()?.canonicalize()?;
    let mut sessions =
        resume::discover_sessions(&discovered.environments, &cwd).map_err(io::Error::other)?;
    if let Some(source) = &args.source {
        sessions.retain(|session| session.environment_id == *source);
    }
    if args.list {
        writeln!(
            io::stdout(),
            "History storage\tSession ID\tAvailability\tTitle"
        )?;
        for session in &sessions {
            writeln!(
                io::stdout(),
                "{}\t{}\t{}\t{}",
                tui::clean(&session.environment_id),
                tui::clean(&session.id),
                if session.busy {
                    "busy"
                } else if session.blockers.is_empty() {
                    "inspect account"
                } else {
                    "disabled"
                },
                tui::clean(&session.title)
            )?;
        }
        return Ok(0);
    }
    if sessions.is_empty() {
        return Err(io::Error::other(
            "No Codex or Dodex sessions match the exact current directory",
        ));
    }
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let session = if let Some(id) = &args.session {
        let found: Vec<_> = sessions
            .iter()
            .filter(|session| session.id == *id)
            .collect();
        match found.as_slice() {
            [session] => *session,
            [] => {
                return Err(io::Error::other(
                    "Session ID was not found in the exact current directory",
                ));
            }
            _ => {
                return Err(io::Error::other(
                    "Session ID exists in more than one history store; specify --source codex or --source dodex",
                ));
            }
        }
    } else {
        require_terminal(
            interactive,
            "Choose a session in a terminal, or supply its ID (use --list to inspect)",
        )?;
        let items: Vec<_> = sessions
            .iter()
            .map(|session| session_item(session, &discovered.environments))
            .collect();
        let Some(index) = tui::choose(
            "acomp resume · original session",
            &format!("Exact directory: {}", cwd.display()),
            &items,
        )?
        else {
            return Ok(0);
        };
        &sessions[index]
    };
    let source = environment(&discovered.environments, &session.environment_id)?;
    let mut accounts = Vec::new();
    for account in &discovered.environments {
        if args.account.as_ref().is_some_and(|id| *id != account.id) {
            continue;
        }
        let inspection = resume::inspect(source, account, session, args.profile.as_deref())
            .unwrap_or_else(|reason| unavailable_inspection(source, account, session, reason));
        accounts.push((account, inspection));
    }
    if accounts.is_empty() {
        return Err(io::Error::other(
            "Selected quota environment has no saved deployment",
        ));
    }
    if args.details {
        for (_, inspection) in &accounts {
            print_inspection(inspection, true)?;
        }
        return Ok(0);
    }
    let (account, inspection) = if args.account.is_some() {
        &accounts[0]
    } else {
        require_terminal(
            interactive,
            "Choose a quota account in a terminal, or specify --account codex or --account dodex; --details is read-only",
        )?;
        let items: Vec<_> = accounts
            .iter()
            .map(|(account, inspection)| account_item(account, inspection))
            .collect();
        let Some(index) = tui::choose(
            "acomp resume · this run's quota account",
            "History and local settings keep their original storage source",
            &items,
        )?
        else {
            return Ok(0);
        };
        &accounts[index]
    };
    print_inspection(inspection, false)?;
    if !inspection.blockers.is_empty() {
        return Err(io::Error::other(format!(
            "Resume is disabled: {}",
            inspection.blockers.join("; ")
        )));
    }
    let prepared = resume::prepare_from_inspection(
        source,
        account,
        session,
        args.profile.as_deref(),
        &discovered.managed_root,
        inspection,
    )
    .map_err(io::Error::other)?;
    let status = prepared.launch().map_err(io::Error::other)?;
    Ok(child_exit_code(status))
}

fn require_terminal(interactive: bool, message: &str) -> io::Result<()> {
    if interactive {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn environment<'a>(environments: &'a [Environment], id: &str) -> io::Result<&'a Environment> {
    environments
        .iter()
        .find(|environment| environment.id == id)
        .ok_or_else(|| {
            io::Error::other("The session's original storage environment is unavailable")
        })
}

fn session_item(session: &Session, environments: &[Environment]) -> tui::Item {
    let label = environments
        .iter()
        .find(|environment| environment.id == session.environment_id)
        .map(|environment| environment.label.as_str())
        .unwrap_or(&session.environment_id);
    let mut blockers = session.blockers.clone();
    if session.busy {
        blockers.push("The original session is open; exit its native client first".into());
    }
    tui::Item {
        title: format!("{} · {} · {}", label, session.title, session.id),
        search: format!("{} {} {}", label, session.title, session.id),
        summary: vec![
            format!("History storage       {label} · {}", session.id),
            "This run's quota      Choose on the next screen".into(),
            format!("Local instructions / memory    {label}"),
            format!("Model / tool settings          {label}"),
            "Project instructions           Current project".into(),
        ],
        details: vec![
            format!("History file: {}", session.rollout_path.display()),
            format!("Project: {}", session.cwd.display()),
            format!(
                "Original creation source: {}",
                session.creation_source.as_deref().unwrap_or("Unknown")
            ),
            "Quota selection shows the current configuration layers, files, and final model."
                .into(),
            "Local provenance does not assert account-side personalization.".into(),
        ],
        blockers,
    }
}

fn account_item(account: &Environment, inspection: &ResumeInspection) -> tui::Item {
    tui::Item {
        title: format!("{} · {}", account.label, inspection.account_label),
        search: format!(
            "{} {} {}",
            account.id, account.label, inspection.account_label
        ),
        summary: inspection
            .provenance
            .rows
            .iter()
            .map(|(key, value)| format!("{key:24} {value}"))
            .collect(),
        details: inspection
            .provenance
            .details
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect(),
        blockers: inspection.blockers.clone(),
    }
}

fn unavailable_inspection(
    source: &Environment,
    account: &Environment,
    session: &Session,
    reason: String,
) -> ResumeInspection {
    // Core inspection errors are deliberately redacted. Keep the combination
    // visible, but never label uninspected settings or credentials as verified.
    ResumeInspection {
        provenance: Provenance {
            rows: vec![
                (
                    "History storage".into(),
                    format!("{} · {}", source.label, session.id),
                ),
                ("Selected quota environment".into(), account.label.clone()),
                (
                    "Local settings".into(),
                    "Could not verify; resume disabled".into(),
                ),
            ],
            details: vec![
                (
                    "History file".into(),
                    session.rollout_path.to_string_lossy().into_owned(),
                ),
                (
                    "Original creation source".into(),
                    session
                        .creation_source
                        .clone()
                        .unwrap_or_else(|| "Unknown".into()),
                ),
                (
                    "Settings source directory".into(),
                    source.home.to_string_lossy().into_owned(),
                ),
                (
                    "Project directory".into(),
                    session.cwd.to_string_lossy().into_owned(),
                ),
            ],
        },
        blockers: vec![reason],
        account_label: "Identity unavailable".into(),
        account_identity: None,
    }
}

fn print_inspection(inspection: &ResumeInspection, expanded: bool) -> io::Result<()> {
    let mut output = io::stdout().lock();
    for (key, value) in &inspection.provenance.rows {
        writeln!(output, "{}: {}", tui::clean(key), tui::clean(value))?;
    }
    if expanded {
        for (key, value) in &inspection.provenance.details {
            writeln!(output, "  {}: {}", tui::clean(key), tui::clean(value))?;
        }
    }
    for reason in &inspection.blockers {
        writeln!(output, "Disabled: {}", tui::clean(reason))?;
    }
    writeln!(output)
}

fn child_exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map_or(1, |signal| 128 + signal)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

pub(crate) fn user_home() -> io::Result<std::path::PathBuf> {
    discovery::user_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: Args,
    }
    #[test]
    fn diagnostic_flags_never_imply_launch() {
        let cli = TestCli::try_parse_from(["acomp", "--list"]).unwrap();
        assert!(cli.args.list);
        assert!(
            TestCli::try_parse_from([
                "acomp",
                "id",
                "--details",
                "--account",
                "dodex",
                "--profile",
                "work"
            ])
            .unwrap()
            .args
            .details
        );
        assert!(TestCli::try_parse_from(["acomp", "--list", "--account", "codex"]).is_err());
        assert!(TestCli::try_parse_from(["acomp", "--account", "unrecognized"]).is_err());
    }
}
