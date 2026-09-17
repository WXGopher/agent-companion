//! Read-only Codex dashboard shared by the macOS surface and fixture tests.
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::codex::SessionCache;
use crate::state::{CodexClient, Phase, STALE_AFTER_SECS, SessionState};
use crate::usage::{self, CodexUsage, WindowUsage};

/// Reuse local quota readings for two minutes independently of task polling.
const USAGE_REFRESH_SECS: u64 = 120;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub active_count: usize,
    pub completed_count: usize,
    pub tasks: Vec<Task>,
    pub weekly: Option<Weekly>,
    pub error: Option<String>,
    pub codex_home: String,
    pub updated_at: u64,
    pub loading: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub project: String,
    pub cwd: Option<String>,
    pub client: &'static str,
    pub state: &'static str,
    pub updated_at: u64,
    pub transcript_path: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Weekly {
    pub used_percent: i64,
    pub resets_at: Option<u64>,
    pub expired: bool,
}

impl Weekly {
    fn from_window(window: WindowUsage, now: u64) -> Option<Self> {
        if !window.used_percent.is_finite() {
            return None;
        }
        Some(Self {
            used_percent: window.used_percent.clamp(0.0, 100.0).round() as i64,
            resets_at: window.resets_at,
            expired: window.resets_at.is_some_and(|at| at <= now),
        })
    }
}

pub struct Dashboard {
    home: PathBuf,
    sessions: SessionCache,
    last_sessions: Vec<SessionState>,
    sessions_scanned_at: Option<u64>,
    usage: Option<CodexUsage>,
    usage_error: Option<String>,
    usage_scanned_at: Option<u64>,
}

impl Dashboard {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            sessions: SessionCache::default(),
            last_sessions: vec![],
            sessions_scanned_at: None,
            usage: None,
            usage_error: None,
            usage_scanned_at: None,
        }
    }

    /// Called off the GUI thread. A missing home is a normal empty state.
    pub fn poll(&mut self, now: u64) -> Snapshot {
        let error = match self.sessions.scan(&self.home, now) {
            Ok(sessions) => {
                self.last_sessions = sessions;
                self.sessions_scanned_at = Some(now);
                None
            }
            Err(error) => {
                // Liveness was only observed during the last successful scan.
                // After a short grace period, let cached events expire normally
                // instead of keeping an inaccessible session active forever.
                if self
                    .sessions_scanned_at
                    .is_none_or(|at| now.saturating_sub(at) >= 30)
                {
                    for session in &mut self.last_sessions {
                        session.observed_alive = false;
                    }
                }
                Some(format!("Could not read Codex sessions: {error}"))
            }
        };
        self.last_sessions
            .retain(|session| !session.is_stale(now, STALE_AFTER_SECS));
        if self
            .usage_scanned_at
            .is_none_or(|at| now.saturating_sub(at) >= USAGE_REFRESH_SECS)
        {
            match usage::scan_codex_usage_at(&self.home) {
                Ok(usage) => {
                    self.usage = usage;
                    self.usage_error = None;
                }
                Err(cause) => {
                    self.usage_error = Some(format!("Could not read Codex usage: {cause}"));
                }
            }
            self.usage_scanned_at = Some(now);
        }
        let weekly = self.usage.as_ref().and_then(|usage| {
            // Prefer an explicitly weekly window. Legacy Codex readings omit
            // its duration; in that format secondary denotes the weekly limit.
            let window = [usage.secondary, usage.primary]
                .into_iter()
                .flatten()
                .find(|window| window.window_minutes == Some(10_080))
                .or_else(|| {
                    usage
                        .secondary
                        .filter(|window| window.window_minutes.is_none())
                })?;
            Weekly::from_window(window, now)
        });
        let mut snapshot = project(&self.last_sessions, &self.home, now);
        snapshot.weekly = weekly;
        snapshot.error = error.or_else(|| self.usage_error.clone());
        snapshot
    }
}

fn project(sessions: &[SessionState], home: &Path, now: u64) -> Snapshot {
    let mut snapshot = Snapshot {
        codex_home: home.to_string_lossy().into_owned(),
        updated_at: now,
        ..Snapshot::default()
    };
    for session in sessions {
        if session.source != crate::protocol::HookSource::Codex {
            continue;
        }
        let state = match session.phase {
            Phase::Running => "running",
            Phase::WaitingForApproval | Phase::WaitingForAnswer => "waiting",
            Phase::Completed => match session.last_event.as_str() {
                "turn_failed" => "failed",
                "turn_aborted" | "session_disconnected" => "stopped",
                _ => "completed",
            },
        };
        if session.phase == Phase::Completed {
            snapshot.completed_count += 1;
        } else {
            snapshot.active_count += 1;
        }
        let project = session
            .cwd
            .as_deref()
            .map(|cwd| {
                cwd.trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(cwd)
                    .to_owned()
            })
            .unwrap_or_else(|| "Codex".into());
        let title = session
            .display_name
            .as_deref()
            .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| project.clone());
        snapshot.tasks.push(Task {
            id: session.session_id.clone(),
            title: title.chars().take(200).collect(),
            project,
            cwd: session.cwd.clone(),
            client: match session.codex_client {
                CodexClient::Cli => "cli",
                CodexClient::Desktop => "desktop",
                CodexClient::Unknown => "unknown",
            },
            state,
            updated_at: session.last_seen,
            transcript_path: session.transcript_path.clone(),
        });
    }
    snapshot.tasks.sort_by_key(|task| {
        (
            !matches!(task.state, "running" | "waiting"),
            std::cmp::Reverse(task.updated_at),
            task.id.clone(),
        )
    });
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::HookSource;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn rollout_dashboard_follows_live_turns_titles_usage_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions/2026/09/05");
        std::fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join("rollout-dashboard.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        let start = crate::usage::parse_iso8601("2026-09-05T00:00:00Z").unwrap();
        for row in [
            json!({"timestamp":"2026-09-05T00:00:00Z", "type":"session_meta", "payload":{
                "id":"f36d990b-9a40-421d-b0a3-a1e36d38e218", "cwd":"/synthetic/project", "source":"cli"}}),
            json!({"timestamp":"2026-09-05T00:00:01Z", "type":"event_msg", "payload":{
                "type":"user_message", "message":"Implement\n the narrow notch", "turn_id":"t1"}}),
            json!({"timestamp":"2026-09-05T00:00:02Z", "type":"event_msg", "payload":{
                "type":"token_count", "rate_limits":{
                    "primary":{"used_percent":7.0,"window_minutes":300},
                    "secondary":{"used_percent":32.0,"window_minutes":10080,"resets_at":start+100}}}}),
        ] {
            writeln!(file, "{row}").unwrap();
        }
        let mut dashboard = Dashboard::new(dir.path().to_owned());
        let active = dashboard.poll(start + 3);
        assert_eq!((active.active_count, active.completed_count), (1, 0));
        assert_eq!(active.tasks[0].title, "Implement the narrow notch");
        assert_eq!(active.tasks[0].client, "cli");
        assert_eq!(active.weekly.unwrap().used_percent, 32);
        writeln!(
            file,
            "{}",
            json!({"timestamp":"2026-09-05T00:00:04Z","type":"event_msg","payload":{
            "type":"task_complete","turn_id":"t1"}})
        )
        .unwrap();
        let done = dashboard.poll(start + 5);
        assert_eq!((done.active_count, done.completed_count), (0, 1));
        assert_eq!(done.tasks[0].state, "completed");
        writeln!(
            file,
            "{}",
            json!({"timestamp":"2026-09-05T00:00:06Z","type":"event_msg","payload":{
                "type":"token_count","rate_limits":{
                    "secondary":{"used_percent":45.0,"window_minutes":10080,"resets_at":start+500}}}})
        )
        .unwrap();
        // Task completion was already visible. Quota stays cached until two
        // minutes after the first read, but its expiry is checked on each poll.
        for elapsed in [30, 119] {
            let cached = dashboard.poll(start + 3 + elapsed);
            let weekly = cached.weekly.unwrap();
            assert_eq!(weekly.used_percent, 32);
            assert_eq!(weekly.expired, elapsed >= 97);
        }
        let refreshed = dashboard.poll(start + 123).weekly.unwrap();
        assert_eq!(refreshed.used_percent, 45);
        assert!(!refreshed.expired);
        let expired = dashboard.poll(start + 1000);
        assert!(expired.tasks.is_empty());
        assert!(expired.weekly.unwrap().expired);
    }

    #[test]
    fn persistent_read_failure_does_not_keep_old_liveness_forever() {
        let dir = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::new(dir.path().to_owned());
        assert!(dashboard.poll(1000).error.is_none());
        let mut session = SessionState::new("old-live-task", HookSource::Codex, 100);
        session.observed_alive = true;
        dashboard.last_sessions.push(session);
        // A regular file in place of the directory reliably fails on both
        // platforms, including test runners allowed to bypass file permissions.
        let sessions = dir.path().join("sessions");
        std::fs::write(&sessions, "not a directory").unwrap();
        let transient = dashboard.poll(1010);
        assert!(transient.error.is_some());
        assert_eq!(transient.active_count, 1);
        let persistent = dashboard.poll(1030);
        assert!(persistent.error.is_some());
        assert_eq!(persistent.active_count, 0);
        assert!(persistent.tasks.is_empty());
        assert!(dashboard.poll(1120).error.is_some());
        std::fs::remove_file(sessions).unwrap();
        // Session reads recover immediately; quota errors retry after two minutes.
        assert!(dashboard.poll(1239).error.is_some());
        assert!(dashboard.poll(1240).error.is_none());
    }

    #[test]
    fn active_and_recently_finished_are_separate_and_only_codex_is_counted() {
        let running = SessionState::new("active", HookSource::Codex, 900);
        let mut done = SessionState::new("done", HookSource::Codex, 950);
        done.phase = Phase::Completed;
        done.display_name = Some("Finish\n  the parser".into());
        let claude = SessionState::new("other", HookSource::Claude, 999);
        let view = project(&[done, claude, running], Path::new("fixture"), 1000);
        assert_eq!((view.active_count, view.completed_count), (1, 1));
        assert_eq!(view.tasks[0].id, "active");
        assert_eq!(view.tasks[1].title, "Finish the parser");
    }

    #[test]
    fn missing_usage_is_not_zero_and_expired_usage_is_not_a_new_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::new(dir.path().join("missing"));
        let snapshot = dashboard.poll(1000);
        assert!(snapshot.tasks.is_empty() && snapshot.weekly.is_none());
        assert!(snapshot.error.is_none());
        let weekly = Weekly::from_window(
            WindowUsage {
                used_percent: 31.6,
                resets_at: Some(900),
                window_minutes: Some(10080),
            },
            1000,
        )
        .unwrap();
        assert_eq!(weekly.used_percent, 32);
        assert!(weekly.expired);
    }
}
