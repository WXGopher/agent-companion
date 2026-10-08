use super::*;
use std::{
    sync::{atomic::AtomicUsize, mpsc},
    time::Instant,
};

fn release(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "html_url": "https://untrusted.example/do-not-open"
    }))
    .unwrap()
}

fn service(
    path: Option<PathBuf>,
    current_version: &str,
    fetch: impl Fn() -> io::Result<Vec<u8>> + Send + Sync + 'static,
) -> (UpdateService, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let service = UpdateService::create(
        path,
        current_version,
        Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            fetch()
        }),
        true,
    );
    (service, calls)
}

fn wait_idle(service: &UpdateService) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while service.inner.state().in_flight {
        assert!(Instant::now() < deadline, "Release worker did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn local_builds_skip_all_checks_and_ignore_newer_cached_releases() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    save_cache(
        &path,
        &Cache {
            last_attempt_at: Some(0),
            latest_tag: Some("v101.0.0".into()),
        },
    )
    .unwrap();
    let original = fs::read(&path).unwrap();
    for _ in 0..2 {
        let local = UpdateService::create(
            Some(path.clone()),
            "100.0.0",
            Box::new(|| panic!("Local builds must never contact the release service")),
            false,
        );
        for now in [0, CHECK_INTERVAL_SECS, CHECK_INTERVAL_SECS * 2] {
            local.panel_open(now);
            local.check_now(now);
            assert_eq!(local.snapshot(), UpdateSnapshot::default());
            assert_eq!(local.manual_snapshot(), ManualCheck::Disabled);
            assert!(!local.inner.state().in_flight);
        }
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn construction_and_snapshot_reads_never_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    save_cache(
        &path,
        &Cache {
            last_attempt_at: Some(0),
            latest_tag: Some("v0.3.22".into()),
        },
    )
    .unwrap();
    let (service, calls) = service(Some(path), "0.3.21", || Ok(release("v0.3.23")));
    for _ in 0..20 {
        assert_eq!(service.snapshot().latest_version.as_deref(), Some("0.3.22"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn successful_checks_run_only_at_the_daily_panel_open_boundary() {
    let (service, calls) = service(None, "0.3.21", || Ok(release("v0.3.22")));
    service.panel_open(0);
    wait_idle(&service);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(service.snapshot().latest_version.as_deref(), Some("0.3.22"));

    for now in [0, 1, CHECK_INTERVAL_SECS - 1] {
        service.panel_open(now);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    service.panel_open(CHECK_INTERVAL_SECS);
    wait_idle(&service);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn blocked_fetch_does_not_block_panel_or_snapshots_and_is_deduplicated() {
    let (started_tx, started_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let finish_rx = Mutex::new(finish_rx);
    let (service, calls) = service(None, "0.3.21", move || {
        started_tx.send(()).unwrap();
        finish_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .map_err(io::Error::other)?;
        Ok(release("v0.3.22"))
    });
    let service = Arc::new(service);
    let worker_service = Arc::clone(&service);
    let (opened_tx, opened_rx) = mpsc::channel();
    std::thread::spawn(move || {
        worker_service.panel_open(0);
        opened_tx.send(()).unwrap();
    });
    // The fake request cannot complete until finish_tx is sent below.
    opened_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let reading_service = Arc::clone(&service);
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Even a due open cannot start a second request while one is pending.
        reading_service.panel_open(2 * CHECK_INTERVAL_SECS);
        snapshot_tx.send(reading_service.snapshot()).unwrap();
    });
    assert_eq!(
        snapshot_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        UpdateSnapshot::default()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    finish_tx.send(()).unwrap();
    wait_idle(&service);
    // Completion appears during ordinary polling, without another panel open.
    assert_eq!(service.snapshot().latest_version.as_deref(), Some("0.3.22"));
}

#[test]
fn successful_cache_survives_restart_and_disappears_after_installing_update() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    let (first, _) = service(Some(path.clone()), "0.3.21", || Ok(release("v0.3.22")));
    first.panel_open(100);
    wait_idle(&first);
    drop(first);

    let (restarted, calls) = service(Some(path.clone()), "0.3.21", || Ok(release("v0.3.23")));
    assert_eq!(
        restarted.snapshot().latest_version.as_deref(),
        Some("0.3.22")
    );
    restarted.panel_open(100 + CHECK_INTERVAL_SECS - 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (upgraded, _) = service(Some(path), "0.3.22", || Ok(release("v0.3.23")));
    assert_eq!(upgraded.snapshot(), UpdateSnapshot::default());
}

#[test]
fn failed_and_timed_out_attempts_are_persisted_and_keep_the_last_known_update() {
    for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        save_cache(
            &path,
            &Cache {
                last_attempt_at: Some(0),
                latest_tag: Some("v0.3.22".into()),
            },
        )
        .unwrap();
        let (failed, calls) = service(Some(path.clone()), "0.3.21", move || {
            Err(io::Error::new(kind, "Fake network failure"))
        });
        failed.panel_open(CHECK_INTERVAL_SECS);
        wait_idle(&failed);
        failed.panel_open(CHECK_INTERVAL_SECS + 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(failed.snapshot().latest_version.as_deref(), Some("0.3.22"));
        drop(failed);

        let (restarted, calls) = service(Some(path), "0.3.21", || Ok(release("v0.3.23")));
        restarted.panel_open(2 * CHECK_INTERVAL_SECS - 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            restarted.snapshot().latest_version.as_deref(),
            Some("0.3.22")
        );
        restarted.panel_open(2 * CHECK_INTERVAL_SECS);
        wait_idle(&restarted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            restarted.snapshot().latest_version.as_deref(),
            Some("0.3.23")
        );
    }
}

#[test]
fn attempt_is_persisted_before_fetch_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    let read_path = path.clone();
    let (service, _) = service(Some(path), "0.3.21", move || {
        assert_eq!(load_cache(&read_path).last_attempt_at, Some(123));
        Ok(release("v0.3.22"))
    });
    service.panel_open(123);
    wait_idle(&service);
    assert_eq!(service.snapshot().latest_version.as_deref(), Some("0.3.22"));
}

#[test]
fn first_failure_is_throttled_across_restart_without_a_known_release() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    let (failed, _) = service(Some(path.clone()), "0.3.21", || {
        Err(io::Error::other("Rate limited"))
    });
    failed.panel_open(100);
    wait_idle(&failed);
    drop(failed);
    let (restarted, calls) = service(Some(path), "0.3.21", || Ok(release("v0.3.22")));
    restarted.panel_open(101);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(restarted.snapshot(), UpdateSnapshot::default());
}

#[test]
fn versions_use_semantic_precedence_and_urls_use_only_validated_tags() {
    for (current, tag, expected) in [
        ("0.3.21", "v0.3.9", None),
        ("0.3.21", "v0.3.21", None),
        ("0.3.21", "v0.3.21+build.2", None),
        ("0.3.21+local", "0.3.21", None),
        ("0.3.21", "v0.3.22", Some("0.3.22")),
        ("0.3.21", "0.3.100", Some("0.3.100")),
        ("0.3.21", "v0.10.0", Some("0.10.0")),
        ("0.3.21", "v1.0.0+build.1", Some("1.0.0+build.1")),
    ] {
        let (service, _) = service(None, current, move || Ok(release(tag)));
        service.panel_open(0);
        wait_idle(&service);
        let snapshot = service.snapshot();
        assert_eq!(
            snapshot.latest_version.as_deref(),
            expected,
            "{current} / {tag}"
        );
        assert_eq!(
            snapshot.release_url,
            expected.map(|_| format!("{RELEASE_URL_PREFIX}{tag}"))
        );
    }
}

#[test]
fn unstable_or_malformed_releases_are_ignored() {
    for body in [
        release("v0.4.0-rc.1"),
        release("v0.4"),
        release("v01.4.0"),
        release("v0.4.0/path"),
        release("v0.4.0?redirect=elsewhere"),
        release("https://untrusted.example/0.4.0"),
        br#"{"tag_name":"v0.4.0","draft":true,"prerelease":false}"#.to_vec(),
        br#"{"tag_name":"v0.4.0","draft":false,"prerelease":true}"#.to_vec(),
        br#"{"tag_name":"v0.4.0"}"#.to_vec(),
        br#"{"message":"rate limited"}"#.to_vec(),
        b"not json".to_vec(),
    ] {
        let (service, calls) = service(None, "0.3.21", move || Ok(body.clone()));
        service.panel_open(0);
        wait_idle(&service);
        service.panel_open(1);
        assert_eq!(service.snapshot(), UpdateSnapshot::default());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn malformed_cache_is_safe_and_does_not_prevent_a_check() {
    for bytes in [
        b"{".to_vec(),
        b"null".to_vec(),
        br#"{"lastAttemptAt":"invalid","latestTag":42}"#.to_vec(),
        vec![b' '; MAX_CACHE_BYTES as usize + 1],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        fs::write(&path, bytes).unwrap();
        let (service, calls) = service(Some(path), "0.3.21", || Ok(release("v0.3.22")));
        assert_eq!(service.snapshot(), UpdateSnapshot::default());
        service.panel_open(100);
        wait_idle(&service);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.snapshot().latest_version.as_deref(), Some("0.3.22"));
    }
}

#[test]
fn invalid_cached_tags_cannot_advertise_links_but_keep_the_attempt_time() {
    for tag in [
        "v0.4.0-rc.1",
        "../../elsewhere",
        "https://untrusted.example/",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        save_cache(
            &path,
            &Cache {
                last_attempt_at: Some(100),
                latest_tag: Some(tag.into()),
            },
        )
        .unwrap();
        let (service, calls) = service(Some(path), "0.3.21", || Ok(release("v0.3.22")));
        assert_eq!(service.snapshot(), UpdateSnapshot::default());
        service.panel_open(101);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn clock_rollback_does_not_bypass_the_daily_throttle() {
    let (service, calls) = service(None, "0.3.21", || Ok(release("v0.3.22")));
    service.panel_open(100);
    wait_idle(&service);
    for now in [99, 0, 100 + CHECK_INTERVAL_SECS - 1] {
        service.panel_open(now);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    service.panel_open(100 + CHECK_INTERVAL_SECS);
    wait_idle(&service);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn oversized_release_response_is_rejected() {
    let mut body = release("v0.3.22");
    body.resize(MAX_RESPONSE_BYTES as usize + 1, b' ');
    assert!(parse_release(&body).is_err());
}

#[test]
fn snapshot_serializes_for_both_desktop_bridges() {
    let snapshot = UpdateSnapshot {
        latest_version: Some("0.3.22".into()),
        release_url: Some(format!("{RELEASE_URL_PREFIX}v0.3.22")),
    };
    assert_eq!(
        serde_json::to_value(snapshot).unwrap(),
        serde_json::json!({
            "latestVersion": "0.3.22",
            "releaseUrl": "https://github.com/WXGopher/agent-companion/releases/tag/v0.3.22"
        })
    );
}

#[test]
fn manual_checks_bypass_daily_cache_and_report_semantic_version_results() {
    for (current, latest, newer) in [
        ("1.2.3", "v1.2.3", false),
        ("1.2.4", "v1.2.3", false),
        ("1.2.3+local", "v1.2.3+release", false),
        ("1.2.3", "v1.2.10", true),
    ] {
        let (service, calls) = service(None, current, move || Ok(release(latest)));
        service.panel_open(100);
        wait_idle(&service);
        assert_eq!(service.manual_snapshot(), ManualCheck::Idle);
        service.check_now(101);
        wait_idle(&service);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let expected = if newer {
            ManualCheck::Available(service.snapshot())
        } else {
            ManualCheck::UpToDate
        };
        assert_eq!(service.manual_snapshot(), expected);
        service.panel_open(102);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        service.check_now(102);
        wait_idle(&service);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(service.manual_snapshot(), expected);
    }
}

#[test]
fn manual_failure_never_presents_cached_success_and_allows_immediate_retry() {
    for error in [
        io::ErrorKind::TimedOut,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::InvalidData,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        save_cache(
            &path,
            &Cache {
                last_attempt_at: Some(100),
                latest_tag: Some("v1.2.9".into()),
            },
        )
        .unwrap();
        let retry = AtomicUsize::new(0);
        let (service, calls) = service(Some(path), "1.2.3", move || {
            if retry.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(io::Error::new(error, "Synthetic private diagnostic"))
            } else {
                Ok(release("v1.2.10"))
            }
        });
        assert_eq!(service.manual_snapshot(), ManualCheck::Idle);
        service.check_now(101);
        wait_idle(&service);
        assert_eq!(service.manual_snapshot(), ManualCheck::Failed);
        assert_eq!(service.snapshot().latest_version.as_deref(), Some("1.2.9"));
        service.check_now(101);
        wait_idle(&service);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            service.manual_snapshot(),
            ManualCheck::Available(service.snapshot())
        );
        assert_eq!(service.snapshot().latest_version.as_deref(), Some("1.2.10"));
    }
}

#[test]
fn manual_checks_join_an_in_flight_request_without_blocking_or_refetching() {
    for background_first in [false, true] {
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let finish_rx = Mutex::new(finish_rx);
        let (service, calls) = service(None, "1.2.3", move || {
            started_tx.send(()).unwrap();
            finish_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            Ok(release("v1.2.4"))
        });
        if background_first {
            service.panel_open(100);
        } else {
            service.check_now(100);
        }
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        for _ in 0..20 {
            service.check_now(101);
            service.panel_open(100 + CHECK_INTERVAL_SECS);
            assert_eq!(service.manual_snapshot(), ManualCheck::Checking);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        finish_tx.send(()).unwrap();
        wait_idle(&service);
        assert_eq!(
            service.manual_snapshot(),
            ManualCheck::Available(service.snapshot())
        );
    }
}

#[test]
fn background_checks_do_not_replace_the_last_manual_feedback() {
    let attempts = AtomicUsize::new(0);
    let service = UpdateService::fixture("1.2.3", move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(release("v1.2.3"))
        } else {
            Ok(release("v1.2.4"))
        }
    });
    service.check_now(0);
    wait_idle(&service);
    assert_eq!(service.manual_snapshot(), ManualCheck::UpToDate);
    service.panel_open(CHECK_INTERVAL_SECS);
    wait_idle(&service);
    assert!(service.snapshot().release_url.is_some());
    assert_eq!(service.manual_snapshot(), ManualCheck::UpToDate);
}

#[test]
fn manual_checks_report_invalid_or_unstable_responses_as_failure() {
    for body in [
        release("v2.0.0-rc.1"),
        release("v2.0.0/elsewhere"),
        br#"{"tag_name":"v2.0.0","draft":true,"prerelease":false}"#.to_vec(),
        br#"{"tag_name":"v2.0.0","draft":false,"prerelease":true}"#.to_vec(),
        br#"{"message":"rate limited"}"#.to_vec(),
    ] {
        let service = UpdateService::fixture("1.2.3", move || Ok(body.clone()));
        service.check_now(0);
        wait_idle(&service);
        assert_eq!(service.manual_snapshot(), ManualCheck::Failed);
        assert!(service.snapshot().release_url.is_none());
    }
}
