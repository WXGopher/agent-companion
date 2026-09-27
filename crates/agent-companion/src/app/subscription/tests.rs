use super::*;
use agent_companion_core::usage_service::{Scheduler, Source};
use serde_json::json;

fn source(id: &str) -> Source {
    Source {
        instance_id: id.into(),
        codex_home: format!("/{id}/home").into(),
        database_path: format!("/{id}/database").into(),
        executable_path: Some(format!("/{id}/codex.exe").into()),
    }
}

fn allowance() -> serde_json::Value {
    json!({"rateLimits":{"limitId":"codex","secondary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":2000}}})
}

fn snapshot(scheduler: &Scheduler) -> Snapshot {
    Snapshot::from_shared(scheduler.snapshot("codex"))
}

#[test]
fn age_reset_and_in_flight_queries_preserve_values_and_only_failures_add_stars() {
    let mut service = Scheduler::new(5);
    let initial = service.sync_sources(vec![source("codex")], 1000).remove(0);
    service.complete(&initial, Ok(allowance()), 1001, 120);
    let first = snapshot(&service);
    assert_eq!(first.weekly(), Some((88, Some(2000))));
    assert_eq!(
        first.limits.unwrap().rows(1002, 0, 50, 20, false)[1].value,
        "88%"
    );

    // Advancing time, including past reset, neither changes the reading nor
    // adds a star. The description explicitly waits for the next query.
    let aged = snapshot(&service);
    assert!(!aged.failed());
    let rows = aged
        .limits
        .as_ref()
        .unwrap()
        .rows(2001, 0, 50, 20, aged.failed());
    assert_eq!(rows[1].value, "88%");
    assert_eq!(
        rows[1].resets,
        "Reset time reached · waiting for the next query"
    );
    let refresh = service.refresh("codex", 2001).remove(0);
    let loading = snapshot(&service);
    assert!(loading.loading);
    assert_eq!(
        quota_value(loading.weekly().unwrap().0, loading.failed()),
        "88%"
    );
    assert_eq!(loading.read_at, Some(1001));

    service.complete(&refresh, Err("connection unavailable".into()), 2002, 1000);
    let failed = snapshot(&service);
    assert_eq!(
        quota_value(failed.weekly().unwrap().0, failed.failed()),
        "88%*"
    );
    assert!(failed.status(2002, 0).contains("connection unavailable"));
    assert!(failed.status(2002, 0).contains("Last success"));
    let retry = service.refresh("codex", 2003).remove(0);
    let loading = snapshot(&service);
    assert!(loading.loading && loading.failed());
    assert_eq!(
        quota_value(loading.weekly().unwrap().0, loading.failed()),
        "88%*"
    );
    service.complete(&retry, Ok(allowance()), 2004, 60);
    let recovered = snapshot(&service);
    assert_eq!(
        quota_value(recovered.weekly().unwrap().0, recovered.failed()),
        "88%"
    );
    assert_eq!(recovered.read_at, Some(2004));
}

#[test]
fn first_failure_has_no_value_and_success_without_a_window_clears_old_allowance() {
    let mut service = Scheduler::new(5);
    let initial = service.sync_sources(vec![source("codex")], 1000).remove(0);
    service.complete(&initial, Err("sign in required".into()), 1001, 20);
    let failed = snapshot(&service);
    assert!(failed.weekly().is_none());
    assert!(failed.read_at.is_none());
    assert_eq!(failed.limits_error, "sign in required");

    let refresh = service.refresh("codex", 1002).remove(0);
    service.complete(&refresh, Ok(allowance()), 1003, 30);
    assert!(snapshot(&service).weekly().is_some());
    let refresh = service.refresh("codex", 1004).remove(0);
    service.complete(
        &refresh,
        Ok(json!({"rateLimits":{"limitId":"codex"}})),
        1005,
        30,
    );
    let empty = snapshot(&service);
    assert!(empty.weekly().is_none());
    assert!(!empty.failed());
    assert_eq!(empty.read_at, Some(1005));
    let rows = empty.limits.unwrap().rows(1005, 0, 50, 20, false);
    assert_eq!(rows[1].value, "—");
}

#[test]
fn history_loading_and_failure_are_independent_of_allowance() {
    let mut service = Scheduler::new(5);
    let allowance = service.sync_sources(vec![source("codex")], 1000).remove(0);
    let history = service.load_history("codex", 1001).remove(0);
    service.complete(&history, Err("history unavailable".into()), 1002, 10);
    let reading = snapshot(&service);
    assert!(reading.loading);
    assert!(!reading.history_loading);
    assert!(!reading.failed());
    assert_eq!(reading.token_error, "history unavailable");
    service.complete(&allowance, Ok(json!({"rateLimits":{}})), 1003, 30);
    let reading = snapshot(&service);
    assert!(!reading.loading);
    assert!(!reading.failed());
    assert_eq!(reading.token_error, "history unavailable");
}

#[test]
fn weekly_quota_does_not_borrow_other_buckets_or_short_windows() {
    let limits: Limits = serde_json::from_value(json!({
        "rateLimits":{"limitId":"other","secondary":{"usedPercent":10,"windowDurationMins":10080}},
        "rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":1,"windowDurationMins":300}},"other":{"secondary":{"usedPercent":10,"windowDurationMins":10080}}}
    })).unwrap();
    assert!(limits.weekly().is_none());
}

#[test]
fn a_nonempty_named_map_does_not_leak_the_default_bucket() {
    let limits: Limits = serde_json::from_value(json!({
        "rateLimits": {"limitId":"codex", "secondary":{"usedPercent":12,"windowDurationMins":10080}},
        "rateLimitsByLimitId": {"review":{"primary":{"usedPercent":8,"windowDurationMins":300}}}
    })).unwrap();
    assert!(limits.weekly().is_none());
    let rows = limits.rows(1000, 0, 50, 20, false);
    assert_eq!(rows[0].label, "review");
    assert_eq!(rows[1].value, "92%");
}

#[test]
fn missing_values_and_reported_days_keep_their_meaning() {
    let tokens: Tokens = serde_json::from_value(json!({
        "summary": {"lifetimeTokens": 1_234_567, "peakDailyTokens": -1},
        "dailyUsageBuckets": [
            {"startDate":"2026-09-01", "tokens":0},
            {"startDate":"2026-09-11", "tokens":80},
            {"startDate":"2026-09-12", "tokens":-1},
            {"startDate":"2026-09-04", "tokens":50}
        ]
    }))
    .unwrap();
    assert_eq!(number(tokens.summary.lifetime_tokens), "1.2M");
    assert_eq!(number(tokens.summary.peak_daily_tokens), "—");
    assert_eq!(number(None), "—");
    assert_eq!(number(Some(0)), "0");
    assert_eq!(duration(Some(3725)), "1h 2m");
    assert_eq!(duration(Some(-1)), "—");
    assert_eq!(day_count(None), "—");
    assert_eq!(
        tokens
            .recent_days()
            .iter()
            .map(|day| day.start_date.as_str())
            .collect::<Vec<_>>(),
        ["2026-09-11", "2026-09-04", "2026-09-01"]
    );
    let many = Tokens {
        summary: Summary::default(),
        daily_usage_buckets: Some(
            (1..=10)
                .map(|day| Day {
                    start_date: format!("2026-09-{day:02}"),
                    tokens: day,
                })
                .collect(),
        ),
    };
    assert_eq!(many.recent_days().len(), 7);
    assert_eq!(many.recent_days().last().unwrap().start_date, "2026-09-04");
}

#[test]
fn multiple_limit_buckets_preserve_expired_values() {
    let limits: Limits = serde_json::from_value(json!({
        "rateLimits": {"limitId":"codex"},
        "rateLimitsByLimitId": {
            "review": {"limitName":"Code review", "primary":{"usedPercent":-5, "resetsAt":200}},
            "codex": {"planType":"pro", "primary":{"usedPercent":35, "windowDurationMins":300, "resetsAt":100},
                "secondary":{"usedPercent":130, "windowDurationMins":10080, "resetsAt":300}}
        }
    })).unwrap();
    let rows = limits.rows(100, 0, 50, 20, false);
    assert_eq!(rows[0].label, "Codex · pro");
    assert_eq!(rows[1].value, "65%");
    assert!(rows[1].resets.starts_with("Reset time reached"));
    assert_eq!(rows[2].value, "0%");
    assert_eq!(rows[4].value, "100%");
}

#[test]
fn history_navigation_only_enters_on_usage_entry_or_instance_change() {
    let mut navigation = Navigation::default();
    assert!(!navigation.select_page(false));
    assert!(navigation.select_page(true));
    assert!(!navigation.select_page(true));
    assert!(!navigation.select_instance(false));
    assert!(navigation.select_instance(true));
    assert!(navigation.secondary());
    assert!(!navigation.select_instance(true));
    assert!(!navigation.select_page(false));
    assert!(
        navigation.select_instance(true),
        "a task-page quota card enters Usage"
    );
    navigation.remove_secondary();
    assert!(!navigation.secondary());
}

#[test]
fn only_closed_to_open_and_preview_to_full_are_panel_query_events() {
    assert!(opens_full_panel(false, false, false));
    assert!(opens_full_panel(true, true, false));
    assert!(!opens_full_panel(true, false, false));
    assert!(!opens_full_panel(false, false, true));
    assert!(!opens_full_panel(true, true, true));
}

#[test]
fn visible_history_source_replacement_is_one_boundary_without_cache_polling() {
    let mut navigation = Navigation::default();
    navigation.select_page(true);
    let mut current = source("codex");
    navigation.history_entered(Some(current.clone()));
    for _ in 0..100 {
        assert!(
            !navigation.visible_history_source_changed(Some(current.clone()), true),
            "staying on Usage never rechecks the history cache"
        );
    }
    current.database_path = "/replacement/database".into();
    assert!(
        !navigation.visible_history_source_changed(Some(current.clone()), false),
        "a closed panel or preview does not enter history"
    );
    assert!(navigation.visible_history_source_changed(Some(current.clone()), true));
    assert!(!navigation.visible_history_source_changed(Some(current.clone()), true));
    current.executable_path = Some("/replacement/codex.exe".into());
    assert!(navigation.visible_history_source_changed(Some(current.clone()), true));
    current.codex_home = "/replacement/home".into();
    assert!(navigation.visible_history_source_changed(Some(current.clone()), true));
    assert!(!navigation.visible_history_source_changed(Some(current), true));
}

#[test]
fn disabling_the_selected_instance_loads_fallback_history_once() {
    let mut navigation = Navigation::default();
    navigation.select_instance(true);
    navigation.history_entered(Some(source("dodex")));
    navigation.remove_secondary();
    assert!(!navigation.secondary());
    assert!(navigation.visible_history_source_changed(Some(source("codex")), true));
    assert!(!navigation.visible_history_source_changed(Some(source("codex")), true));
}

#[test]
fn a_history_entry_before_source_registration_is_not_lost() {
    let mut navigation = Navigation::default();
    navigation.select_page(true);
    navigation.history_entered(None);
    assert!(!navigation.visible_history_source_changed(None, true));
    assert!(navigation.visible_history_source_changed(Some(source("codex")), true));
    assert!(!navigation.visible_history_source_changed(Some(source("codex")), true));
    assert!(!navigation.visible_history_source_changed(None, true));
    assert!(
        navigation.visible_history_source_changed(Some(source("codex")), true),
        "removing and re-registering the same source starts a new state"
    );
}
