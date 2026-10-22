//! Notification policy is independent of the Windows delivery mechanism.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use agent_companion_core::protocol::events;
use agent_companion_core::state::{
    ASK_USER_QUESTION, CodexClient, PENDING_TTL_SECS, Phase, STALE_AFTER_SECS, SessionState,
    SessionTable,
};

mod desktop;
pub use desktop::Notifier;

const MIN_RUN_SECS: u64 = 30;

#[derive(Clone, Debug)]
pub struct Completion {
    pub session_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attention {
    pub session_id: String,
    pub request_key: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitingUpdate {
    Show(Attention),
    Clear {
        session_id: String,
        request_key: String,
    },
}

struct WaitingRequest {
    prompt: Option<String>,
    question: bool,
    requested_at: u64,
    expires: bool,
}

#[derive(Default)]
struct WaitingSeen {
    requests: BTreeSet<String>,
    shown: BTreeSet<String>,
}

#[derive(Default)]
pub struct WaitingTracker {
    seen: HashMap<String, WaitingSeen>,
    started_at: Option<u64>,
}

impl WaitingTracker {
    /// Consume every snapshot, including disabled and watched requests. The
    /// first snapshot establishes a baseline; the cutoff also excludes old
    /// requests discovered by a later asynchronous rollout scan.
    pub fn observe(
        &mut self,
        table: &SessionTable,
        now: u64,
        enabled: bool,
        mut watched: impl FnMut(&SessionState) -> bool,
    ) -> Vec<WaitingUpdate> {
        let initial = self.started_at.is_none();
        let started_at = *self.started_at.get_or_insert(now);
        let mut updates = Vec::new();
        self.seen.retain(|session_id, seen| {
            if table.get(session_id).is_some() {
                return true;
            }
            for request_key in &seen.shown {
                updates.push(WaitingUpdate::Clear {
                    session_id: session_id.clone(),
                    request_key: request_key.clone(),
                });
            }
            false
        });
        for state in table.sessions() {
            let all_requests = waiting_requests(state);
            let known_requests = all_requests.keys().cloned().collect();
            let requests = if state.is_stale(now, STALE_AFTER_SECS) {
                BTreeMap::new()
            } else {
                all_requests
                    .into_iter()
                    .filter(|(_, request)| {
                        !request.expires
                            || now.saturating_sub(request.requested_at) < PENDING_TTL_SECS
                    })
                    .collect()
            };
            let seen = self.seen.entry(state.session_id.clone()).or_default();
            let eligible = enabled && terminal_session(state);
            // Foreground checks may query UI Automation; only ask when there
            // is something new to show or an existing toast to withdraw.
            let changed = requests.keys().any(|key| !seen.requests.contains(key));
            let watched = eligible && (changed || !seen.shown.is_empty()) && watched(state);
            let show = eligible && !watched;
            seen.shown.retain(|key| {
                if show && requests.contains_key(key) {
                    return true;
                }
                updates.push(WaitingUpdate::Clear {
                    session_id: state.session_id.clone(),
                    request_key: key.clone(),
                });
                false
            });
            for (key, request) in &requests {
                if initial
                    || !show
                    || seen.requests.contains(key)
                    || request.requested_at < started_at
                    || request.requested_at > now
                {
                    continue;
                }
                let project = state
                    .cwd
                    .as_deref()
                    .map(crate::util::project_name)
                    .unwrap_or_else(|| "Session".into());
                let body = if request.question {
                    request
                        .prompt
                        .as_deref()
                        .map(|prompt| format!("Answer needed: {prompt}"))
                        .unwrap_or_else(|| {
                            "A question needs your answer. Click to return to the session.".into()
                        })
                } else {
                    "Approval needed. Click to return to the session.".into()
                };
                updates.push(WaitingUpdate::Show(Attention {
                    session_id: state.session_id.clone(),
                    request_key: key.clone(),
                    title: format!("Codex needs input · {}", concise(&project, 60)),
                    body,
                }));
                seen.shown.insert(key.clone());
            }
            // Timeouts and refreshed transport timestamps do not create new
            // request IDs. Consume them until the source actually removes them.
            seen.requests = known_requests;
        }
        updates
    }
}

/// A question's human-facing text is safe for matching its terminal pane. Tool
/// commands, option values, and questions marked secret are never inspected.
pub fn waiting_hint(state: &SessionState, request_key: Option<&str>) -> Option<String> {
    waiting_requests(state)
        .into_iter()
        .filter(|(key, _)| request_key.is_none_or(|wanted| key == wanted))
        .find_map(|(_, request)| request.prompt)
}

pub fn waiting_request_is_pending(state: &SessionState, request_key: &str) -> bool {
    waiting_requests(state).contains_key(request_key)
}

fn waiting_requests(state: &SessionState) -> BTreeMap<String, WaitingRequest> {
    if !state.phase.is_waiting() {
        return BTreeMap::new();
    }
    let mut requests = BTreeMap::new();
    for request in &state.observed_questions {
        requests.insert(
            format!("question:{}", request.key),
            WaitingRequest {
                prompt: request
                    .prompt
                    .as_deref()
                    .map(|prompt| concise(prompt, 180))
                    .filter(|prompt| !prompt.is_empty()),
                question: true,
                requested_at: request.requested_at,
                expires: false,
            },
        );
    }
    for pending in &state.pending {
        let question = pending.event == events::CODEX_USER_INPUT
            || pending.tool_name.as_deref() == Some(ASK_USER_QUESTION);
        // Native app-server hook IDs identify the transport request. Its itemId
        // identifies the rollout call, allowing both sources to share a toast.
        let key = question
            .then(|| {
                pending
                    .tool_input
                    .as_ref()?
                    .get("itemId")?
                    .as_str()
                    .filter(|key| !key.is_empty())
            })
            .flatten()
            .unwrap_or(&pending.key);
        let prompt = question
            .then(|| question_prompt(pending.tool_input.as_ref()?))
            .flatten();
        let key = format!("{}:{key}", if question { "question" } else { "approval" });
        let expires = !requests.contains_key(&key);
        requests.insert(
            key,
            WaitingRequest {
                prompt,
                question,
                requested_at: pending.requested_at,
                // An outstanding rollout observation remains valid even if
                // the transient hook reply channel has timed out.
                expires,
            },
        );
    }
    requests
}

fn question_prompt(input: &serde_json::Value) -> Option<String> {
    input
        .get("questions")?
        .as_array()?
        .iter()
        .find_map(|question| {
            if ["isSecret", "is_secret"].iter().any(|flag| {
                question
                    .get(*flag)
                    .is_some_and(|value| value.as_bool() != Some(false))
            }) {
                return None;
            }
            let prompt = concise(question.get("question")?.as_str()?, 180);
            (!prompt.is_empty()).then_some(prompt)
        })
}

fn concise(text: &str, limit: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|ch| !ch.is_control() || ch.is_whitespace())
        .collect();
    crate::util::truncate(&crate::util::one_line(&clean), limit)
}

fn terminal_session(state: &SessionState) -> bool {
    if state.codex_client == CodexClient::Desktop {
        return false;
    }
    let terminal = state.terminal.as_ref();
    if terminal.is_some_and(|terminal| {
        terminal
            .ancestors
            .iter()
            .any(|process| process.exe.eq_ignore_ascii_case("chatgpt.exe"))
    }) {
        return false;
    }
    state.codex_client == CodexClient::Cli
        || terminal.is_some_and(|terminal| {
            super::win::target::from_meta(terminal).is_some()
                || terminal.ancestors.iter().any(|process| {
                    matches!(
                        process.exe.to_ascii_lowercase().as_str(),
                        "windowsterminal.exe"
                            | "windowsterminalpreview.exe"
                            | "openconsole.exe"
                            | "conhost.exe"
                            | "wezterm-gui.exe"
                            | "alacritty.exe"
                            | "mintty.exe"
                            | "conemu.exe"
                            | "conemu64.exe"
                    )
                })
                || terminal
                    .env
                    .get("WT_SESSION")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
}

struct Seen {
    phase: Phase,
    started: u64,
    last_event: String,
    last_seen: u64,
}

#[derive(Default)]
pub struct Tracker {
    seen: HashMap<String, Seen>,
}

impl Tracker {
    /// Observe even when disabled or watched: turning notifications on must
    /// never replay old completions. A first snapshot only establishes a baseline.
    pub fn observe(
        &mut self,
        table: &SessionTable,
        now: u64,
        enabled: bool,
        mut watched: impl FnMut(&SessionState) -> bool,
    ) -> Vec<Completion> {
        let mut notices = Vec::new();
        self.seen.retain(|id, _| table.get(id).is_some());
        for state in table.sessions() {
            let old = self.seen.get(&state.session_id);
            let new_turn = old.is_some_and(|old| {
                state.phase != Phase::Completed
                    && (old.phase == Phase::Completed
                        || (state.last_seen > old.last_seen
                            && matches!(
                                state.last_event.as_str(),
                                "SessionStart"
                                    | "UserPromptSubmit"
                                    | "task_started"
                                    | "user_message"
                            )))
            });
            let started = if new_turn {
                now
            } else {
                old.map(|old| old.started).unwrap_or(now)
            };
            let just_finished = state.phase == Phase::Completed
                && old.is_some_and(|old| old.phase != Phase::Completed);
            let cancelled = matches!(
                state.last_event.as_str(),
                "Interrupt"
                    | "turn_aborted"
                    | "SessionEnd"
                    | "turn_failed"
                    | "session_disconnected"
            );
            if just_finished
                && !cancelled
                && enabled
                && now.saturating_sub(started) >= MIN_RUN_SECS
                && !watched(state)
            {
                let project = state
                    .cwd
                    .as_deref()
                    .map(crate::util::project_name)
                    .unwrap_or_else(|| "Session".into());
                notices.push(Completion {
                    session_id: state.session_id.clone(),
                    title: format!(
                        "{} · {}",
                        "Codex",
                        crate::util::truncate(&crate::util::one_line(&project), 60)
                    ),
                    body: "Task finished. Click to view the session.".into(),
                });
            }
            // Ignore unchanged snapshots without allocating a new event string.
            if old.is_none_or(|old| {
                old.phase != state.phase
                    || old.last_seen != state.last_seen
                    || old.last_event != state.last_event
            }) {
                self.seen.insert(
                    state.session_id.clone(),
                    Seen {
                        phase: state.phase,
                        started,
                        last_event: state.last_event.clone(),
                        last_seen: state.last_seen,
                    },
                );
            }
        }
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_companion_core::protocol::{HookPayload, HookSource, ProcessRef, TerminalMeta};
    use agent_companion_core::state::ObservedQuestion;
    use serde_json::{Value, json};
    fn event(table: &mut SessionTable, name: &str, at: u64) {
        let payload: HookPayload = serde_json::from_value(serde_json::json!({"session_id":"test", "hook_event_name":name,"cwd":"C:/projects/demo"})).unwrap();
        table.apply(&payload, HookSource::Codex, at);
    }
    #[test]
    fn only_long_unwatched_completions_notify_once() {
        let mut table = SessionTable::new();
        let mut tracker = Tracker::default();
        event(&mut table, "Stop", 1);
        assert!(tracker.observe(&table, 1, true, |_| false).is_empty());
        event(&mut table, "UserPromptSubmit", 10);
        tracker.observe(&table, 10, true, |_| false);
        event(&mut table, "Stop", 50);
        assert_eq!(tracker.observe(&table, 50, true, |_| false).len(), 1);
        assert!(tracker.observe(&table, 51, true, |_| false).is_empty());
        for (start, enabled, watched, end) in [
            (60, true, false, "Interrupt"),
            (110, true, true, "Stop"),
            (160, false, false, "Stop"),
        ] {
            event(&mut table, "UserPromptSubmit", start);
            tracker.observe(&table, start, enabled, |_| watched);
            event(&mut table, end, start + 40);
            assert!(
                tracker
                    .observe(&table, start + 40, enabled, |_| watched)
                    .is_empty()
            );
            assert!(
                tracker
                    .observe(&table, start + 41, true, |_| false)
                    .is_empty()
            );
        }
        event(&mut table, "UserPromptSubmit", 210);
        tracker.observe(&table, 210, true, |_| false);
        event(&mut table, "Stop", 211);
        assert!(tracker.observe(&table, 211, true, |_| false).is_empty());
    }
    #[test]
    fn new_turn_while_running_restarts_the_duration() {
        let mut table = SessionTable::new();
        let mut tracker = Tracker::default();
        event(&mut table, "UserPromptSubmit", 1);
        tracker.observe(&table, 1, true, |_| false);
        event(&mut table, "UserPromptSubmit", 50);
        tracker.observe(&table, 50, true, |_| false);
        event(&mut table, "Stop", 51);
        assert!(tracker.observe(&table, 51, true, |_| false).is_empty());
    }

    #[test]
    fn desktop_failure_and_disconnect_do_not_send_success_notifications() {
        for end in ["turn_failed", "session_disconnected"] {
            let mut table = SessionTable::new();
            let mut tracker = Tracker::default();
            event(&mut table, "UserPromptSubmit", 1);
            tracker.observe(&table, 1, true, |_| false);
            event(&mut table, "Stop", 40);
            table.get_mut("test").unwrap().last_event = end.into();
            assert!(tracker.observe(&table, 40, true, |_| false).is_empty());
        }
    }

    fn request(
        table: &mut SessionTable,
        session: &str,
        key: &str,
        at: u64,
        question: bool,
        input: Value,
    ) {
        let payload = serde_json::from_value(json!({
            "session_id":session,
            "hook_event_name":if question { events::CODEX_USER_INPUT } else { events::PERMISSION_REQUEST },
            "cwd":"C:/projects/demo",
            "tool_use_id":key,
            "tool_name":if question { ASK_USER_QUESTION } else { "Bash" },
            "tool_input":input,
        })).unwrap();
        table.apply(&payload, HookSource::Codex, at);
        table.get_mut(session).unwrap().codex_client = CodexClient::Cli;
    }

    fn question(table: &mut SessionTable, session: &str, key: &str, at: u64) {
        request(
            table,
            session,
            key,
            at,
            true,
            json!({"questions":[{"question":"Which option?"}]}),
        );
    }

    fn shown(updates: &[WaitingUpdate]) -> Vec<&Attention> {
        updates
            .iter()
            .filter_map(|update| match update {
                WaitingUpdate::Show(notice) => Some(notice),
                WaitingUpdate::Clear { .. } => None,
            })
            .collect()
    }

    #[test]
    fn waiting_baselines_existing_requests_and_notifies_new_sessions_immediately() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        question(&mut table, "existing", "first", 1);
        assert!(tracker.observe(&table, 1, true, |_| false).is_empty());
        assert!(tracker.observe(&table, 2, true, |_| false).is_empty());

        question(&mut table, "new-session", "first", 2);
        let updates = tracker.observe(&table, 2, true, |_| false);
        let notices = shown(&updates);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].session_id, "new-session");
        assert_eq!(notices[0].request_key, "question:first");
        assert_eq!(notices[0].title, "Codex needs input · demo");
        assert_eq!(notices[0].body, "Answer needed: Which option?");
        assert!(tracker.observe(&table, 3, true, |_| false).is_empty());

        // A second question can arrive without leaving the waiting phase.
        question(&mut table, "existing", "second", 3);
        let updates = tracker.observe(&table, 3, true, |_| false);
        assert_eq!(shown(&updates).len(), 1);
        assert_eq!(shown(&updates)[0].request_key, "question:second");
    }

    #[test]
    fn delayed_startup_rollouts_do_not_replay_old_questions() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        assert!(tracker.observe(&table, 100, true, |_| false).is_empty());
        question(&mut table, "old-session", "old-question", 90);
        assert!(tracker.observe(&table, 101, true, |_| false).is_empty());
        assert!(tracker.observe(&table, 102, true, |_| false).is_empty());

        // Same-second live requests are allowed despite coarse event clocks.
        question(&mut table, "live-session", "new-question", 100);
        let updates = tracker.observe(&table, 102, true, |_| false);
        assert_eq!(shown(&updates).len(), 1);
        assert_eq!(shown(&updates)[0].session_id, "live-session");
    }

    #[test]
    fn disabled_and_watched_waits_are_consumed_without_later_replay() {
        for (enabled, watched) in [(false, false), (true, true)] {
            let mut table = SessionTable::new();
            let mut tracker = WaitingTracker::default();
            tracker.observe(&table, 1, enabled, |_| watched);
            question(&mut table, "s", "first", 2);
            assert!(tracker.observe(&table, 2, enabled, |_| watched).is_empty());
            assert!(tracker.observe(&table, 3, true, |_| false).is_empty());
            question(&mut table, "s", "second", 4);
            assert_eq!(shown(&tracker.observe(&table, 4, true, |_| false)).len(), 1);
        }
    }

    #[test]
    fn retries_dedupe_but_concurrent_requests_notify_and_clear_independently() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        tracker.observe(&table, 1, true, |_| false);
        question(&mut table, "s", "first", 2);
        assert_eq!(shown(&tracker.observe(&table, 2, true, |_| false)).len(), 1);
        question(&mut table, "s", "first", 3);
        assert!(tracker.observe(&table, 3, true, |_| false).is_empty());
        question(&mut table, "s", "second", 3);
        let updates = tracker.observe(&table, 3, true, |_| false);
        assert_eq!(shown(&updates).len(), 1);
        assert_eq!(shown(&updates)[0].request_key, "question:second");

        table.get_mut("s").unwrap().resolve("first");
        assert_eq!(
            tracker.observe(&table, 4, true, |_| false),
            vec![WaitingUpdate::Clear {
                session_id: "s".into(),
                request_key: "question:first".into(),
            }]
        );
        assert!(waiting_request_is_pending(
            table.get("s").unwrap(),
            "question:second"
        ));
        assert!(!waiting_request_is_pending(
            table.get("s").unwrap(),
            "question:first"
        ));
        table.get_mut("s").unwrap().resolve("second");
        assert_eq!(
            tracker.observe(&table, 5, true, |_| false),
            vec![WaitingUpdate::Clear {
                session_id: "s".into(),
                request_key: "question:second".into(),
            }]
        );
    }

    #[test]
    fn clear_waiting_toasts_when_watched_disabled_expired_or_removed() {
        for reason in ["watched", "disabled", "expired", "removed", "completed"] {
            let mut table = SessionTable::new();
            let mut tracker = WaitingTracker::default();
            tracker.observe(&table, 1, true, |_| false);
            question(&mut table, "s", "first", 2);
            assert_eq!(shown(&tracker.observe(&table, 2, true, |_| false)).len(), 1);
            if matches!(reason, "removed" | "completed") {
                let payload = serde_json::from_value(json!({
                    "session_id":"s",
                    "hook_event_name":if reason == "removed" { events::SESSION_END } else { events::STOP },
                })).unwrap();
                table.apply(&payload, HookSource::Codex, 3);
            }
            let now = if reason == "expired" {
                2 + PENDING_TTL_SECS
            } else {
                3
            };
            let updates =
                tracker.observe(&table, now, reason != "disabled", |_| reason == "watched");
            assert_eq!(
                updates,
                vec![WaitingUpdate::Clear {
                    session_id: "s".into(),
                    request_key: "question:first".into(),
                }],
                "{reason}"
            );
            assert!(tracker.observe(&table, now + 1, true, |_| false).is_empty());
        }
    }

    #[test]
    fn refreshing_an_expired_transport_does_not_replay_its_request() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        tracker.observe(&table, 1, true, |_| false);
        question(&mut table, "s", "first", 2);
        assert_eq!(shown(&tracker.observe(&table, 2, true, |_| false)).len(), 1);
        assert!(matches!(
            tracker
                .observe(&table, 2 + PENDING_TTL_SECS, true, |_| false)
                .as_slice(),
            [WaitingUpdate::Clear { .. }]
        ));
        question(&mut table, "s", "first", 3 + PENDING_TTL_SECS);
        assert!(
            tracker
                .observe(&table, 3 + PENDING_TTL_SECS, true, |_| false)
                .is_empty()
        );
    }

    #[test]
    fn native_and_rollout_question_identity_share_one_toast() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        tracker.observe(&table, 1, true, |_| false);
        request(
            &mut table,
            "s",
            "transport-id",
            2,
            true,
            json!({"itemId":"call-id","questions":[{"question":"Native prompt?"}]}),
        );
        let state = table.get_mut("s").unwrap();
        state.observed_questions.push(ObservedQuestion {
            key: "call-id".into(),
            prompt: Some("Observed prompt?".into()),
            requested_at: 2,
        });
        let updates = tracker.observe(&table, 2, true, |_| false);
        let notices = shown(&updates);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].request_key, "question:call-id");
        assert_eq!(notices[0].body, "Answer needed: Native prompt?");
        let state = table.get("s").unwrap();
        assert_eq!(
            waiting_hint(state, Some("question:call-id")).as_deref(),
            Some("Native prompt?")
        );
        assert!(waiting_hint(state, Some("question:missing")).is_none());

        // A hook arriving after an observed request must not replay it either.
        table.get_mut("s").unwrap().pending.clear();
        assert!(tracker.observe(&table, 3, true, |_| false).is_empty());
    }

    #[test]
    fn secret_questions_and_permission_commands_never_appear_in_notifications() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        tracker.observe(&table, 1, true, |_| false);
        request(
            &mut table,
            "s",
            "secret",
            2,
            true,
            json!({"questions":[{"question":"secret-password","isSecret":true}]}),
        );
        request(
            &mut table,
            "s",
            "command",
            2,
            false,
            json!({"command":"TOKEN=secret-token upload", "questions":[{"question":"secret-command"}]}),
        );
        let updates = tracker.observe(&table, 2, true, |_| false);
        assert_eq!(shown(&updates).len(), 2);
        for notice in shown(&updates) {
            assert!(!notice.body.contains("secret"));
            assert!(!notice.body.contains("TOKEN"));
            assert!(!notice.body.contains("upload"));
        }
        assert!(waiting_hint(table.get("s").unwrap(), None).is_none());
    }

    #[test]
    fn question_hints_select_public_text_and_bound_unicode_length() {
        let mut table = SessionTable::new();
        request(
            &mut table,
            "s",
            "q",
            1,
            true,
            json!({"questions":[
                {"question":"do not show","is_secret":true},
                {"question":format!("  Use\n\t<fast> & safe?\u{0} {}", "选".repeat(300)), "options":[{"label":"private choice"}]}
            ]}),
        );
        let hint = waiting_hint(table.get("s").unwrap(), None).unwrap();
        assert!(hint.starts_with("Use <fast> & safe? "));
        assert_eq!(hint.chars().count(), 180);
        assert!(hint.ends_with('…'));
        assert!(!hint.contains("private choice"));
        assert!(!hint.chars().any(char::is_control));
    }

    #[test]
    fn malformed_secret_markers_cannot_expose_hook_question_text() {
        for flag in ["isSecret", "is_secret"] {
            for marker in [
                json!(true),
                json!("false"),
                Value::Null,
                json!(0),
                json!({}),
            ] {
                let mut question = json!({"question":"private prompt"});
                question[flag] = marker;
                assert!(question_prompt(&json!({"questions":[question]})).is_none());
            }
            let mut question = json!({"question":"public prompt"});
            question[flag] = json!(false);
            assert_eq!(
                question_prompt(&json!({"questions":[question]})).as_deref(),
                Some("public prompt")
            );
        }
    }

    #[test]
    fn waiting_notifications_require_reliable_tui_evidence() {
        for (client, executables, expected) in [
            (CodexClient::Unknown, vec![], false),
            (CodexClient::Unknown, vec!["cmd.exe", "pwsh.exe"], false),
            (CodexClient::Cli, vec![], true),
            (CodexClient::Unknown, vec!["windowsterminal.exe"], true),
            (CodexClient::Desktop, vec!["windowsterminal.exe"], false),
            (CodexClient::Cli, vec!["chatgpt.exe"], false),
            (
                CodexClient::Unknown,
                vec!["windowsterminal.exe", "chatgpt.exe"],
                false,
            ),
        ] {
            let mut table = SessionTable::new();
            let mut tracker = WaitingTracker::default();
            tracker.observe(&table, 1, true, |_| false);
            question(&mut table, "s", "first", 2);
            let state = table.get_mut("s").unwrap();
            state.codex_client = client;
            state.terminal = Some(TerminalMeta {
                ancestors: executables
                    .iter()
                    .map(|exe| ProcessRef {
                        pid: 42,
                        exe: (*exe).into(),
                    })
                    .collect(),
                ..Default::default()
            });
            let updates = tracker.observe(&table, 2, true, |_| false);
            assert_eq!(
                !shown(&updates).is_empty(),
                expected,
                "{client:?} {executables:?}"
            );
            if !expected {
                table.get_mut("s").unwrap().codex_client = CodexClient::Cli;
                table.get_mut("s").unwrap().terminal = None;
                assert!(tracker.observe(&table, 3, true, |_| false).is_empty());
            }
        }
    }

    #[test]
    fn mere_waiting_phase_does_not_invent_a_question() {
        let mut table = SessionTable::new();
        let mut tracker = WaitingTracker::default();
        tracker.observe(&table, 1, true, |_| false);
        event(&mut table, events::USER_PROMPT_SUBMIT, 2);
        let state = table.get_mut("test").unwrap();
        state.codex_client = CodexClient::Cli;
        state.phase = Phase::WaitingForAnswer;
        assert!(tracker.observe(&table, 2, true, |_| false).is_empty());
    }

    #[test]
    fn terminal_environment_is_evidence_but_an_empty_capture_is_not() {
        let mut state = SessionState::new("s", HookSource::Codex, 1);
        state.terminal = Some(TerminalMeta::default());
        assert!(!terminal_session(&state));
        state
            .terminal
            .as_mut()
            .unwrap()
            .env
            .insert("WT_SESSION".into(), "  ".into());
        assert!(!terminal_session(&state));
        state
            .terminal
            .as_mut()
            .unwrap()
            .env
            .insert("WT_SESSION".into(), "terminal-session".into());
        assert!(terminal_session(&state));
        state.codex_client = CodexClient::Desktop;
        assert!(!terminal_session(&state));
    }
}
