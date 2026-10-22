//! Retained session presentation, separate from the live lifecycle table.
//! Quota reads, repainting, and waiting-pipe keepalives never refresh activity.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use agent_companion_core::protocol::HookSource;
use agent_companion_core::state::{AgentTasks, Phase, STALE_AFTER_SECS, SessionState};
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
#[serde(rename_all = "camelCase")]
struct SavedSession {
    id: String,
    title: String,
    detail: String,
    phase: String,
    source: String,
    /// Missing only in snapshots written before per-session retention existed.
    #[serde(default)]
    last_seen: Option<u64>,
    /// Legacy snapshots had one global timestamp. Keep its provenance until
    /// authentic session evidence can replace the approximation, even earlier.
    #[serde(default)]
    last_seen_estimated: bool,
}

/// A current lifecycle observation. Current writer/wait evidence is transient:
/// restoring a saved "waiting" label must not restore a live approval.
pub struct SessionObservation {
    row: ui::SessionRow,
    last_seen: u64,
    confirmed_live: bool,
    waiting: bool,
}

impl SessionObservation {
    pub fn new(row: ui::SessionRow, state: &SessionState) -> Self {
        Self {
            row,
            last_seen: state.last_seen,
            confirmed_live: state.phase != Phase::Completed
                && (state.observed_alive
                    || !state.pending.is_empty()
                    || !state.observed_questions.is_empty()),
            waiting: state.phase.is_waiting(),
        }
    }
}

impl SavedSession {
    fn key(&self) -> (String, String) {
        (self.source.clone(), self.id.clone())
    }

    fn inactive(&self, now: u64, confirmed_live: bool) -> bool {
        task_status::is_finished(&self.phase)
            || (!confirmed_live
                && now.saturating_sub(self.last_seen.unwrap_or(0)) >= STALE_AFTER_SECS)
    }

    fn phase(&self, now: u64, confirmed_live: bool) -> &str {
        if self.inactive(now, confirmed_live) && !task_status::is_finished(&self.phase) {
            "stopped"
        } else {
            task_status::saved_phase(&self.phase, &self.detail)
        }
    }

    fn row(&self, now: u64, confirmed_live: bool) -> ui::SessionRow {
        let inactive = self.inactive(now, confirmed_live) && !task_status::is_finished(&self.phase);
        ui::SessionRow {
            id: self.id.clone().into(),
            title: self.title.clone().into(),
            detail: if inactive {
                "Inactive".into()
            } else {
                self.detail.clone().into()
            },
            phase: self.phase(now, confirmed_live).into(),
            source: self.source.clone().into(),
            jumpable: false,
        }
    }
}

/// Keep a dismissal's activity watermark, so another poll or restart cannot
/// restore the same row. Account and id together are the identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DismissedSession {
    source: String,
    id: String,
    last_seen: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Snapshot {
    codex: AgentState,
    sessions: Vec<SavedSession>,
    updated_at: u64,
    dismissed: Vec<DismissedSession>,
}

pub struct DisplayState {
    snapshot: Snapshot,
    live: bool,
    dirty: bool,
    last_save: Option<Instant>,
    confirmed_live: HashMap<(String, String), bool>,
    /// The native question pipe refreshes the lifecycle TTL while it waits.
    /// Remember its actual event time before that synthetic clock adjustment.
    keepalive_activity: HashMap<(String, String), (u64, u64)>,
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
        let previous = snapshot.clone();
        snapshot
            .sessions
            .retain(|session| supported_account(&session.source));
        snapshot
            .dismissed
            .retain(|session| supported_account(&session.source));
        // Materialize this once, before any save advances updated_at. Older
        // snapshots have no finer evidence than their original snapshot time.
        let legacy_seen = if snapshot.updated_at > 0 {
            snapshot.updated_at
        } else {
            snapshot.codex.last_seen.unwrap_or(0)
        };
        for session in &mut snapshot.sessions {
            if session.last_seen.is_none() {
                session.last_seen = Some(legacy_seen);
                session.last_seen_estimated = true;
            }
        }
        snapshot.sessions.retain(|session| {
            !snapshot.dismissed.iter().any(|dismissed| {
                dismissed.source == session.source
                    && dismissed.id == session.id
                    && session.last_seen.unwrap_or(0) <= dismissed.last_seen
            })
        });
        let dirty = previous != snapshot;
        Self {
            snapshot,
            live: false,
            dirty,
            last_save: None,
            confirmed_live: HashMap::new(),
            keepalive_activity: HashMap::new(),
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

    pub fn sessions(&self, source: &str, limit: usize, now: u64) -> Vec<ui::SessionRow> {
        self.snapshot
            .sessions
            .iter()
            .filter(|row| row.source == source)
            .take(limit)
            .map(|session| session.row(now, self.confirmed(session)))
            .collect()
    }

    pub fn tasks(&self, source: &str, now: u64) -> AgentTasks {
        let mut tasks = AgentTasks::default();
        for session in self
            .snapshot
            .sessions
            .iter()
            .filter(|row| row.source == source)
        {
            match session.phase(now, self.confirmed(session)) {
                "running" => tasks.running += 1,
                "waitingForApproval" | "waitingForAnswer" => tasks.pending += 1,
                phase if task_status::is_finished(phase) => tasks.done += 1,
                _ => (),
            }
        }
        tasks
    }

    pub fn outcomes(&self, source: &str, now: u64) -> task_status::TaskOutcomes {
        task_status::TaskOutcomes::from_phases(
            self.snapshot
                .sessions
                .iter()
                .filter(|row| row.source == source)
                .map(|row| row.phase(now, self.confirmed(row))),
        )
    }

    fn confirmed(&self, session: &SavedSession) -> bool {
        self.confirmed_live
            .get(&session.key())
            .copied()
            .unwrap_or(false)
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

    /// Current activity updates agent liveness. Session retention separately
    /// uses each row's own event time, including on startup and timer ticks.
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

    pub fn mark_keepalive(&mut self, source: &str, id: &str, last_seen: u64, now: u64) {
        self.keepalive_activity
            .entry((source.into(), id.into()))
            .and_modify(|times| times.1 = now)
            .or_insert((last_seen, now));
    }

    pub fn mark_activity(&mut self, source: &str, id: &str) {
        self.keepalive_activity.remove(&(source.into(), id.into()));
        let previous_count = self.snapshot.dismissed.len();
        self.snapshot
            .dismissed
            .retain(|dismissed| dismissed.source != source || dismissed.id != id);
        self.dirty |= previous_count != self.snapshot.dismissed.len();
    }

    pub fn end_session(&mut self, source: &str, id: &str, now: u64) {
        self.snapshot
            .sessions
            .retain(|session| session.source != source || session.id != id);
        self.snapshot.dismissed.push(DismissedSession {
            source: source.into(),
            id: id.into(),
            last_seen: now,
        });
        self.keepalive_activity.remove(&(source.into(), id.into()));
        self.snapshot.updated_at = now;
        self.dirty = true;
    }

    /// Merge current records without deleting history merely because the core
    /// lifecycle table swept it after fifteen minutes. Notification trackers
    /// continue observing that original table, never this retained display.
    pub fn remember(&mut self, sessions: Vec<SessionObservation>, now: u64) {
        self.confirmed_live.clear();
        let mut changed = false;
        for observation in sessions {
            let row = observation.row;
            let key = (row.source.to_string(), row.id.to_string());
            let last_seen = match self.keepalive_activity.get(&key).copied() {
                Some((real_seen, synthetic_seen))
                    if observation.waiting && observation.last_seen == synthetic_seen =>
                {
                    real_seen
                }
                _ => {
                    self.keepalive_activity.remove(&key);
                    observation.last_seen
                }
            };
            if let Some(index) = self
                .snapshot
                .dismissed
                .iter()
                .position(|dismissed| dismissed.source == key.0 && dismissed.id == key.1)
            {
                if last_seen <= self.snapshot.dismissed[index].last_seen
                    && !observation.confirmed_live
                {
                    continue;
                }
                self.snapshot.dismissed.remove(index);
                changed = true;
            }
            self.confirmed_live
                .insert(key.clone(), observation.confirmed_live);
            let incoming = SavedSession {
                source: key.0,
                id: key.1,
                title: row.title.to_string(),
                detail: row.detail.to_string(),
                phase: row.phase.to_string(),
                last_seen: Some(last_seen),
                last_seen_estimated: false,
            };
            if let Some(existing) = self
                .snapshot
                .sessions
                .iter_mut()
                .find(|session| session.source == incoming.source && session.id == incoming.id)
            {
                if (existing.last_seen_estimated || last_seen >= existing.last_seen.unwrap_or(0))
                    && *existing != incoming
                {
                    *existing = incoming;
                    changed = true;
                }
            } else {
                self.snapshot.sessions.push(incoming);
                changed = true;
            }
        }
        if changed {
            self.snapshot.updated_at = now;
            self.dirty = true;
        }
    }

    pub fn inactive_count(&self, sources: &[&str], now: u64) -> usize {
        self.snapshot
            .sessions
            .iter()
            .filter(|session| {
                sources.contains(&session.source.as_str())
                    && session.inactive(now, self.confirmed(session))
            })
            .count()
    }

    pub fn expire(&mut self, now: u64, retention_secs: u64) -> usize {
        self.dismiss_where(now, |session| {
            now.saturating_sub(session.last_seen.unwrap_or(0)) >= retention_secs
        })
    }

    pub fn clear_inactive(&mut self, sources: &[&str], now: u64) -> usize {
        self.dismiss_where(now, |session| sources.contains(&session.source.as_str()))
    }

    fn dismiss_where(&mut self, now: u64, eligible: impl Fn(&SavedSession) -> bool) -> usize {
        let mut removed = Vec::new();
        self.snapshot.sessions.retain(|session| {
            let confirmed = self
                .confirmed_live
                .get(&session.key())
                .copied()
                .unwrap_or(false);
            if eligible(session) && session.inactive(now, confirmed) {
                removed.push(DismissedSession {
                    source: session.source.clone(),
                    id: session.id.clone(),
                    last_seen: session.last_seen.unwrap_or(0),
                });
                false
            } else {
                true
            }
        });
        let count = removed.len();
        for dismissal in removed {
            self.keepalive_activity
                .remove(&(dismissal.source.clone(), dismissal.id.clone()));
            if let Some(previous) =
                self.snapshot.dismissed.iter_mut().find(|previous| {
                    previous.source == dismissal.source && previous.id == dismissal.id
                })
            {
                previous.last_seen = previous.last_seen.max(dismissal.last_seen);
            } else {
                self.snapshot.dismissed.push(dismissal);
            }
        }
        if count > 0 {
            self.snapshot.updated_at = now;
            self.dirty = true;
        }
        count
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

fn supported_account(source: &str) -> bool {
    matches!(source, "codex" | "dodex")
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
                last_seen: Some(THEN),
                last_seen_estimated: false,
            }],
            updated_at: THEN,
            ..Default::default()
        }
    }
    #[test]
    fn startup_and_new_activity_preserve_retained_history() {
        let snapshot = previous_display();
        let mut display = DisplayState::restore(snapshot.clone());
        display.observe(CODEX, THEN + 10);
        display.remember(vec![], THEN + 20);
        assert!(!display.is_live());
        assert!(display.visible(CODEX));
        assert_eq!(display.snapshot.sessions, snapshot.sessions);
        assert_eq!(display.tasks("codex", THEN + 20).running, 1);
        assert!(!display.sessions("codex", 1, THEN + 20)[0].jumpable);
        display.activate(CODEX, THEN + 30, THEN + 30);
        display.remember(vec![], THEN + 30);
        assert!(display.is_live());
        assert_eq!(display.sessions("codex", 1, THEN + 30).len(), 1);
    }
    #[test]
    fn obsolete_saved_sources_are_removed_without_restoring_quota() {
        let mut value = serde_json::to_value(previous_display()).unwrap();
        value["sessions"][0]["source"] = serde_json::json!("unsupported");
        value["usage"] = serde_json::json!({"codex":{"primary":{"used_percent":25}}});
        let snapshot: Snapshot = serde_json::from_value(value).unwrap();
        let display = DisplayState::restore(snapshot);
        assert!(display.sessions("codex", 1, THEN).is_empty());
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
            last_seen: Some(THEN),
            last_seen_estimated: false,
        })
        .collect();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("display.json");
        write_snapshot(&path, &snapshot).unwrap();
        let display = DisplayState::restore(read_snapshot(&path).unwrap());
        assert_eq!(display.tasks("codex", THEN).done, 3);
        assert_eq!(
            display.outcomes("codex", THEN),
            task_status::TaskOutcomes {
                failed: 1,
                stopped: 1
            }
        );
        let rows = display.sessions("codex", 10, THEN);
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

    fn observation(id: &str, source: &str, phase: Phase, at: u64) -> SessionObservation {
        let mut state = SessionState::new(id, CODEX, at);
        state.phase = phase;
        observation_from(&state, source)
    }

    fn observation_from(state: &SessionState, source: &str) -> SessionObservation {
        SessionObservation::new(
            ui::SessionRow {
                id: state.session_id.clone().into(),
                source: source.into(),
                title: "project".into(),
                detail: "Working".into(),
                phase: task_status::phase(state).into(),
                jumpable: true,
            },
            state,
        )
    }

    #[test]
    fn five_hour_retention_uses_each_sessions_activity_and_exact_boundary() {
        const RETENTION: u64 = 5 * 60 * 60;
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(
            vec![
                observation("older", "codex", Phase::Running, THEN),
                observation("newer", "codex", Phase::Completed, THEN + 60),
            ],
            THEN,
        );
        // Core liveness can expire without taking the display history with it.
        display.remember(Vec::new(), THEN + STALE_AFTER_SECS);
        let rows = display.sessions("codex", 10, THEN + STALE_AFTER_SECS);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].phase, "stopped");
        assert_eq!(rows[0].detail, "Inactive");
        assert_eq!(display.tasks("codex", THEN + STALE_AFTER_SECS).running, 0);
        assert_eq!(display.tasks("codex", THEN + STALE_AFTER_SECS).done, 2);
        assert_eq!(
            display.outcomes("codex", THEN + STALE_AFTER_SECS).stopped,
            1
        );
        assert_eq!(display.expire(THEN + RETENTION - 1, RETENTION), 0);
        assert_eq!(display.expire(THEN + RETENTION, RETENTION), 1);
        assert_eq!(
            display.sessions("codex", 10, THEN + RETENTION)[0].id,
            "newer"
        );
        assert_eq!(display.expire(THEN + RETENTION + 60, RETENTION), 1);
        assert_eq!(display.tasks("codex", THEN + RETENTION + 60).total(), 0);
    }

    #[test]
    fn active_writers_real_waits_and_fresh_unconfirmed_work_are_protected() {
        let now = THEN + 8 * 60 * 60;
        let mut writer = SessionState::new("writer", CODEX, THEN);
        writer.observed_alive = true;
        let mut waiting = SessionState::new("wait", CODEX, THEN);
        waiting.phase = Phase::WaitingForAnswer;
        waiting
            .observed_questions
            .push(agent_companion_core::state::ObservedQuestion {
                key: "call".into(),
                prompt: None,
                requested_at: THEN,
            });
        let mut approval = SessionState::new("approval", CODEX, THEN);
        approval.phase = Phase::WaitingForApproval;
        approval
            .pending
            .push(agent_companion_core::state::PendingApproval {
                key: "approval".into(),
                event: "PermissionRequest".into(),
                tool_name: None,
                tool_input: None,
                requested_at: THEN,
            });
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(
            vec![
                observation_from(&writer, "codex"),
                observation_from(&waiting, "codex"),
                observation_from(&approval, "codex"),
                observation("fresh", "codex", Phase::Running, now - STALE_AFTER_SECS + 1),
                observation("old-running", "codex", Phase::Running, THEN),
                observation("done", "codex", Phase::Completed, now),
            ],
            now,
        );
        assert_eq!(display.expire(now, 5 * 60 * 60), 1);
        assert_eq!(display.inactive_count(&["codex"], now), 1);
        assert_eq!(display.clear_inactive(&["codex"], now), 1);
        assert_eq!(
            display.tasks("codex", now),
            AgentTasks {
                running: 2,
                pending: 2,
                done: 0
            }
        );
        assert_eq!(
            display.clear_inactive(&["codex"], now + 1),
            1,
            "fresh work becomes eligible at fifteen minutes"
        );
    }

    #[test]
    fn restored_wait_labels_cannot_claim_a_live_wait_indefinitely() {
        let mut snapshot = previous_display();
        snapshot.sessions[0].phase = "waitingForAnswer".into();
        let mut display = DisplayState::restore(snapshot);
        assert_eq!(display.tasks("codex", THEN).pending, 1);
        assert_eq!(display.tasks("codex", THEN + STALE_AFTER_SECS).pending, 0);
        assert_eq!(
            display.sessions("codex", 10, THEN + STALE_AFTER_SECS)[0].phase,
            "stopped"
        );
        assert_eq!(
            display.clear_inactive(&["codex"], THEN + STALE_AFTER_SECS),
            1
        );
    }

    #[test]
    fn cleanup_survives_repeated_scans_and_restart_then_new_activity_revives() {
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(
            vec![observation("same", "codex", Phase::Completed, THEN)],
            THEN,
        );
        assert_eq!(display.clear_inactive(&["codex"], THEN), 1);
        for _ in 0..2 {
            display.remember(
                vec![observation("same", "codex", Phase::Completed, THEN)],
                THEN + 1,
            );
            assert!(display.sessions("codex", 10, THEN + 1).is_empty());
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("display.json");
        write_snapshot(&path, &display.snapshot).unwrap();
        let mut display = DisplayState::restore(read_snapshot(&path).unwrap());
        display.remember(
            vec![observation("same", "codex", Phase::Completed, THEN)],
            THEN + 2,
        );
        assert!(display.sessions("codex", 10, THEN + 2).is_empty());
        display.remember(
            vec![observation("same", "codex", Phase::Running, THEN + 3)],
            THEN + 3,
        );
        assert_eq!(display.tasks("codex", THEN + 3).running, 1);
        assert_eq!(display.inactive_count(&["codex"], THEN + 3), 0);
        assert!(display.snapshot.dismissed.is_empty());
        // A genuine hook is stronger than second-resolution timestamps.
        display.remember(
            vec![observation("same", "codex", Phase::Completed, THEN + 3)],
            THEN + 3,
        );
        display.clear_inactive(&["codex"], THEN + 3);
        display.mark_activity("codex", "same");
        display.remember(
            vec![observation("same", "codex", Phase::Running, THEN + 3)],
            THEN + 3,
        );
        assert_eq!(display.tasks("codex", THEN + 3).running, 1);
    }

    #[test]
    fn a_real_writer_or_wait_can_recover_a_dismissed_unchanged_turn() {
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(
            vec![observation("resume", "codex", Phase::Running, THEN)],
            THEN,
        );
        display.clear_inactive(&["codex"], THEN + STALE_AFTER_SECS);
        let mut active = SessionState::new("resume", CODEX, THEN);
        active.observed_alive = true;
        display.remember(
            vec![observation_from(&active, "codex")],
            THEN + 10 * 60 * 60,
        );
        assert_eq!(display.tasks("codex", THEN + 10 * 60 * 60).running, 1);
    }

    #[test]
    fn account_scoped_cleanup_cannot_hide_the_other_accounts_identical_id() {
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(
            vec![
                observation("same", "codex", Phase::Completed, THEN),
                observation("same", "dodex", Phase::Completed, THEN),
            ],
            THEN,
        );
        assert_eq!(display.clear_inactive(&["dodex"], THEN), 1);
        assert_eq!(display.tasks("codex", THEN).done, 1);
        assert_eq!(display.tasks("dodex", THEN).done, 0);
        let mut display = DisplayState::restore(display.snapshot);
        display.remember(
            vec![observation("same", "dodex", Phase::Completed, THEN)],
            THEN + 1,
        );
        assert_eq!(display.tasks("codex", THEN + 1).done, 1);
        assert_eq!(display.tasks("dodex", THEN + 1).done, 0);
    }

    #[test]
    fn old_snapshot_timestamps_are_materialized_once_and_repaints_never_extend_them() {
        let mut value = serde_json::to_value(previous_display()).unwrap();
        value["sessions"][0]
            .as_object_mut()
            .unwrap()
            .remove("lastSeen");
        let mut display = DisplayState::restore(serde_json::from_value(value).unwrap());
        assert!(display.dirty);
        assert_eq!(display.snapshot.sessions[0].last_seen, Some(THEN));
        display.activate(CODEX, THEN + 4 * 60 * 60, THEN + 4 * 60 * 60);
        display.remember(Vec::new(), THEN + 4 * 60 * 60);
        let mut restarted = DisplayState::restore(
            serde_json::from_str(&serde_json::to_string(&display.snapshot).unwrap()).unwrap(),
        );
        assert_eq!(restarted.snapshot.sessions[0].last_seen, Some(THEN));
        assert_eq!(restarted.expire(THEN + 5 * 60 * 60, 5 * 60 * 60), 1);
    }

    #[test]
    fn authentic_observations_replace_estimated_legacy_times_even_after_restart() {
        let mut snapshot = previous_display();
        snapshot.sessions[0].last_seen = None;
        let display = DisplayState::restore(snapshot);
        assert!(display.snapshot.sessions[0].last_seen_estimated);
        let serialized = serde_json::to_string(&display.snapshot).unwrap();
        let mut restarted = DisplayState::restore(serde_json::from_str(&serialized).unwrap());
        restarted.remember(
            vec![observation("saved", "codex", Phase::Completed, THEN - 10)],
            THEN,
        );
        assert_eq!(restarted.tasks("codex", THEN).running, 0);
        assert_eq!(restarted.tasks("codex", THEN).done, 1);
        assert_eq!(restarted.snapshot.sessions[0].last_seen, Some(THEN - 10));
        assert!(!restarted.snapshot.sessions[0].last_seen_estimated);
        // Authentic timestamps retain the usual out-of-order protection.
        restarted.remember(
            vec![observation("saved", "codex", Phase::Running, THEN - 11)],
            THEN + 1,
        );
        assert_eq!(restarted.tasks("codex", THEN + 1).done, 1);
        assert_eq!(restarted.snapshot.sessions[0].last_seen, Some(THEN - 10));
    }

    #[test]
    fn waiting_pipe_keepalives_do_not_refresh_real_activity_and_resolution_releases_the_clock() {
        let mut state = SessionState::new("question", CODEX, THEN);
        state.phase = Phase::WaitingForAnswer;
        let mut display = DisplayState::restore(Snapshot::default());
        display.remember(vec![observation_from(&state, "codex")], THEN);
        let later = THEN + 3 * 60 * 60;
        display.mark_keepalive("codex", "question", state.last_seen, later);
        state.last_seen = later;
        display.remember(vec![observation_from(&state, "codex")], later);
        assert_eq!(display.snapshot.sessions[0].last_seen, Some(THEN));
        state.phase = Phase::Running;
        display.remember(vec![observation_from(&state, "codex")], later);
        assert_eq!(display.snapshot.sessions[0].last_seen, Some(later));
        assert!(display.keepalive_activity.is_empty());
        state.last_seen = later + 5;
        display.remember(vec![observation_from(&state, "codex")], later + 5);
        assert_eq!(display.snapshot.sessions[0].last_seen, Some(later + 5));
    }
}
