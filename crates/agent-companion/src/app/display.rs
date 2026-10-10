//! Persist the last display and resume it only when an agent produces activity.
//! Quota caches and network replies are readings, never signs of life.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use agent_companion_core::protocol::HookSource;
use agent_companion_core::state::{AgentTasks, STALE_AFTER_SECS};
use serde::{Deserialize, Serialize};

use super::{AGENTS, config, task_status, ui};

const SAVE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AgentState {
    last_seen: Option<u64>,
    visible: bool,
    ended: bool,
}

/// Display text only. Approval connections and terminal targets must come from
/// live hooks, so neither is restored from disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SavedSession {
    id: String,
    title: String,
    detail: String,
    phase: String,
    source: String,
}

impl From<ui::SessionRow> for SavedSession {
    fn from(row: ui::SessionRow) -> Self {
        Self {
            id: row.id.to_string(),
            title: row.title.to_string(),
            detail: row.detail.to_string(),
            phase: row.phase.to_string(),
            source: row.source.to_string(),
        }
    }
}

impl SavedSession {
    fn row(&self) -> ui::SessionRow {
        ui::SessionRow {
            id: self.id.clone().into(),
            title: self.title.clone().into(),
            detail: self.detail.clone().into(),
            phase: task_status::saved_phase(&self.phase, &self.detail).into(),
            source: self.source.clone().into(),
            jumpable: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Snapshot {
    codex: AgentState,
    sessions: Vec<SavedSession>,
    updated_at: u64,
}

pub struct DisplayState {
    snapshot: Snapshot,
    live: bool,
    dirty: bool,
    last_save: Option<Instant>,
}

impl DisplayState {
    pub fn load() -> Self {
        let snapshot = config::config_dir()
            .ok()
            .and_then(|dir| read_snapshot(&dir.join("display.json")))
            .unwrap_or_default();
        Self::restore(snapshot)
    }

    fn restore(mut snapshot: Snapshot) -> Self {
        let previous_count = snapshot.sessions.len();
        snapshot
            .sessions
            .retain(|session| HookSource::parse(&session.source).is_some());
        let removed_sessions = previous_count != snapshot.sessions.len();
        Self {
            snapshot,
            live: false,
            dirty: removed_sessions,
            last_save: None,
        }
    }

    fn agent(&self, _source: HookSource) -> &AgentState {
        &self.snapshot.codex
    }
    fn agent_mut(&mut self, _source: HookSource) -> &mut AgentState {
        &mut self.snapshot.codex
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    pub fn visible(&self, source: HookSource) -> bool {
        self.agent(source).visible
    }

    pub fn saved_sessions(&self, limit: usize) -> Vec<ui::SessionRow> {
        self.snapshot
            .sessions
            .iter()
            .take(limit)
            .map(SavedSession::row)
            .collect()
    }

    pub fn saved_tasks(&self, source: HookSource) -> AgentTasks {
        let mut tasks = AgentTasks::default();
        for session in self
            .snapshot
            .sessions
            .iter()
            .filter(|row| row.source == source.as_str())
        {
            match session.phase.as_str() {
                "running" => tasks.running += 1,
                "waitingForApproval" | "waitingForAnswer" => tasks.pending += 1,
                phase if task_status::is_finished(phase) => tasks.done += 1,
                _ => (),
            }
        }
        tasks
    }

    pub fn saved_outcomes(&self, source: HookSource) -> task_status::TaskOutcomes {
        task_status::TaskOutcomes::from_phases(
            self.snapshot
                .sessions
                .iter()
                .filter(|row| row.source == source.as_str())
                .map(|row| task_status::saved_phase(&row.phase, &row.detail)),
        )
    }

    /// Existing log records can help validate an agent at the next hook. They
    /// keep their original timestamp and do not change the restored display.
    pub fn observe(&mut self, source: HookSource, at: u64) {
        let agent = self.agent_mut(source);
        if agent.last_seen.is_none_or(|previous| at > previous) {
            agent.last_seen = Some(at);
            self.dirty = true;
        }
    }

    /// Called by a hook, a new Codex event, or a confirmed live writer. Expiry is checked
    /// here, never at startup, on a timer, or because a quota fetch succeeded.
    pub fn activate(&mut self, source: HookSource, at: u64, now: u64) {
        self.observe(source, at);
        self.agent_mut(source).ended = false;
        self.live = true;
        for source in AGENTS {
            let agent = self.agent_mut(source);
            agent.visible = !agent.ended
                && agent
                    .last_seen
                    .is_some_and(|seen| now.saturating_sub(seen) < STALE_AFTER_SECS);
        }
        self.snapshot.updated_at = now;
        self.dirty = true;
    }

    pub fn end_agent(&mut self, source: HookSource) {
        *self.agent_mut(source) = AgentState {
            ended: true,
            ..Default::default()
        };
        self.dirty = true;
    }

    pub fn remember(&mut self, sessions: Vec<ui::SessionRow>, now: u64) {
        if !self.live {
            return;
        }
        self.snapshot.sessions = sessions.into_iter().map(SavedSession::from).collect();
        self.snapshot.updated_at = now;
        self.dirty = true;
    }

    /// Coalesce rapid hooks; a clean shutdown always flushes the final state.
    pub fn save(&mut self, force: bool) {
        if !self.dirty
            || (!force
                && self
                    .last_save
                    .is_some_and(|at| at.elapsed() < SAVE_INTERVAL))
        {
            return;
        }
        self.last_save = Some(Instant::now());
        let result = config::config_dir()
            .and_then(|dir| write_snapshot(&dir.join("display.json"), &self.snapshot));
        if result.is_ok() {
            self.dirty = false;
        }
    }
}

fn read_snapshot(path: &Path) -> Option<Snapshot> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(snapshot).map_err(io::Error::other)?;
    std::fs::write(&temporary, body)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const THEN: u64 = 1_787_000_000;
    const CODEX: HookSource = HookSource::Codex;
    fn previous_display() -> Snapshot {
        Snapshot {
            codex: AgentState {
                visible: true,
                last_seen: Some(THEN),
                ..Default::default()
            },
            sessions: vec![SavedSession {
                id: "saved".into(),
                title: "project".into(),
                detail: "Working".into(),
                phase: "running".into(),
                source: "codex".into(),
            }],
            updated_at: THEN,
        }
    }
    #[test]
    fn startup_keeps_saved_tasks_until_new_activity() {
        let snapshot = previous_display();
        let mut display = DisplayState::restore(snapshot.clone());
        display.observe(CODEX, THEN + 10);
        display.remember(vec![], THEN + 20);
        assert!(!display.is_live());
        assert!(display.visible(CODEX));
        assert_eq!(display.snapshot.sessions, snapshot.sessions);
        assert_eq!(display.saved_tasks(CODEX).running, 1);
        assert!(!display.saved_sessions(1)[0].jumpable);
        display.activate(CODEX, THEN + 30, THEN + 30);
        display.remember(vec![], THEN + 30);
        assert!(display.is_live());
        assert!(display.saved_sessions(1).is_empty());
    }
    #[test]
    fn obsolete_saved_sources_are_removed_without_restoring_quota() {
        let mut value = serde_json::to_value(previous_display()).unwrap();
        value["sessions"][0]["source"] = serde_json::json!("unsupported");
        value["usage"] = serde_json::json!({"codex":{"primary":{"used_percent":25}}});
        let snapshot: Snapshot = serde_json::from_value(value).unwrap();
        let display = DisplayState::restore(snapshot);
        assert!(display.saved_sessions(1).is_empty());
        assert!(display.dirty);
        assert!(
            serde_json::to_value(&display.snapshot)
                .unwrap()
                .get("usage")
                .is_none()
        );
    }
    #[test]
    fn old_observations_do_not_extend_liveness_or_reactivate_an_ended_session() {
        let mut display = DisplayState::restore(previous_display());
        display.observe(CODEX, THEN - 1);
        assert_eq!(display.agent(CODEX).last_seen, Some(THEN));
        display.activate(CODEX, THEN, THEN + STALE_AFTER_SECS);
        assert!(!display.visible(CODEX));
        display.activate(CODEX, THEN + 1, THEN + 1);
        assert!(display.visible(CODEX));
        display.end_agent(CODEX);
        display.observe(CODEX, THEN + 1);
        assert!(!display.visible(CODEX));
        display.activate(CODEX, THEN + 2, THEN + 2);
        assert!(display.visible(CODEX));
    }
    #[test]
    fn snapshots_replace_atomically_and_invalid_json_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/display.json");
        assert!(read_snapshot(&path).is_none());
        let mut snapshot = previous_display();
        write_snapshot(&path, &snapshot).unwrap();
        assert_eq!(read_snapshot(&path), Some(snapshot.clone()));
        snapshot.sessions.clear();
        write_snapshot(&path, &snapshot).unwrap();
        assert_eq!(read_snapshot(&path), Some(snapshot));
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read_snapshot(&path).is_none());
    }
    #[test]
    fn saved_task_status_migrates_failures_and_stops_without_changing_finished_counts() {
        let mut snapshot = previous_display();
        snapshot.sessions = [
            ("failed", "Failed · Open in Codex"),
            ("stopped", "Interrupted"),
            ("done", "Done"),
        ]
        .into_iter()
        .map(|(id, detail)| SavedSession {
            id: id.into(),
            title: "fixture".into(),
            detail: detail.into(),
            phase: "completed".into(),
            source: "codex".into(),
        })
        .collect();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("display.json");
        write_snapshot(&path, &snapshot).unwrap();
        let display = DisplayState::restore(read_snapshot(&path).unwrap());
        assert_eq!(display.saved_tasks(CODEX).done, 3);
        assert_eq!(
            display.saved_outcomes(CODEX),
            task_status::TaskOutcomes {
                failed: 1,
                stopped: 1
            }
        );
        let rows = display.saved_sessions(10);
        assert_eq!(
            rows.iter()
                .map(|row| row.phase.as_str())
                .collect::<Vec<_>>(),
            ["failed", "stopped", "completed"]
        );
        assert!(
            rows.iter()
                .all(|row| task_status::is_finished(row.phase.as_str()))
        );
    }
}
