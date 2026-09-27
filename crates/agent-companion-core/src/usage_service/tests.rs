use super::*;
use serde_json::json;

fn source(id: &str) -> Source {
    Source {
        instance_id: id.into(),
        codex_home: format!("/{id}/home").into(),
        executable_path: Some(format!("/{id}/codex").into()),
        database_path: format!("/{id}/database").into(),
    }
}

fn finish(scheduler: &mut Scheduler, requests: &[Request], now: u64) {
    for request in requests {
        assert!(scheduler.complete(request, Ok(json!({"rateLimits":{}})), now, 7));
    }
}

#[test]
fn startup_timer_panel_and_selected_refresh_are_the_only_quota_triggers() {
    let mut scheduler = Scheduler::new(5);
    let startup = scheduler.sync_sources(vec![source("codex"), source("dodex")], 1_000);
    assert_eq!(startup.len(), 2);
    assert!(
        startup
            .iter()
            .all(|request| request.kind == QueryKind::Limits)
    );
    assert!(scheduler.panel_open(1_010).is_empty());
    assert!(scheduler.refresh("codex", 1_010).is_empty());
    assert!(
        scheduler
            .sync_sources(vec![source("codex"), source("dodex")], 1_010)
            .is_empty()
    );
    finish(&mut scheduler, &startup, 1_020);
    assert!(scheduler.tick(1_319).is_empty());
    let timer = scheduler.tick(1_320);
    assert_eq!(timer.len(), 2);
    finish(&mut scheduler, &timer, 1_321);
    let panel = scheduler.panel_open(1_322);
    assert_eq!(panel.len(), 2);
    finish(&mut scheduler, &panel, 1_323);
    let manual = scheduler.refresh("dodex", 1_324);
    assert_eq!(manual.len(), 1);
    assert_eq!(manual[0].source.instance_id, "dodex");
    // Task/render/selection/close events have no scheduler entry point. A read
    // of the same state, however old, cannot become an implicit refresh.
    for _ in 0..10 {
        assert!(scheduler.snapshot("codex").is_some());
    }
    assert!(scheduler.tick(1_400).is_empty());
}

#[test]
fn failures_wait_a_full_interval_and_sleep_does_not_queue_replays() {
    let mut scheduler = Scheduler::new(5);
    let startup = scheduler.sync_sources(vec![source("codex")], 100);
    scheduler.complete(&startup[0], Err("offline".into()), 120, 20_000);
    assert!(scheduler.tick(419).is_empty());
    let resumed = scheduler.tick(40_000);
    assert_eq!(resumed.len(), 1);
    for now in [40_000, 40_010, 60_000] {
        assert!(scheduler.tick(now).is_empty());
    }
    scheduler.complete(&resumed[0], Err("offline".into()), 60_010, 20_000);
    assert!(scheduler.tick(60_309).is_empty());
    assert_eq!(scheduler.tick(60_310).len(), 1);
}

#[test]
fn interval_change_restarts_the_wait_without_touching_history_or_values() {
    let mut scheduler = Scheduler::new(5);
    let requests = scheduler.sync_sources(vec![source("codex"), source("dodex")], 100);
    finish(&mut scheduler, &requests, 110);
    let history = scheduler.load_history("codex", 120);
    finish(&mut scheduler, &history, 130);
    scheduler.set_interval(1, 200).unwrap();
    assert!(scheduler.tick(259).is_empty());
    assert_eq!(scheduler.tick(260).len(), 2);
    assert_eq!(
        scheduler.snapshot("codex").unwrap().history.next_query_at,
        Some(430)
    );
    assert!(scheduler.set_interval(0, 260).is_err());
    assert!(scheduler.set_interval(61, 260).is_err());
    assert_eq!(scheduler.interval_minutes(), 1);
}

#[test]
fn successful_values_survive_age_reset_and_querying_but_failure_alone_marks_them() {
    let mut scheduler = Scheduler::new(5);
    let startup = scheduler.sync_sources(vec![source("codex")], 100);
    let value = json!({"rateLimits":{"secondary":{"usedPercent":73,"resetsAt":200}}});
    scheduler.complete(&startup[0], Ok(value.clone()), 110, 3);
    for now in [150, 201, 500_000] {
        let request = scheduler.refresh("codex", now);
        let limits = scheduler.snapshot("codex").unwrap().limits;
        assert_eq!(limits.value, Some(value.clone()));
        assert!(!limits.failed_reading());
        assert!(limits.loading);
        scheduler.complete(&request[0], Ok(value.clone()), now, 3);
    }
    let request = scheduler.refresh("codex", 500_010);
    scheduler.complete(&request[0], Err("offline".into()), 500_011, 3);
    let failed = scheduler.snapshot("codex").unwrap().limits;
    assert!(failed.failed_reading());
    assert_eq!(failed.last_success_at, Some(500_000));
    let request = scheduler.refresh("codex", 500_012);
    assert!(scheduler.snapshot("codex").unwrap().limits.failed_reading());
    scheduler.complete(&request[0], Ok(json!({"rateLimits":{}})), 500_013, 3);
    let empty = scheduler.snapshot("codex").unwrap().limits;
    assert!(!empty.failed_reading());
    assert_eq!(empty.value, Some(json!({"rateLimits":{}})));
    assert_eq!(empty.last_success_at, Some(500_013));
}

#[test]
fn initial_failure_has_no_invented_value_and_open_or_manual_can_retry_immediately() {
    let mut scheduler = Scheduler::new(5);
    let request = scheduler.sync_sources(vec![source("codex")], 100);
    scheduler.complete(&request[0], Err("signed out".into()), 101, 1);
    let failed = scheduler.snapshot("codex").unwrap().limits;
    assert!(failed.value.is_none());
    assert!(!failed.failed_reading());
    assert_eq!(failed.error.as_deref(), Some("signed out"));
    assert_eq!(scheduler.panel_open(102).len(), 1);
}

#[test]
fn history_is_independent_and_caches_failures_from_completion() {
    let mut scheduler = Scheduler::new(1);
    let limits = scheduler.sync_sources(vec![source("codex"), source("dodex")], 100);
    let history = scheduler.load_history("codex", 100);
    assert_eq!(history.len(), 1);
    assert!(scheduler.load_history("codex", 101).is_empty());
    assert_eq!(scheduler.load_history("dodex", 101).len(), 1);
    finish(&mut scheduler, &limits, 102);
    assert!(scheduler.snapshot("codex").unwrap().history.loading);
    assert!(!scheduler.snapshot("codex").unwrap().limits.loading);
    scheduler.complete(&history[0], Err("unsupported".into()), 120, 20_000);
    assert!(scheduler.load_history("codex", 419).is_empty());
    assert!(
        scheduler
            .tick(1_000)
            .iter()
            .all(|request| request.kind == QueryKind::Limits)
    );
    assert!(!scheduler.snapshot("codex").unwrap().history.loading);
    assert_eq!(scheduler.load_history("codex", 1_000).len(), 1);
}

#[test]
fn source_removal_and_each_identity_change_ignore_late_results() {
    for change in 0..4 {
        let mut scheduler = Scheduler::new(5);
        let initial = source("codex");
        let old = scheduler.sync_sources(vec![initial.clone()], 100);
        let mut replacement = initial;
        let sources = match change {
            0 => {
                replacement.executable_path = Some("/new/runtime".into());
                vec![replacement]
            }
            1 => {
                replacement.codex_home = "/new/config".into();
                vec![replacement]
            }
            2 => {
                replacement.database_path = "/new/database".into();
                vec![replacement]
            }
            _ => Vec::new(),
        };
        let new = scheduler.sync_sources(sources, 101);
        assert!(!scheduler.is_current(&old[0]));
        assert!(!scheduler.complete(&old[0], Ok(json!({"wrong":"account"})), 102, 2));
        if change < 3 {
            assert_eq!(new.len(), 1);
            assert!(scheduler.snapshot("codex").unwrap().limits.value.is_none());
            finish(&mut scheduler, &new, 103);
        } else {
            assert!(scheduler.snapshot("codex").is_none());
        }
    }
}

#[test]
fn settings_default_validate_round_trip_and_recover_from_damage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings/usage.json");
    assert_eq!(UsageSettings::load(&path).refresh_interval_minutes, 5);
    for minutes in [1, 5, 60] {
        let settings = UsageSettings {
            refresh_interval_minutes: minutes,
        };
        settings.save(&path).unwrap();
        assert_eq!(UsageSettings::load(&path), settings);
    }
    for contents in [
        "broken",
        "null",
        "{\"refreshIntervalMinutes\":0}",
        "{\"refreshIntervalMinutes\":61}",
        "{\"refreshIntervalMinutes\":1.5}",
        "{\"refreshIntervalMinutes\":\"5\"}",
    ] {
        fs::write(&path, contents).unwrap();
        assert_eq!(UsageSettings::load(&path), UsageSettings::default());
    }
    for minutes in [0, 61, 255] {
        assert!(
            UsageSettings {
                refresh_interval_minutes: minutes
            }
            .save(&path)
            .is_err()
        );
    }
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn interval_input_requires_an_integer_in_the_supported_range() {
    for (input, expected) in [("1", 1), ("5", 5), ("60", 60)] {
        assert_eq!(parse_interval(input).unwrap(), expected);
    }
    for input in [
        "0",
        "61",
        "1.5",
        "1e1",
        "-1",
        "+1",
        "abc",
        "",
        " 5 ",
        "9999999999999999",
    ] {
        assert!(parse_interval(input).is_err(), "accepted {input:?}");
    }
}
