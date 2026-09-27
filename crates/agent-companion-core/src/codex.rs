//! Observe Codex sessions from their local rollout events, independently of
//! hooks. No agent settings are changed and no approval replies are fabricated.
//!
//! Windows can leave directory metadata unchanged while a rollout is open, so
//! the cache queries each file's current length too. Activity comes from events;
//! writer locks can preserve an already known active turn. Rescanning or copying
//! an old rollout never makes it active by itself.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::protocol::HookSource;
use crate::state::{CodexActivity, CodexClient, Phase, STALE_AFTER_SECS, SessionState};
use crate::usage::parse_iso8601;

const READ_BUDGET: u64 = 4 * 1024 * 1024;
const FILES_PER_SCAN: usize = 32;

#[cfg(feature = "desktop-history")]
mod desktop;

#[derive(Default)]
struct Events {
    session: Option<SessionState>,
    turn: Option<String>,
    phase: Option<Phase>,
    last_seen: u64,
    activity_at: u128,
    started_at: Option<u128>,
    excluded: bool,
    questions: HashSet<String>,
}

impl Events {
    fn push(&mut self, line: &[u8]) -> bool {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        let payload = &record["payload"];
        let timestamp = record["timestamp"].as_str().and_then(parse_iso8601);
        let activity_at = timestamp.map(|seconds| {
            let fraction = record["timestamp"]
                .as_str()
                .unwrap_or_default()
                .split_once('.')
                .map(|(_, suffix)| {
                    suffix
                        .bytes()
                        .take_while(u8::is_ascii_digit)
                        .take(9)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let nanos = fraction
                .iter()
                .fold(0u128, |total, digit| total * 10 + u128::from(digit - b'0'))
                * 10u128.pow(9 - fraction.len() as u32);
            u128::from(seconds) * 1_000_000_000 + nanos
        });
        match record["type"].as_str() {
            Some("response_item") => {
                let Some(at) = timestamp else { return true };
                if activity_at.unwrap() < self.activity_at || self.phase == Some(Phase::Completed) {
                    return true;
                }
                let call = payload["call_id"].as_str();
                let name = payload["name"].as_str();
                if payload["type"].as_str() == Some("function_call")
                    && matches!(
                        name,
                        Some("request_user_input" | "functions.request_user_input")
                    )
                    && let Some(call) = call
                {
                    self.questions.insert(call.to_string());
                    self.phase = Some(Phase::WaitingForAnswer);
                    self.last_seen = self.last_seen.max(at);
                    self.activity_at = activity_at.unwrap();
                    if let Some(session) = &mut self.session {
                        session.last_event = "request_user_input".into();
                    }
                } else if payload["type"].as_str() == Some("function_call_output")
                    && let Some(call) = call
                    && self.questions.remove(call)
                {
                    if self.questions.is_empty() && self.phase == Some(Phase::WaitingForAnswer) {
                        self.phase = Some(Phase::Running);
                    }
                    self.last_seen = self.last_seen.max(at);
                    self.activity_at = activity_at.unwrap();
                    if let Some(session) = &mut self.session {
                        session.last_event = "user_input_answered".into();
                    }
                }
            }
            Some("session_meta") => {
                let id = payload["id"]
                    .as_str()
                    .or_else(|| payload["session_id"].as_str())
                    .filter(|id| !id.is_empty());
                // Forked rollouts can replay a parent's metadata after their
                // own header. That must not rename the child to the parent or
                // turn an excluded subagent into a second visible parent task.
                if let (Some(session), Some(id)) = (&self.session, id)
                    && session.session_id != id
                {
                    return true;
                }
                self.excluded |= payload["source"].get("subagent").is_some()
                    || payload["source"].as_str() == Some("subagent")
                    || payload["thread_source"].as_str() == Some("subagent");
                if let Some(id) = id {
                    let mut session =
                        SessionState::new(id, HookSource::Codex, timestamp.unwrap_or(0));
                    session.cwd = payload["cwd"].as_str().map(str::to_string);
                    session.codex_client = CodexClient::from_metadata(
                        payload["source"].as_str(),
                        payload["originator"].as_str(),
                    );
                    self.session = Some(session);
                }
            }
            Some("turn_context") => {
                if let Some(cwd) = payload["cwd"].as_str()
                    && let Some(session) = &mut self.session
                {
                    session.cwd = Some(cwd.to_string());
                }
            }
            Some("event_msg") => {
                let Some(timestamp) = timestamp else {
                    return true;
                };
                if activity_at.unwrap() < self.activity_at {
                    return true;
                }
                let kind = payload["type"].as_str().unwrap_or_default();
                let turn = payload["turn_id"].as_str();
                if kind != "task_started"
                    && let (Some(current), Some(incoming)) = (self.turn.as_deref(), turn)
                    && current != incoming
                {
                    return true;
                }
                // Replayed starts for a finished turn cannot undo its terminal
                // event. A genuinely new turn is allowed even in the same second.
                if self.phase == Some(Phase::Completed)
                    && !(matches!(kind, "task_started" | "user_message")
                        && ((turn.is_some() && turn != self.turn.as_deref())
                            || (turn.is_none() && activity_at.unwrap() > self.activity_at)))
                {
                    return true;
                }
                match kind {
                    "task_started" | "user_message" => {
                        self.questions.clear();
                        self.turn = turn.map(str::to_string);
                        self.phase = Some(Phase::Running);
                        self.started_at = activity_at;
                        if kind == "user_message"
                            && let Some(message) = payload["message"].as_str()
                            && let Some(session) = &mut self.session
                        {
                            // A useful title for CLI rollouts without desktop metadata.
                            // Bound the preview before allocating a normalized string.
                            let preview: String = message.chars().take(240).collect();
                            let title = preview.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !title.is_empty() {
                                session.display_name = Some(title);
                            }
                        }
                    }
                    "task_complete" | "turn_aborted" => {
                        if self.turn.is_none() {
                            self.turn = turn.map(str::to_string);
                        }
                        self.questions.clear();
                        self.phase = Some(Phase::Completed);
                    }
                    // Activity is useful when attaching to an older rollout or
                    // when a very long turn's start is outside the initial tail.
                    "agent_reasoning" | "agent_message" | "item_started" | "item_completed" => {
                        if self.phase.is_none() {
                            self.phase = Some(Phase::Running);
                            self.turn = turn.map(str::to_string);
                        }
                        if payload["phase"].as_str() == Some("final")
                            || payload["item"]["phase"].as_str() == Some("final")
                        {
                            self.phase = Some(Phase::Completed);
                        }
                    }
                    // Token accounting after a completed turn must not make it
                    // look busy again or keep an old session alive forever.
                    _ => return true,
                }
                self.last_seen = self.last_seen.max(timestamp);
                self.activity_at = activity_at.unwrap();
                if let Some(session) = &mut self.session {
                    session.last_event = kind.to_string();
                }
            }
            _ => {}
        }
        true
    }

    fn snapshot(&self, path: &Path) -> Option<SessionState> {
        if self.excluded || self.last_seen == 0 {
            return None;
        }
        let mut session = self.session.clone()?;
        session.phase = self.phase?;
        session.last_seen = self.last_seen;
        session.codex_activity = Some(CodexActivity {
            turn: self.turn.clone(),
            at: self.activity_at,
            started_at: self.started_at,
        });
        session.transcript_path = Some(path.to_string_lossy().into_owned());
        Some(session)
    }
}

#[derive(Default)]
struct Cursor {
    events: Events,
    offset: u64,
    length: u64,
    modified: Option<SystemTime>,
    skipping_line: bool,
}

impl Cursor {
    fn read(&mut self, path: &Path, length: u64, modified: Option<SystemTime>) -> io::Result<()> {
        self.read_from(&mut File::open(path)?, length, modified)
    }

    fn read_from(
        &mut self,
        file: &mut (impl Read + Seek),
        length: u64,
        modified: Option<SystemTime>,
    ) -> io::Result<()> {
        if length < self.length
            || length < self.offset
            || (length == self.length && modified != self.modified)
        {
            *self = Self::default();
        }
        let mut budget = READ_BUDGET;
        if self.offset == 0 && length > READ_BUDGET {
            // Metadata is the first record. For a large existing session, start
            // with its recent events instead of replaying hours of tool output.
            // Spend the remaining budget on the tail: a tool result can easily
            // exceed 512 KiB. Include BufReader's prefetch in the read allowance.
            let mut reader = BufReader::new((&mut *file).take(budget));
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line)?;
            self.events.push(&line);
            budget = reader.get_ref().limit();
            self.offset = length - budget;
            self.skipping_line = true;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file.take(budget.min(length - self.offset)));
        let mut line = Vec::new();
        let start = self.offset;
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            let terminated = line.ends_with(b"\n");
            if self.skipping_line {
                self.offset += read as u64;
                self.skipping_line = !terminated;
                continue;
            }
            if !terminated && !self.events.push(&line) {
                // Retry a half-written JSON record on the next scan. A record
                // larger than the read budget is skipped until its newline.
                if self.offset + read as u64 - start >= budget {
                    self.offset += read as u64;
                    self.skipping_line = true;
                }
                break;
            }
            if terminated {
                self.events.push(&line);
            }
            self.offset += read as u64;
        }
        self.length = length;
        self.modified = modified;
        Ok(())
    }
}

/// Cursors into append-only rollouts. Only the worker thread owns this cache.
#[derive(Default)]
pub struct SessionCache {
    files: HashMap<PathBuf, Cursor>,
    stopped: HashMap<String, Stopped>,
    source: Option<(PathBuf, PathBuf)>,
    #[cfg(feature = "desktop-history")]
    desktop: desktop::Cache,
}

impl SessionCache {
    /// `codex_home` is CODEX_HOME, or `<user home>/.codex`. Old files are also
    /// checked for growth: a resumed conversation can live in an old directory.
    pub fn scan(&mut self, codex_home: &Path, now: u64) -> io::Result<Vec<SessionState>> {
        self.scan_with_database_home(codex_home, codex_home, now)
    }

    /// Databases may live outside CODEX_HOME; rollouts and writer locks do not.
    pub fn scan_with_database_home(
        &mut self,
        codex_home: &Path,
        database_home: &Path,
        now: u64,
    ) -> io::Result<Vec<SessionState>> {
        let source = (codex_home.to_owned(), database_home.to_owned());
        if self.source.as_ref() != Some(&source) {
            self.files.clear();
            self.stopped.clear();
            self.source = Some(source);
        }
        let mut files = Vec::new();
        collect(&codex_home.join("sessions"), &mut files)?;
        let seen: HashSet<_> = files.iter().map(|(path, _, _)| path.clone()).collect();
        files.sort_by_key(|(path, length, modified)| {
            let changed = self
                .files
                .get(path)
                .is_some_and(|old| old.length != *length || old.modified != *modified);
            (std::cmp::Reverse(changed), std::cmp::Reverse(*modified))
        });
        let mut opened = 0;
        for (path, length, modified) in files {
            let cached = self.files.entry(path.clone()).or_default();
            if cached.offset == length && cached.length == length && cached.modified == modified {
                continue;
            }
            if opened >= FILES_PER_SCAN {
                continue;
            }
            opened += 1;
            // A transient sharing error keeps the last good state until its
            // event timestamp ages out, and is retried on the next scan.
            let _ = cached.read(&path, length, modified);
        }
        self.files.retain(|path, _| seen.contains(path));
        let mut sessions: Vec<_> = self
            .files
            .iter()
            .filter_map(|(path, cursor)| cursor.events.snapshot(path))
            .collect();
        // Multiple physical rollouts can describe one conversation. Resolve
        // them before merging desktop state so stale copies cannot survive the
        // history update, inflate counts or share a SwiftUI row identity.
        sessions.sort_unstable_by(|left, right| {
            left.session_id
                .cmp(&right.session_id)
                .then_with(|| activity_order(right, left))
                .then_with(|| {
                    rollout_phase_priority(right.phase).cmp(&rollout_phase_priority(left.phase))
                })
                .then_with(|| left.transcript_path.cmp(&right.transcript_path))
        });
        sessions.dedup_by(|left, right| left.session_id == right.session_id);
        #[cfg(feature = "desktop-history")]
        let sessions = {
            let mut sessions = sessions;
            self.desktop
                .merge_with_database_home(codex_home, database_home, now, &mut sessions);
            sessions
        };
        // Keep stopped evidence while its rollout exists or history has not
        // explicitly archived it. Neither a DB outage nor its limited query
        // window proves deletion of an unchanged inProgress row.
        #[allow(unused_mut)]
        let mut retained_ids: HashSet<_> = self
            .files
            .values()
            .filter_map(|cursor| {
                cursor
                    .events
                    .session
                    .as_ref()
                    .map(|session| session.session_id.clone())
            })
            .collect();
        #[cfg(feature = "desktop-history")]
        retained_ids.extend(self.desktop.retained_ids().map(str::to_owned));
        self.stopped.retain(|id, _| retained_ids.contains(id));
        Ok(sessions
            .into_iter()
            .filter(|session| session.last_seen <= now)
            .map(|mut session| {
                self.observe_liveness(codex_home, &mut session);
                session
            })
            .filter(|session| !session.is_stale(now, STALE_AFTER_SECS))
            .collect())
    }

    fn observe_liveness(&mut self, home: &Path, session: &mut SessionState) {
        let Some(activity) = session.codex_activity.clone() else {
            return;
        };
        let explicit_stop =
            session.phase == Phase::Completed && session.last_event != "session_disconnected";
        if let Some(stopped) = self.stopped.get(&session.session_id) {
            let new_turn = activity.turn.is_some()
                && stopped.activity.turn.is_some()
                && activity.turn != stopped.activity.turn
                && activity.at >= stopped.activity.at;
            // Legacy events may omit turn IDs. A strictly later explicit start
            // is still evidence, including when later activity arrived in the
            // same batch. Ordinary activity cannot reopen an explicit finish.
            let legacy_start = (activity.turn.is_none() || stopped.activity.turn.is_none())
                && activity
                    .started_at
                    .is_some_and(|at| at > stopped.activity.at);
            let new_activity = !stopped.explicit && activity.at > stopped.activity.at;
            if !(new_turn
                || legacy_start
                || new_activity
                || (explicit_stop && activity.at >= stopped.activity.at))
            {
                session.phase = Phase::Completed;
                session.observed_alive = false;
                session.last_event = stopped.event.clone();
                session.last_seen = stopped.last_seen;
                session.codex_activity = Some(stopped.activity.clone());
                return;
            }
            self.stopped.remove(&session.session_id);
        }
        session.observed_alive = false;
        if session.phase != Phase::Completed {
            match writer_alive(home, &session.session_id) {
                Some(true) => session.observed_alive = true,
                Some(false) => {
                    session.phase = Phase::Completed;
                    session.last_event = "session_disconnected".into();
                }
                None => {} // Unknown liveness keeps the event-age policy.
            }
        }
        if session.phase == Phase::Completed {
            self.stopped.insert(
                session.session_id.clone(),
                Stopped {
                    activity,
                    event: session.last_event.clone(),
                    last_seen: session.last_seen,
                    explicit: session.last_event != "session_disconnected",
                },
            );
        }
    }
}

struct Stopped {
    activity: CodexActivity,
    event: String,
    last_seen: u64,
    explicit: bool,
}

fn valid_thread_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn writer_alive(home: &Path, id: &str) -> Option<bool> {
    if !valid_thread_id(id) {
        return None;
    }
    let locks = home.join("thread-writer-locks");
    // Writers take this coordination lock exclusively while acquiring/releasing
    // their thread lock. Defer instead of causing a one-shot writer acquisition
    // to fail against our brief shared probe. Never create either lock file.
    let coordination = File::open(locks.join(".coordination.lock")).ok()?;
    coordination.try_lock_shared().ok()?;
    // Declared after coordination: released first on every return path.
    let file = File::open(locks.join(format!("{id}.lock"))).ok()?;
    match file.try_lock_shared() {
        Ok(()) => Some(false), // Dropping the handle immediately releases our probe.
        Err(TryLockError::WouldBlock) => Some(true),
        Err(TryLockError::Error(_)) => None,
    }
}

fn activity_order(left: &SessionState, right: &SessionState) -> std::cmp::Ordering {
    match (&left.codex_activity, &right.codex_activity) {
        (Some(left), Some(right)) => left.at.cmp(&right.at),
        _ => left.last_seen.cmp(&right.last_seen),
    }
}

#[cfg(feature = "desktop-history")]
fn same_turn(left: &SessionState, right: &SessionState) -> bool {
    match (&left.codex_activity, &right.codex_activity) {
        (Some(left), Some(right)) if left.turn.is_some() && right.turn.is_some() => {
            left.turn == right.turn
        }
        _ => true,
    }
}

fn rollout_phase_priority(phase: Phase) -> u8 {
    // Timestamps have second precision. At a tie prefer an explicit stop or
    // pending question to an older running observation, then a stable path.
    match phase {
        Phase::Completed => 2,
        Phase::WaitingForApproval | Phase::WaitingForAnswer => 1,
        Phase::Running => 0,
    }
}

type RolloutFile = (PathBuf, u64, Option<SystemTime>);

fn collect(dir: &Path, files: &mut Vec<RolloutFile>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten() {
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), files)?;
        } else if kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            // DirEntry metadata can contain a stale size on Windows while
            // Codex holds the writer open. Query the file itself each time.
            let metadata = fs::metadata(entry.path())?;
            files.push((entry.path(), metadata.len(), metadata.modified().ok()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
