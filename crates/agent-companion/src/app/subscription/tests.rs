use super::*;

#[test]
fn slow_reads_start_cache_lifetime_at_completion_and_instances_stay_separate() {
    let mut primary = Monitor::default();
    let mut secondary = Monitor::default();
    primary.completed_at = Some(Instant::now() - CACHE_TTL - Duration::from_secs(20));
    let (tx, rx) = mpsc::channel();
    primary.receiver = Some(rx);
    let limits = serde_json::from_value(json!({"rateLimits":{"limitId":"codex","secondary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":2000}}})).unwrap();
    tx.send(Snapshot {
        limits: Some(limits),
        ..Default::default()
    })
    .unwrap();
    assert!(primary.poll());
    assert_eq!(primary.weekly(1000), Some((88, Some(2000))));
    assert!(primary.completed_at.unwrap().elapsed() < Duration::from_secs(2));
    assert!(secondary.weekly(1000).is_none());
    secondary.stop();
    assert_eq!(primary.weekly(1000), Some((88, Some(2000))));
    assert!(primary.weekly(2000).is_none());
    let fresh_local = agent_companion_core::usage::parse_codex_rate_limits(&json!({
        "secondary":{"used_percent":5,"window_minutes":10080,"resets_at":4000}
    }));
    assert_eq!(
        super::super::instances::weekly(Some(&fresh_local), &primary, 2001),
        Some((95, Some(4000)))
    );
    primary.completed_at = Some(Instant::now() - CACHE_TTL);
    assert!(primary.weekly(1000).is_none());
}

#[test]
fn weekly_quota_does_not_borrow_other_buckets_or_short_windows() {
    let limits: Limits = serde_json::from_value(json!({
        "rateLimits":{"limitId":"other","secondary":{"usedPercent":10,"windowDurationMins":10080}},
        "rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":1,"windowDurationMins":300}},"other":{"secondary":{"usedPercent":10,"windowDurationMins":10080}}}
    })).unwrap();
    assert!(limits.weekly(1000).is_none());
}

#[tokio::test]
#[ignore = "reads the installed Codex subscription over the network"]
async fn installed_codex_reports_subscription_usage() {
    let snapshot = read(agent_companion_core::install::codex_home().unwrap()).await;
    assert!(snapshot.error.is_empty(), "{}", snapshot.error);
    assert!(snapshot.limits.is_some(), "{}", snapshot.limits_error);
    assert!(snapshot.tokens.is_some(), "{}", snapshot.token_error);
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
fn multiple_limit_buckets_and_expired_windows_are_not_invented_allowance() {
    let limits: Limits = serde_json::from_value(json!({
        "rateLimits": {"limitId":"codex"},
        "rateLimitsByLimitId": {
            "review": {"limitName":"Code review", "primary":{"usedPercent":-5, "resetsAt":200}},
            "codex": {"planType":"pro", "primary":{"usedPercent":35, "windowDurationMins":300, "resetsAt":100},
                "secondary":{"usedPercent":130, "windowDurationMins":10080, "resetsAt":300}}
        }
    })).unwrap();
    let rows = limits.rows(100, 0, 50, 20);
    assert_eq!(rows[0].label, "Codex · pro");
    assert_eq!(rows[1].value, "—");
    assert!(rows[1].resets.starts_with("Reset passed"));
    assert_eq!(rows[2].value, "0%");
    assert_eq!(rows[4].value, "100%");
}

#[tokio::test]
async fn account_reads_are_read_only_and_keep_partial_results() {
    let (client, server) = tokio::io::duplex(16384);
    let (read, write) = tokio::io::split(client);
    let mut rpc = Rpc::new(read, write, Duration::from_secs(2));
    let fixture = async {
        let (read, mut write) = tokio::io::split(server);
        let mut lines = BufReader::new(read).lines();
        let initialize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(
            initialize["params"]["capabilities"]["experimentalApi"],
            true
        );
        write
            .write_all(b"{\"id\":1,\"result\":{}}\n")
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap()).unwrap()["method"],
            "initialized"
        );
        let account: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(
            account,
            json!({"id":2,"method":"account/read","params":{"refreshToken":false}})
        );
        write
            .write_all(b"{\"id\":2,\"result\":{\"account\":{\"type\":\"chatgpt\"}}}\n")
            .await
            .unwrap();
        for method in ["account/rateLimits/read", "account/usage/read"] {
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], method);
        }
        // Notifications and out-of-order responses are common app-server traffic.
        write.write_all(b"{\"method\":\"notification\"}\n{\"id\":4,\"error\":{\"code\":-32601,\"message\":\"sensitive server detail\"}}\n{\"id\":3,\"result\":{\"rateLimits\":{\"planType\":\"plus\"}}}\n").await.unwrap();
    };
    let (snapshot, ()) = tokio::join!(exchange(&mut rpc), fixture);
    let snapshot = snapshot.unwrap();
    assert!(snapshot.limits.is_some());
    assert!(snapshot.tokens.is_none());
    assert!(snapshot.token_error.starts_with("Update Codex CLI"));
    assert!(!snapshot.token_error.contains("sensitive"));
    assert!(snapshot.read_at.is_some());
}

#[tokio::test]
async fn signed_out_and_api_key_accounts_do_not_request_subscription_endpoints() {
    for account in [Value::Null, json!({"type":"apiKey"})] {
        let (client, mut server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let mut rpc = Rpc::new(read, write, Duration::from_secs(1));
        server
            .write_all(
                format!(
                    "{{\"id\":1,\"result\":{{}}}}\n{}\n",
                    json!({"id":2,"result":{"account":account}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let snapshot = exchange(&mut rpc).await.unwrap();
        assert!(snapshot.error.starts_with("Sign in to Codex"));
        assert!(snapshot.read_at.is_none());
        drop(rpc);
        let mut requests = String::new();
        server.read_to_string(&mut requests).await.unwrap();
        assert!(!requests.contains("account/usage/read"));
        assert!(!requests.contains("account/rateLimits/read"));
    }
}

#[tokio::test]
async fn a_token_timeout_preserves_limits_and_reads_have_byte_limits() {
    let (client, mut server) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client);
    let mut rpc = Rpc::new(read, write, Duration::from_millis(30));
    server.write_all(b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"account\":{\"type\":\"chatgpt\"}}}\n{\"id\":3,\"result\":{\"rateLimits\":{}}}\n").await.unwrap();
    let snapshot = exchange(&mut rpc).await.unwrap();
    assert!(snapshot.limits.is_some());
    assert!(snapshot.token_error.contains("did not finish"));
    let bytes = vec![b'x'; MAX_RESPONSE_BYTES + 1];
    let mut rpc = Rpc::new(bytes.as_slice(), tokio::io::sink(), Duration::from_secs(1));
    assert!(
        rpc.receive(1)
            .await
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
    assert!(rpc.received <= MAX_RESPONSE_BYTES + 1);
}

#[test]
fn completed_reads_are_cached_and_cancelled_results_cannot_replace_them() {
    let home = PathBuf::from("usage-test-home");
    let mut monitor = Monitor::default();
    monitor.source_home = Some(home.clone());
    monitor.completed_at = Some(Instant::now());
    monitor.refresh(home.clone(), false);
    assert!(
        !monitor.loading(),
        "completed reads are cached for five minutes"
    );
    monitor.stop();
    monitor.refresh(home, false);
    assert!(!monitor.loading(), "reopening uses a completed read");
    let (tx, rx) = mpsc::channel();
    let (cancel, mut cancelled) = oneshot::channel();
    monitor.receiver = Some(rx);
    monitor.cancel = Some(cancel);
    monitor.stop();
    assert!(
        monitor.completed_at.is_some(),
        "cancelling a refresh preserves the completed cache lifetime"
    );
    assert!(cancelled.try_recv().is_ok());
    assert!(
        tx.send(Snapshot::default()).is_err(),
        "late results have no receiver"
    );
    monitor.snapshot.tokens = Some(Tokens {
        summary: Summary::default(),
        daily_usage_buckets: None,
    });
    let (tx, rx) = mpsc::channel();
    monitor.receiver = Some(rx);
    tx.send(Snapshot::failure("signed out")).unwrap();
    assert!(monitor.poll());
    assert!(
        monitor.snapshot.tokens.is_none(),
        "failure clears account data"
    );
    assert!(!monitor.loading());
}
