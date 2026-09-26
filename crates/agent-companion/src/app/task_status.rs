//! Presentation outcomes are separate from the lifecycle's terminal phase.
use agent_companion_core::protocol::HookSource;
use agent_companion_core::state::{Phase, STALE_AFTER_SECS, SessionState, SessionTable};

/// Unsuccessful or unclassified display outcomes cannot establish that every
/// task completed successfully. Lifecycle counters remain unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskOutcomes {
    pub failed: usize,
    pub stopped: usize,
}

impl TaskOutcomes {
    pub fn from_phases<'a>(phases: impl IntoIterator<Item = &'a str>) -> Self {
        let mut outcomes = Self::default();
        for phase in phases {
            match phase {
                "failed" => outcomes.failed += 1,
                "stopped" => outcomes.stopped += 1,
                "running" | "waitingForApproval" | "waitingForAnswer" | "completed" => (),
                _ => outcomes.stopped += 1,
            }
        }
        outcomes
    }
}

pub fn phase(state: &SessionState) -> &'static str {
    match (state.phase, state.last_event.as_str()) {
        (Phase::Completed, "turn_failed") => "failed",
        (Phase::Completed, "turn_aborted" | "session_disconnected" | "Interrupt") => "stopped",
        _ => state.phase.as_str(),
    }
}

pub fn is_finished(phase: &str) -> bool {
    matches!(phase, "completed" | "failed" | "stopped")
}

/// Both app-owned tables use the default stale window. Match the existing
/// task-count filter without changing core lifecycle or notification behavior.
pub fn outcomes(table: &SessionTable, source: HookSource, now: u64) -> TaskOutcomes {
    TaskOutcomes::from_phases(
        table
            .sessions()
            .filter(|state| state.source == source && !state.is_stale(now, STALE_AFTER_SECS))
            .map(phase),
    )
}

/// Old display snapshots stored only "completed" even when their existing
/// detail text identified an unsuccessful outcome. Upgrade only those exact
/// built-in descriptions; arbitrary user text is not an outcome signal.
pub fn saved_phase<'a>(phase: &'a str, detail: &str) -> &'a str {
    if phase != "completed" {
        return phase;
    }
    let detail = detail
        .strip_prefix("Codex · ")
        .or_else(|| detail.strip_prefix("Dodex · "))
        .unwrap_or(detail);
    match detail {
        "Failed · Open in Codex" => "failed",
        "Interrupted" | "Disconnected · Open in Codex" => "stopped",
        _ => phase,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_companion_core::protocol::HookPayload;

    #[test]
    fn terminal_outcomes_do_not_change_lifecycle_or_active_work() {
        for (event, expected) in [
            ("Stop", "completed"),
            ("task_complete", "completed"),
            ("turn_failed", "failed"),
            ("turn_aborted", "stopped"),
            ("session_disconnected", "stopped"),
            ("Interrupt", "stopped"),
        ] {
            let mut state = SessionState::new("fixture", HookSource::Codex, 100);
            state.phase = Phase::Completed;
            state.last_event = event.into();
            assert_eq!(phase(&state), expected);
            assert!(is_finished(phase(&state)));
            assert_eq!(state.phase, Phase::Completed);
            state.phase = Phase::Running;
            assert_eq!(phase(&state), "running");
        }
    }

    #[test]
    fn primary_and_secondary_outcomes_remain_isolated_and_ignore_stale_tasks() {
        let mut primary = SessionTable::new();
        let mut secondary = SessionTable::new();
        let seed = |table: &mut SessionTable, id: &str, event: &str, source, at| {
            let payload: HookPayload = serde_json::from_value(serde_json::json!({
                "session_id": id, "hook_event_name": "Stop"
            }))
            .unwrap();
            table.apply(&payload, source, at);
            table.get_mut(id).unwrap().last_event = event.into();
        };
        seed(
            &mut primary,
            "same-id",
            "task_complete",
            HookSource::Codex,
            100,
        );
        seed(
            &mut primary,
            "other-agent",
            "turn_failed",
            HookSource::Claude,
            100,
        );
        seed(
            &mut secondary,
            "same-id",
            "turn_failed",
            HookSource::Codex,
            100,
        );
        seed(
            &mut secondary,
            "interrupted",
            "Interrupt",
            HookSource::Codex,
            100,
        );
        seed(&mut secondary, "stale", "turn_failed", HookSource::Codex, 0);
        let now = STALE_AFTER_SECS;
        assert_eq!(
            outcomes(&primary, HookSource::Codex, now),
            TaskOutcomes::default()
        );
        assert_eq!(
            outcomes(&secondary, HookSource::Codex, now),
            TaskOutcomes {
                failed: 1,
                stopped: 1
            }
        );
        // The terminal counter still includes every ended task.
        assert_eq!(secondary.tasks(HookSource::Codex, now).done, 2);
    }

    #[test]
    fn old_snapshot_outcomes_upgrade_only_known_terminal_descriptions() {
        assert_eq!(saved_phase("completed", "Failed · Open in Codex"), "failed");
        assert_eq!(saved_phase("completed", "Codex · Interrupted"), "stopped");
        assert_eq!(
            saved_phase("completed", "Dodex · Disconnected · Open in Codex"),
            "stopped"
        );
        assert_eq!(saved_phase("completed", "Done"), "completed");
        assert_eq!(saved_phase("running", "Failed · Open in Codex"), "running");
        assert_eq!(
            saved_phase("completed", "Failed user command in a successful turn"),
            "completed"
        );
        assert_eq!(
            TaskOutcomes::from_phases(["completed", "unknown"]),
            TaskOutcomes {
                failed: 0,
                stopped: 1
            }
        );
    }
}
