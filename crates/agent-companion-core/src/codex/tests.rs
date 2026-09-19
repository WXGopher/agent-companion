use super::*;
use serde_json::json;
use std::io::Write;

fn now(second: u64) -> u64 {
    parse_iso8601("2026-09-05T00:00:00Z").unwrap() + second
}

fn event(second: u64, kind: &str, turn: &str) -> Value {
    json!({"timestamp": format!("2026-09-05T00:00:{second:02}Z"), "type": "event_msg",
        "payload": {"type": kind, "turn_id": turn}})
}

fn append(path: &Path, rows: &[Value]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
}

fn fixture(source: Value) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions/2026/09/05");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("rollout-test.jsonl");
    let meta = json!({"timestamp": "2026-09-05T00:00:00Z", "type": "session_meta", "payload": {
        "id": "s-1", "cwd": "C:/synthetic/agent-companion", "source": source}});
    fs::write(&path, format!("{meta}\n")).unwrap();
    (dir, path)
}

#[test]
fn replayed_parent_metadata_cannot_reinclude_a_subagent() {
    for source in [json!("subagent"), json!({"subagent": {"thread_spawn": {}}})] {
        for replayed_id in ["parent", "s-1"] {
            let (dir, path) = fixture(source.clone());
            append(
                &path,
                &[
                    json!({"timestamp":"2026-09-05T00:00:01Z", "type":"session_meta", "payload": {
                    "id":replayed_id, "source":"cli", "cwd":"/synthetic/parent"}}),
                    event(2, "task_started", "turn"),
                ],
            );
            let mut cache = SessionCache::default();
            assert!(cache.scan(dir.path(), now(3)).unwrap().is_empty());
            append(&path, &[event(4, "agent_message", "turn")]);
            assert!(cache.scan(dir.path(), now(5)).unwrap().is_empty());
        }
    }
}

#[test]
fn a_rollout_keeps_its_own_identity_when_other_metadata_is_replayed() {
    let (dir, path) = fixture(json!("cli"));
    append(
        &path,
        &[
            json!({"timestamp":"2026-09-05T00:00:01Z", "type":"session_meta", "payload": {
            "id":"another-thread", "source":"subagent", "cwd":"/synthetic/other"}}),
            event(2, "task_started", "turn"),
        ],
    );
    let sessions = SessionCache::default().scan(dir.path(), now(3)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s-1");
    assert_eq!(sessions[0].codex_client, CodexClient::Cli);
    assert_eq!(
        sessions[0].cwd.as_deref(),
        Some("C:/synthetic/agent-companion")
    );
}

#[test]
fn rollout_copies_count_once_and_follow_the_newest_event() {
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "turn")]);
    let copy = path.with_file_name("rollout-copy.jsonl");
    fs::copy(&path, &copy).unwrap();
    let mut cache = SessionCache::default();
    let active = cache.scan(dir.path(), now(2)).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].phase, Phase::Running);
    append(&path, &[event(3, "task_complete", "turn")]);
    let finished = cache.scan(dir.path(), now(4)).unwrap();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].phase, Phase::Completed);
    assert_eq!(
        Path::new(finished[0].transcript_path.as_deref().unwrap()),
        path
    );
    append(&copy, &[event(5, "task_started", "new-turn")]);
    let resumed = cache.scan(dir.path(), now(6)).unwrap();
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].phase, Phase::Running);
    assert_eq!(
        Path::new(resumed[0].transcript_path.as_deref().unwrap()),
        copy
    );
}

#[test]
fn tied_rollout_copies_do_not_resurrect_a_completed_turn() {
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "turn")]);
    let copy = path.with_file_name("rollout-copy.jsonl");
    fs::copy(&path, &copy).unwrap();
    append(&copy, &[event(1, "task_complete", "turn")]);
    for _ in 0..4 {
        let sessions = SessionCache::default().scan(dir.path(), now(2)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].phase, Phase::Completed);
        assert_eq!(
            Path::new(sessions[0].transcript_path.as_deref().unwrap()),
            copy
        );
    }
}

#[test]
fn rollout_metadata_distinguishes_cli_desktop_and_unknown_clients() {
    for (source, originator, expected) in [
        ("cli", "codex-tui", CodexClient::Cli),
        ("cli", "Codex Desktop", CodexClient::Cli),
        ("vscode", "Codex Desktop", CodexClient::Desktop),
        ("vscode", "codex_vscode", CodexClient::Unknown),
        ("appServer", "agent-companion", CodexClient::Unknown),
    ] {
        let (dir, path) = fixture(json!(source));
        append(
            &path,
            &[
                json!({"timestamp":"2026-09-05T00:00:00Z", "type":"session_meta", "payload": {
                "id":"s-1", "source":source, "originator":originator}}),
                event(1, "task_started", "t-1"),
            ],
        );
        let sessions = SessionCache::default().scan(dir.path(), now(2)).unwrap();
        assert_eq!(sessions[0].codex_client, expected);
        assert_eq!(
            Path::new(sessions[0].transcript_path.as_deref().unwrap()),
            path
        );
    }
}

#[test]
fn recovers_an_already_running_session_and_follows_completion_and_the_next_turn() {
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "t1")]);
    let mut cache = SessionCache::default();
    let running = cache.scan(dir.path(), now(2)).unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].phase, Phase::Running);
    assert_eq!(
        running[0].cwd.as_deref(),
        Some("C:/synthetic/agent-companion")
    );
    assert_eq!(
        SessionCache::default().scan(dir.path(), now(2)).unwrap(),
        running
    );

    append(
        &path,
        &[
            event(3, "task_complete", "t1"),
            event(4, "token_count", "t1"),
        ],
    );
    let done = cache.scan(dir.path(), now(5)).unwrap();
    assert_eq!(done[0].phase, Phase::Completed);
    assert_eq!(done[0].last_seen, now(3));
    append(
        &path,
        &[
            event(6, "task_started", "t2"),
            event(7, "task_complete", "t1"),
        ],
    );
    assert_eq!(
        cache.scan(dir.path(), now(8)).unwrap()[0].phase,
        Phase::Running
    );
    append(&path, &[event(9, "turn_aborted", "t2")]);
    assert_eq!(
        cache.scan(dir.path(), now(10)).unwrap()[0].phase,
        Phase::Completed
    );
}

#[test]
fn notices_appends_even_when_windows_keeps_the_original_mtime() {
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "t1")]);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut cache = SessionCache::default();
    cache.scan(dir.path(), now(2)).unwrap();
    append(&path, &[event(3, "task_complete", "t1")]);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        cache.scan(dir.path(), now(4)).unwrap()[0].phase,
        Phase::Completed
    );
}

#[test]
fn retries_a_half_written_record_and_ignores_malformed_lines() {
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "t1")]);
    let mut cache = SessionCache::default();
    cache.scan(dir.path(), now(2)).unwrap();
    let record = event(3, "task_complete", "t1").to_string();
    let half = record.len() / 2;
    let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(&record.as_bytes()[..half]).unwrap();
    assert_eq!(
        cache.scan(dir.path(), now(4)).unwrap()[0].phase,
        Phase::Running
    );
    writer.write_all(&record.as_bytes()[half..]).unwrap();
    writer.write_all(b"\nnot json\n").unwrap();
    assert_eq!(
        cache.scan(dir.path(), now(5)).unwrap()[0].phase,
        Phase::Completed
    );
}

#[test]
fn old_files_and_repeated_scans_do_not_manufacture_active_sessions() {
    let (dir, path) = fixture(json!("cli"));
    let mut cache = SessionCache::default();
    assert!(cache.scan(dir.path(), now(1)).unwrap().is_empty());
    append(&path, &[event(1, "task_started", "t1")]);
    assert_eq!(cache.scan(dir.path(), now(2)).unwrap().len(), 1);
    assert!(
        cache
            .scan(dir.path(), now(1) + STALE_AFTER_SECS)
            .unwrap()
            .is_empty()
    );
    assert!(
        SessionCache::default()
            .scan(dir.path(), now(1) + STALE_AFTER_SECS)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn native_questions_wait_for_the_matching_answer_without_creating_approval_requests() {
    let (dir, path) = fixture(json!("vscode"));
    let response = |second: u64, payload: Value| {
        json!({
            "timestamp": format!("2026-09-05T00:00:{second:02}Z"),
            "type": "response_item", "payload": payload
        })
    };
    append(
        &path,
        &[
            event(1, "task_started", "turn"),
            response(
                2,
                json!({"type":"function_call", "name":"request_user_input", "call_id":"q1"}),
            ),
            response(3, json!({"type":"function_call_output", "call_id":"other"})),
        ],
    );
    let mut cache = SessionCache::default();
    let waiting = cache.scan(dir.path(), now(4)).unwrap();
    assert_eq!(waiting[0].phase, Phase::WaitingForAnswer);
    assert!(
        waiting[0].pending.is_empty(),
        "a log reading cannot answer a server request"
    );
    append(
        &path,
        &[response(
            5,
            json!({"type":"function_call_output", "call_id":"q1"}),
        )],
    );
    assert_eq!(
        cache.scan(dir.path(), now(6)).unwrap()[0].phase,
        Phase::Running
    );
    append(
        &path,
        &[response(
            7,
            json!({"type":"function_call", "name":"request_user_input_async", "call_id":"async"}),
        )],
    );
    assert_eq!(
        cache.scan(dir.path(), now(8)).unwrap()[0].phase,
        Phase::Running
    );
    append(
        &path,
        &[
            response(
                9,
                json!({"type":"function_call", "name":"request_user_input", "call_id":"q2"}),
            ),
            event(10, "turn_aborted", "turn"),
        ],
    );
    assert_eq!(
        cache.scan(dir.path(), now(11)).unwrap()[0].phase,
        Phase::Completed
    );
}

#[test]
fn writer_observations_extend_live_hooks_without_overwriting_or_pinning_them() {
    let mut table = crate::state::SessionTable::new();
    let payload =
        serde_json::from_value(json!({"session_id":"owned","hook_event_name":"UserPromptSubmit"}))
            .unwrap();
    table.apply(&payload, HookSource::Codex, now(1));
    let mut observed = SessionState::new("owned", HookSource::Codex, now(1));
    observed.observed_alive = true;
    table.sync_observed(HookSource::Codex, vec![observed.clone()], now(2));
    assert!(table.get("owned").unwrap().observed_alive);
    table.sweep(now(2) + STALE_AFTER_SECS);
    assert!(table.get("owned").is_some());
    table.sync_observed(HookSource::Codex, vec![], now(3) + STALE_AFTER_SECS);
    table.sweep(now(3) + STALE_AFTER_SECS);
    assert!(table.get("owned").is_none());
    table.apply(&payload, HookSource::Codex, now(1));
    table.sync_observed(HookSource::Codex, vec![observed], now(2));
    let stop =
        serde_json::from_value(json!({"session_id":"owned","hook_event_name":"Stop"})).unwrap();
    table.apply(&stop, HookSource::Codex, now(3));
    assert!(!table.get("owned").unwrap().observed_alive);
}

#[test]
fn skips_subagents_and_removes_deleted_or_truncated_rollouts() {
    let (subdir, subpath) =
        fixture(json!({"subagent": {"thread_spawn": {"parent_thread_id": "parent"}}}));
    append(&subpath, &[event(1, "task_started", "t1")]);
    assert!(
        SessionCache::default()
            .scan(subdir.path(), now(2))
            .unwrap()
            .is_empty()
    );

    let (dir, path) = fixture(json!("vscode"));
    append(&path, &[event(1, "task_started", "t1")]);
    let mut cache = SessionCache::default();
    assert_eq!(cache.scan(dir.path(), now(2)).unwrap().len(), 1);
    fs::write(&path, b"").unwrap();
    assert!(cache.scan(dir.path(), now(3)).unwrap().is_empty());
    fs::remove_file(&path).unwrap();
    assert!(cache.scan(dir.path(), now(4)).unwrap().is_empty());
    assert!(cache.files.is_empty());
}

#[test]
fn a_large_rollout_starts_with_its_recent_events() {
    let (dir, path) = fixture(json!("cli"));
    let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writer
        .write_all(&vec![b'x'; READ_BUDGET as usize + 100])
        .unwrap();
    writer.write_all(b"\n").unwrap();
    append(&path, &[event(1, "item_completed", "t1")]);
    let sessions = SessionCache::default().scan(dir.path(), now(2)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, Phase::Running);
}

#[test]
fn observed_sessions_update_counts_but_never_overwrite_live_hook_approvals() {
    use crate::protocol::HookPayload;
    use crate::state::SessionTable;
    let (dir, path) = fixture(json!("cli"));
    append(&path, &[event(1, "task_started", "t1")]);
    let observations = SessionCache::default().scan(dir.path(), now(2)).unwrap();
    let mut table = SessionTable::new();
    assert!(table.sync_observed(HookSource::Codex, observations.clone(), now(2)));
    assert_eq!(table.tasks(HookSource::Codex, now(2)).running, 1);
    assert!(!table.sync_observed(HookSource::Codex, observations.clone(), now(3)));
    let approval: HookPayload = serde_json::from_value(json!({"session_id": "s-1",
        "hook_event_name": "PermissionRequest", "tool_name": "Bash"}))
    .unwrap();
    table.apply(&approval, HookSource::Codex, now(4));
    assert!(!table.sync_observed(HookSource::Codex, observations, now(5)));
    assert_eq!(table.get("s-1").unwrap().phase, Phase::WaitingForApproval);
    assert!(!table.sync_observed(HookSource::Codex, vec![], now(6)));
    assert_eq!(table.get("s-1").unwrap().pending.len(), 1);

    let mut observed_only = SessionTable::new();
    let observations = SessionCache::default().scan(dir.path(), now(2)).unwrap();
    observed_only.sync_observed(HookSource::Codex, observations, now(2));
    assert!(observed_only.sync_observed(HookSource::Codex, vec![], now(3)));
    assert!(observed_only.is_empty());
}

#[test]
#[ignore = "reads this machine's real Codex session logs"]
fn inspect_local_sessions() {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".codex"));
    let sessions = SessionCache::default()
        .scan(&home, crate::now_unix_secs())
        .unwrap();
    for session in &sessions {
        eprintln!(
            "{} {} {}",
            session.session_id,
            session.phase.as_str(),
            session.last_event
        );
    }
    eprintln!("{} detected Codex sessions", sessions.len());
}
