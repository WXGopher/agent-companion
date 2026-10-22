use super::*;
use crate::codex::SessionCache;
use crate::state::ObservedQuestion;

const ID: &str = "01a08965-575f-7870-b001-3698447df18c";
const NOW: u64 = 1_789_025_000;

#[test]
fn in_progress_history_preserves_rollout_question_metadata_until_completion() {
    let fixture = Fixture::new();
    fixture
        .history
        .execute(
            "INSERT INTO thread_items VALUES (?1, 'turn-1', ?2)",
            (ID, (NOW * 1000) as i64),
        )
        .unwrap();
    let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 10);
    rollout.phase = Phase::WaitingForAnswer;
    rollout.last_seen = NOW - 1;
    rollout.transcript_path = Some("C:/synthetic/rollout.jsonl".into());
    rollout.codex_activity = Some(CodexActivity {
        turn: Some("turn-1".into()),
        at: u128::from(NOW - 1) * 1_000_000_000,
        started_at: Some(u128::from(NOW - 10) * 1_000_000_000),
    });
    rollout.codex_rollout_offset = Some(240);
    rollout.codex_rollout_path = rollout.transcript_path.clone();
    rollout.observed_questions = vec![ObservedQuestion {
        key: "call".into(),
        prompt: Some("Choose?".into()),
        requested_at: NOW - 1,
    }];
    let mut sessions = vec![rollout.clone()];
    Cache::default().merge(fixture.dir.path(), NOW, &mut sessions);
    assert_eq!(sessions[0].phase, Phase::WaitingForAnswer);
    assert_eq!(sessions[0].observed_questions, rollout.observed_questions);
    assert_eq!(
        sessions[0].codex_rollout_offset,
        rollout.codex_rollout_offset
    );
    assert_eq!(sessions[0].codex_rollout_path, rollout.codex_rollout_path);
    fixture
        .history
        .execute(
            "UPDATE thread_turns SET status='completed', completed_at=?1",
            [NOW as i64],
        )
        .unwrap();
    Cache::default().merge(fixture.dir.path(), NOW + 1, &mut sessions);
    assert_eq!(sessions[0].phase, Phase::Completed);
    assert!(sessions[0].observed_questions.is_empty());
}

#[test]
fn history_merge_keeps_the_cli_rollout_needed_to_find_its_terminal() {
    let fixture = Fixture::new();
    fixture
        .state
        .execute("UPDATE threads SET source = 'cli'", [])
        .unwrap();
    for last_seen in [NOW - 11, NOW] {
        let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 20);
        rollout.last_seen = last_seen;
        rollout.codex_client = CodexClient::Cli;
        rollout.transcript_path = Some("C:/synthetic/rollout-cli.jsonl".into());
        let mut sessions = vec![rollout];
        Cache::default().merge(fixture.dir.path(), NOW, &mut sessions);
        assert_eq!(sessions[0].codex_client, CodexClient::Cli);
        assert_eq!(
            sessions[0].transcript_path.as_deref(),
            Some("C:/synthetic/rollout-cli.jsonl")
        );
    }
}

#[test]
fn history_without_client_details_preserves_explicit_desktop_metadata() {
    let fixture = Fixture::new();
    let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 20);
    rollout.codex_client = CodexClient::Desktop;
    let mut sessions = vec![rollout];
    Cache::default().merge(fixture.dir.path(), NOW, &mut sessions);
    assert_eq!(sessions[0].codex_client, CodexClient::Desktop);
}

struct Fixture {
    dir: tempfile::TempDir,
    state: Connection,
    history: Connection,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = Connection::open(dir.path().join("state_5.sqlite")).unwrap();
        state
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             CREATE TABLE threads (id TEXT, cwd TEXT, name TEXT, title TEXT, archived INTEGER,
                                   source TEXT, history_mode TEXT, updated_at INTEGER);",
            )
            .unwrap();
        state.execute("INSERT INTO threads VALUES (?1, 'C:/project', 'Desktop work', 'old title', 0, 'vscode', 'paginated', ?2)", (ID, NOW as i64)).unwrap();
        let history = Connection::open(dir.path().join("thread_history_1.sqlite")).unwrap();
        history
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             CREATE TABLE thread_turns (thread_id TEXT, turn_id TEXT, rollout_ordinal INTEGER,
                 status TEXT, started_at INTEGER, completed_at INTEGER);
             CREATE TABLE thread_items (thread_id TEXT, turn_id TEXT, created_at_ms INTEGER);",
            )
            .unwrap();
        history
            .execute(
                "INSERT INTO thread_turns VALUES (?1, 'turn-1', 1, 'inProgress', ?2, NULL)",
                (ID, (NOW - 10) as i64),
            )
            .unwrap();
        Self {
            dir,
            state,
            history,
        }
    }

    fn scan(&self, now: u64) -> Vec<SessionState> {
        SessionCache::default().scan(self.dir.path(), now).unwrap()
    }

    fn rollout_path(&self, path: &Path) {
        self.state
            .execute("ALTER TABLE threads ADD COLUMN rollout_path TEXT", [])
            .unwrap();
        self.state
            .execute(
                "UPDATE threads SET rollout_path = ?1",
                [path.to_string_lossy().as_ref()],
            )
            .unwrap();
    }
}

fn rollout(home: &Path, id: &str) -> std::path::PathBuf {
    let directory = home.join("sessions");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("rollout-{id}.jsonl"));
    let metadata = serde_json::json!({"timestamp":"2026-09-05T00:00:00Z", "type":"session_meta",
        "payload":{"id":id, "source":"cli"}});
    let started = serde_json::json!({"timestamp":"2026-09-05T00:00:01Z", "type":"event_msg",
        "payload":{"type":"task_started", "turn_id":"turn-1"}});
    std::fs::write(&path, format!("{metadata}\n{started}\n")).unwrap();
    path
}

fn writer(home: &Path, id: &str) -> File {
    let locks = home.join("thread-writer-locks");
    std::fs::create_dir_all(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let file = File::create(locks.join(format!("{id}.lock"))).unwrap();
    file.lock().unwrap();
    file
}

#[test]
fn foreign_history_is_excluded_from_shared_and_independent_instance_databases() {
    const FOREIGN_ID: &str = "02a08965-575f-7870-b001-3698447df18c";
    let primary = Fixture::new();
    let secondary = Fixture::new();
    let path = rollout(primary.dir.path(), ID);
    primary.rollout_path(&path);
    secondary.rollout_path(&path);
    let other_path = primary.dir.path().join("sessions/rollout-other.jsonl");
    secondary
        .state
        .execute(
            "INSERT INTO threads VALUES (?1, '/synthetic', '', '', 0, 'cli', 'paginated', ?2, ?3)",
            (
                FOREIGN_ID,
                NOW as i64,
                other_path.to_string_lossy().as_ref(),
            ),
        )
        .unwrap();
    secondary
        .history
        .execute(
            "INSERT INTO thread_turns VALUES (?1, 'other-turn', 1, 'inProgress', ?2, NULL)",
            (FOREIGN_ID, (NOW - 8) as i64),
        )
        .unwrap();
    let _writer = writer(primary.dir.path(), ID);
    let locks = secondary.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    // Copied logs must not survive as a fallback after the history row proves
    // that they originated in the other instance.
    let copy = rollout(secondary.dir.path(), ID);
    assert_ne!(copy, path);
    let mut primary_table = crate::state::SessionTable::new();
    primary_table.sync_observed(HookSource::Codex, primary.scan(NOW), NOW);
    assert_eq!(primary_table.tasks(HookSource::Codex, NOW).running, 1);
    let mut cache = SessionCache::default();
    for database in [primary.dir.path(), secondary.dir.path()] {
        let sessions = cache
            .scan_with_database_home(secondary.dir.path(), database, NOW)
            .unwrap();
        assert!(
            sessions.is_empty(),
            "foreign history was attributed to the secondary instance"
        );
        let mut table = crate::state::SessionTable::new();
        table.sync_observed(HookSource::Codex, sessions, NOW);
        assert_eq!(table.tasks(HookSource::Codex, NOW).running, 0);
    }
    assert!(
        !locks.join(format!("{ID}.lock")).exists(),
        "read-only ownership checks created a thread lock"
    );
}

#[test]
fn a_local_writer_resuming_foreign_history_keeps_disconnect_tombstones() {
    let fixture = Fixture::new();
    fixture.rollout_path(&fixture.dir.path().join("sessions/old-rollout.jsonl"));
    let home = tempfile::tempdir().unwrap();
    let writer = writer(home.path(), ID);
    let mut cache = SessionCache::default();
    let active = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), NOW)
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].phase, Phase::Running);
    assert!(active[0].observed_alive);
    drop(writer);
    let stopped = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), NOW + 1)
        .unwrap();
    assert_eq!(stopped[0].phase, Phase::Completed);
    assert_eq!(stopped[0].last_event, "session_disconnected");
    let writer = File::open(home.path().join(format!("thread-writer-locks/{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    assert_eq!(
        cache
            .scan_with_database_home(home.path(), fixture.dir.path(), NOW + 2)
            .unwrap()[0]
            .phase,
        Phase::Completed
    );
    fixture
        .history
        .execute(
            "UPDATE thread_turns SET turn_id='new-turn', started_at=?1",
            [(NOW + 2) as i64],
        )
        .unwrap();
    assert_eq!(
        cache
            .scan_with_database_home(home.path(), fixture.dir.path(), NOW + 3)
            .unwrap()[0]
            .phase,
        Phase::Running
    );
}

#[test]
fn cached_foreign_history_does_not_hide_a_new_local_writer_during_database_failure() {
    let fixture = Fixture::new();
    fixture.rollout_path(&fixture.dir.path().join("sessions/old-rollout.jsonl"));
    let now = crate::usage::parse_iso8601("2026-09-05T00:00:02Z").unwrap();
    fixture
        .history
        .execute("UPDATE thread_turns SET started_at=?1", [(now - 1) as i64])
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut cache = SessionCache::default();
    assert!(
        cache
            .scan_with_database_home(home.path(), fixture.dir.path(), now)
            .unwrap()
            .is_empty()
    );
    fixture
        .history
        .execute("DROP TABLE thread_turns", [])
        .unwrap();
    let path = rollout(home.path(), ID);
    let _writer = writer(home.path(), ID);
    let sessions = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), now + 1)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, ID);
    assert_eq!(sessions[0].phase, Phase::Running);
    assert!(sessions[0].observed_alive);
    assert_eq!(sessions[0].transcript_path.as_deref(), path.to_str());
}

#[test]
fn missing_thread_locks_keep_the_legacy_fallback_for_owned_history() {
    let fixture = Fixture::new();
    fixture.rollout_path(&rollout(fixture.dir.path(), ID));
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let sessions = fixture.scan(NOW);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, Phase::Running);
    assert!(!sessions[0].observed_alive);
    assert!(!locks.join(format!("{ID}.lock")).exists());
}

#[test]
fn external_legacy_history_requires_a_local_rollout_or_writer() {
    let fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let mut cache = SessionCache::default();
    assert!(
        cache
            .scan_with_database_home(home.path(), fixture.dir.path(), NOW)
            .unwrap()
            .is_empty()
    );
    rollout(home.path(), ID);
    let sessions = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), NOW + 1)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, ID);
}

#[test]
fn rollout_ownership_resolves_missing_leaves_and_checks_home_boundaries() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("codex");
    std::fs::create_dir(&home).unwrap();
    assert!(path_within_home(
        &home.join("sessions/missing.jsonl"),
        &home
    ));
    assert!(!path_within_home(
        &fixture
            .path()
            .join("codex-secondary/sessions/missing.jsonl"),
        &home
    ));
    assert!(!path_within_home(
        &home.join("../codex-secondary/missing.jsonl"),
        &home
    ));
    #[cfg(windows)]
    assert!(path_within_home(
        &PathBuf::from(
            home.join("sessions/missing.jsonl")
                .to_string_lossy()
                .to_uppercase()
                .replace('\\', "/")
        ),
        &home
    ));
}

#[test]
fn desktop_completion_updates_the_single_session_from_duplicate_rollouts() {
    let fixture = Fixture::new();
    let started = crate::usage::parse_iso8601("2026-09-05T00:00:00Z").unwrap();
    fixture
        .history
        .execute(
            "UPDATE thread_turns SET status='completed', started_at=?1, completed_at=?2",
            ((started + 1) as i64, (started + 3) as i64),
        )
        .unwrap();
    let directory = fixture.dir.path().join("sessions");
    std::fs::create_dir(&directory).unwrap();
    let metadata = serde_json::json!({"timestamp":"2026-09-05T00:00:00Z", "type":"session_meta",
        "payload":{"id":ID, "source":"cli"}});
    let event = serde_json::json!({"timestamp":"2026-09-05T00:00:01Z", "type":"event_msg",
        "payload":{"type":"task_started", "turn_id":"turn-1"}});
    for name in ["rollout-original.jsonl", "rollout-copy.jsonl"] {
        std::fs::write(directory.join(name), format!("{metadata}\n{event}\n")).unwrap();
    }
    let sessions = fixture.scan(started + 4);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, Phase::Completed);
    assert_eq!(sessions[0].last_seen, started + 3);
}

#[test]
fn desktop_history_works_without_rollouts_and_reads_wal_updates() {
    let fixture = Fixture::new();
    let running = fixture.scan(NOW);
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].phase, Phase::Running);
    assert_eq!(running[0].display_name.as_deref(), Some("Desktop work"));
    fixture
        .history
        .execute(
            "UPDATE thread_turns SET status = 'completed', completed_at = ?1",
            [NOW as i64],
        )
        .unwrap();
    let done = fixture.scan(NOW + 1);
    assert_eq!(done[0].phase, Phase::Completed);
    assert_eq!(done[0].last_seen, NOW);
    fixture
        .history
        .execute(
            "INSERT INTO thread_turns VALUES (?1, 'turn-2', 2, 'inProgress', ?2, NULL)",
            (ID, (NOW + 2) as i64),
        )
        .unwrap();
    assert_eq!(fixture.scan(NOW + 3)[0].phase, Phase::Running);
    assert_eq!(fixture.scan(NOW + 3)[0].first_seen, NOW + 2);
}

#[test]
fn explicit_database_directory_keeps_locks_at_instance_home_and_clears_old_cache() {
    let fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let lock_dir = home.path().join("thread-writer-locks");
    std::fs::create_dir(&lock_dir).unwrap();
    File::create(lock_dir.join(".coordination.lock")).unwrap();
    let lock = File::create(lock_dir.join(format!("{ID}.lock"))).unwrap();
    lock.lock().unwrap();
    let mut cache = SessionCache::default();
    let sessions = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), NOW)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].observed_alive);
    drop(lock);
    let stopped = cache
        .scan_with_database_home(home.path(), fixture.dir.path(), NOW + 1)
        .unwrap();
    assert_eq!(stopped[0].last_event, "session_disconnected");
    let missing = home.path().join("other-database");
    assert!(
        cache
            .scan_with_database_home(home.path(), &missing, NOW + 2)
            .unwrap()
            .is_empty()
    );
    assert!(
        !missing.exists(),
        "A read-only scan created the missing database directory"
    );
}

#[test]
fn cancelled_and_failed_turns_are_distinct_and_archiving_removes_stale_rollouts() {
    let fixture = Fixture::new();
    for (status, event) in [("interrupted", "turn_aborted"), ("failed", "turn_failed")] {
        fixture
            .history
            .execute(
                "UPDATE thread_turns SET status = ?1, completed_at = ?2",
                (status, NOW as i64),
            )
            .unwrap();
        assert_eq!(fixture.scan(NOW + 1)[0].last_event, event);
    }
    let mut log_sessions = fixture.scan(NOW + 1);
    fixture
        .state
        .execute("UPDATE threads SET archived = 1", [])
        .unwrap();
    Cache::default().merge(fixture.dir.path(), NOW + 1, &mut log_sessions);
    assert!(log_sessions.is_empty());
    fixture
        .state
        .execute("UPDATE threads SET archived = 0, name = 'Renamed'", [])
        .unwrap();
    assert_eq!(
        fixture.scan(NOW + 1)[0].display_name.as_deref(),
        Some("Renamed")
    );
}

#[test]
fn a_live_writer_keeps_a_quiet_turn_and_a_closed_writer_cannot_keep_it_alive() {
    let fixture = Fixture::new();
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let writer = File::create(locks.join(format!("{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    let quiet = fixture.scan(NOW + STALE_AFTER_SECS);
    assert_eq!(quiet.len(), 1);
    assert!(quiet[0].observed_alive);
    assert_eq!(quiet[0].last_seen, NOW - 10);
    drop(writer);
    let disconnected = fixture.scan(NOW);
    assert_eq!(disconnected[0].last_event, "session_disconnected");
    assert!(!disconnected[0].observed_alive);
    assert!(fixture.scan(NOW + STALE_AFTER_SECS).is_empty());
}

#[test]
fn missing_or_incompatible_storage_is_read_only_and_preserves_the_log_fallback() {
    let absent = tempfile::tempdir().unwrap();
    assert!(read(absent.path(), NOW).is_err());
    assert!(std::fs::read_dir(absent.path()).unwrap().next().is_none());
    let fixture = Fixture::new();
    assert!(
        read_only(&fixture.dir.path().join("state_5.sqlite"))
            .unwrap()
            .execute("DELETE FROM threads", [])
            .is_err()
    );
    let mut from_logs = fixture.scan(NOW);
    let original = from_logs.clone();
    fixture
        .history
        .execute("DROP TABLE thread_turns", [])
        .unwrap();
    Cache::default().merge(fixture.dir.path(), NOW, &mut from_logs);
    assert_eq!(from_logs, original);
}

#[test]
fn subagents_unknown_status_and_path_ids_do_not_become_desktop_sessions() {
    let fixture = Fixture::new();
    fixture
        .state
        .execute("UPDATE threads SET source = '{\"subagent\":{}}'", [])
        .unwrap();
    assert!(fixture.scan(NOW).is_empty());
    fixture
        .state
        .execute(
            "UPDATE threads SET source = 'vscode', id = '../outside'",
            [],
        )
        .unwrap();
    assert!(fixture.scan(NOW).is_empty());
    fixture
        .state
        .execute("UPDATE threads SET id = ?1", [ID])
        .unwrap();
    fixture
        .history
        .execute("UPDATE thread_turns SET status = 'futureStatus'", [])
        .unwrap();
    assert!(fixture.scan(NOW).is_empty());
}

#[test]
fn a_temporary_database_failure_retains_state_for_a_bounded_interval() {
    let fixture = Fixture::new();
    let mut cache = SessionCache::default();
    let baseline = cache.scan(fixture.dir.path(), NOW).unwrap();
    fixture
        .history
        .execute("DROP TABLE thread_turns", [])
        .unwrap();
    assert_eq!(cache.scan(fixture.dir.path(), NOW + 2).unwrap(), baseline);
    assert!(cache.scan(fixture.dir.path(), NOW + 31).unwrap().is_empty());
}

#[test]
fn cached_history_unknown_writer_does_not_stop_a_newer_rollout() {
    let fixture = Fixture::new();
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let lock_path = locks.join(format!("{ID}.lock"));
    let writer = File::create(&lock_path).unwrap();
    writer.lock().unwrap();
    let mut cache = Cache::default();
    let mut sessions = vec![];
    cache.merge(fixture.dir.path(), NOW, &mut sessions);
    assert!(sessions[0].observed_alive);
    fixture
        .history
        .execute("DROP TABLE thread_turns", [])
        .unwrap();
    drop(writer);
    std::fs::remove_file(lock_path).unwrap();
    let mut rollout = SessionState::new(ID, HookSource::Codex, NOW + 1);
    rollout.last_event = "agent_reasoning".into();
    sessions = vec![rollout];
    cache.merge(fixture.dir.path(), NOW + 2, &mut sessions);
    assert_eq!(sessions[0].phase, Phase::Running);
    assert_eq!(sessions[0].last_seen, NOW + 1);
    assert!(
        !sessions[0].observed_alive,
        "unknown lock state cannot preserve liveness proof"
    );
}

#[test]
#[ignore = "reads the installed Codex history without changing any thread"]
fn native_paginated_history_is_readable() {
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".codex")
        });
    let snapshot = read(&home, crate::now_unix_secs()).expect("compatible local desktop history");
    eprintln!(
        "Codex history: {} current sessions, {} archived, {} live writers",
        snapshot.sessions.len(),
        snapshot.archived.len(),
        snapshot
            .sessions
            .iter()
            .filter(|state| state.observed_alive)
            .count()
    );
}

#[test]
fn a_reacquired_writer_cannot_revive_stale_in_progress_history() {
    let fixture = Fixture::new();
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let writer = File::create(locks.join(format!("{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    let mut cache = SessionCache::default();
    assert!(cache.scan(fixture.dir.path(), NOW).unwrap()[0].observed_alive);
    drop(writer);
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW + 1).unwrap()[0].phase,
        Phase::Completed
    );
    let writer = File::open(locks.join(format!("{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW + 2).unwrap()[0].phase,
        Phase::Completed
    );
    assert!(
        cache
            .scan(fixture.dir.path(), NOW + 86400)
            .unwrap()
            .is_empty()
    );
    fixture
        .history
        .execute("UPDATE thread_turns SET turn_id = 'turn-2'", [])
        .unwrap();
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW + 3).unwrap()[0].phase,
        Phase::Running
    );
}

#[test]
fn subsecond_rollout_state_is_not_overwritten_by_older_history() {
    for completed in [true, false] {
        let fixture = Fixture::new();
        fixture
            .history
            .execute(
                "INSERT INTO thread_items VALUES (?1, 'turn-1', ?2)",
                (ID, (NOW * 1000 + 300) as i64),
            )
            .unwrap();
        if !completed {
            fixture
                .history
                .execute(
                    "UPDATE thread_turns SET status = 'completed', completed_at = ?1",
                    [NOW as i64],
                )
                .unwrap();
        }
        let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 10);
        rollout.last_seen = NOW;
        rollout.phase = if completed {
            Phase::Completed
        } else {
            Phase::Running
        };
        rollout.codex_activity = Some(CodexActivity {
            turn: Some(if completed { "turn-1" } else { "turn-2" }.into()),
            at: u128::from(NOW) * 1_000_000_000 + 500_000_000,
            started_at: None,
        });
        let mut sessions = vec![rollout.clone()];
        Cache::default().merge(fixture.dir.path(), NOW + 1, &mut sessions);
        assert_eq!(sessions[0].phase, rollout.phase);
        assert_eq!(sessions[0].codex_activity, rollout.codex_activity);
    }
}

#[test]
fn second_precision_history_completion_wins_over_same_turn_rollout_activity() {
    let fixture = Fixture::new();
    fixture
        .history
        .execute(
            "UPDATE thread_turns SET status = 'completed', completed_at = ?1",
            [NOW as i64],
        )
        .unwrap();
    let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 10);
    rollout.last_seen = NOW;
    rollout.codex_activity = Some(CodexActivity {
        turn: Some("turn-1".into()),
        at: u128::from(NOW) * 1_000_000_000 + 300_000_000,
        started_at: None,
    });
    let mut sessions = vec![rollout];
    Cache::default().merge(fixture.dir.path(), NOW + 1, &mut sessions);
    assert_eq!(sessions[0].phase, Phase::Completed);
    assert_eq!(sessions[0].last_event, "task_complete");
}

#[test]
fn stopped_history_survives_a_database_outage_but_archiving_removes_its_cache() {
    let fixture = Fixture::new();
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let writer = File::create(locks.join(format!("{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    let mut cache = SessionCache::default();
    cache.scan(fixture.dir.path(), NOW).unwrap();
    drop(writer);
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW + 1).unwrap()[0].phase,
        Phase::Completed
    );
    fixture
        .history
        .execute("ALTER TABLE thread_turns RENAME TO unavailable", [])
        .unwrap();
    assert!(cache.scan(fixture.dir.path(), NOW + 32).unwrap().is_empty());
    assert!(cache.stopped.contains_key(ID));
    fixture
        .history
        .execute("ALTER TABLE unavailable RENAME TO thread_turns", [])
        .unwrap();
    let writer = File::open(locks.join(format!("{ID}.lock"))).unwrap();
    writer.lock().unwrap();
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW + 33).unwrap()[0].phase,
        Phase::Completed
    );
    fixture
        .state
        .execute("UPDATE threads SET archived = 1", [])
        .unwrap();
    assert!(cache.scan(fixture.dir.path(), NOW + 34).unwrap().is_empty());
    assert!(cache.stopped.is_empty());
}

#[test]
fn leaving_the_history_query_window_is_not_evidence_of_deletion() {
    let fixture = Fixture::new();
    let locks = fixture.dir.path().join("thread-writer-locks");
    std::fs::create_dir(&locks).unwrap();
    File::create(locks.join(".coordination.lock")).unwrap();
    let writer = File::create(locks.join(format!("{ID}.lock"))).unwrap();
    let mut cache = SessionCache::default();
    assert_eq!(
        cache.scan(fixture.dir.path(), NOW).unwrap()[0].phase,
        Phase::Completed
    );
    for index in 1..=256 {
        let id = format!("02a08965-575f-7870-b001-{index:012x}");
        fixture
            .state
            .execute(
                "INSERT INTO threads VALUES (?1, '/synthetic', '', '', 0, 'cli', 'paginated', ?2)",
                (&id, (NOW + index) as i64),
            )
            .unwrap();
        fixture
            .history
            .execute(
                "INSERT INTO thread_turns VALUES (?1, 'other', 1, 'completed', ?2, ?2)",
                (&id, NOW as i64),
            )
            .unwrap();
    }
    assert!(
        !cache
            .scan(fixture.dir.path(), NOW + 1)
            .unwrap()
            .iter()
            .any(|session| session.session_id == ID)
    );
    writer.lock().unwrap();
    fixture
        .state
        .execute(
            "UPDATE threads SET updated_at = ?1 WHERE id = ?2",
            ((NOW + 300) as i64, ID),
        )
        .unwrap();
    let sessions = cache.scan(fixture.dir.path(), NOW + 2).unwrap();
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.session_id == ID)
            .unwrap()
            .phase,
        Phase::Completed
    );
}

#[test]
fn an_exact_timestamp_tie_between_different_turns_uses_known_start_order() {
    for new_rollout in [true, false] {
        let fixture = Fixture::new();
        if new_rollout {
            fixture
                .history
                .execute(
                    "UPDATE thread_turns SET status = 'completed', completed_at = ?1",
                    [NOW as i64],
                )
                .unwrap();
        } else {
            fixture
                .history
                .execute(
                    "UPDATE thread_turns SET turn_id = 'turn-2', started_at = ?1",
                    [NOW as i64],
                )
                .unwrap();
        }
        let mut rollout = SessionState::new(ID, HookSource::Codex, NOW - 10);
        rollout.last_seen = NOW;
        rollout.phase = if new_rollout {
            Phase::Running
        } else {
            Phase::Completed
        };
        rollout.last_event = if new_rollout {
            "task_started"
        } else {
            "task_complete"
        }
        .into();
        rollout.codex_activity = Some(CodexActivity {
            turn: Some(if new_rollout { "turn-2" } else { "turn-1" }.into()),
            at: u128::from(NOW) * 1_000_000_000,
            started_at: Some(u128::from(if new_rollout { NOW } else { NOW - 10 }) * 1_000_000_000),
        });
        let mut sessions = vec![rollout];
        Cache::default().merge(fixture.dir.path(), NOW + 1, &mut sessions);
        assert_eq!(sessions[0].phase, Phase::Running);
        assert_eq!(
            sessions[0].codex_activity.as_ref().unwrap().turn.as_deref(),
            Some("turn-2")
        );
    }
}
