use super::*;
use std::cell::{Cell, RefCell};

struct Fake {
    versions: RefCell<[Option<Version>; 4]>,
    calls: RefCell<Vec<String>>,
    unreadable: Option<Target>,
    offline: bool,
    running: bool,
    primary_running: bool,
    secondary_running: [bool; 2],
    fail_align_cli: Cell<bool>,
    fail_align_app: bool,
    unsupported: bool,
    app_needs_sync: bool,
    cli_needs_sync: bool,
    updater_cli: Option<Version>,
    newer_release: bool,
    store_app: Option<Version>,
    bundled_tui: bool,
}
fn cli(value: &str) -> Version {
    Version {
        version: value.into(),
        build: None,
    }
}
fn app(build: &str) -> Version {
    Version {
        version: format!("26.9.{build}"),
        build: Some(build.into()),
    }
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            versions: RefCell::new([
                Some(cli("0.159.3")),
                Some(cli("0.155.0-alpha.16.4")),
                Some(app("100")),
                Some(app("90")),
            ]),
            calls: RefCell::new(Vec::new()),
            unreadable: None,
            offline: false,
            running: false,
            primary_running: false,
            secondary_running: [false; 2],
            fail_align_cli: Cell::new(false),
            fail_align_app: false,
            unsupported: false,
            app_needs_sync: false,
            cli_needs_sync: false,
            updater_cli: None,
            newer_release: false,
            store_app: None,
            bundled_tui: false,
        }
    }
}
impl Operations for Fake {
    fn local_source_note(&self, target: Target) -> Option<&'static str> {
        (target == Target::CodexTui && self.bundled_tui).then_some("参考来自 Codex App 内置 CLI。")
    }
    fn always_update_primary(&self, app: bool) -> bool {
        app && self.store_app.is_some()
    }

    fn app_needs_sync(&self) -> bool {
        self.app_needs_sync && !self.calls.borrow().iter().any(|call| call == "align-true")
    }
    fn cli_needs_sync(&self) -> bool {
        self.cli_needs_sync && !self.calls.borrow().iter().any(|call| call == "align-false")
    }
    fn installed(&self, target: Target) -> Result<Option<Version>, String> {
        if self.unreadable == Some(target) {
            return Err("无法读取版本元数据".into());
        }
        Ok(self.versions.borrow()[target.index()].clone())
    }
    fn latest(&self, app: bool) -> Result<Release, String> {
        self.calls.borrow_mut().push(format!("fetch-{app}"));
        if self.offline {
            return Err("offline: latest unknown".into());
        }
        Ok(Release {
            version: if app {
                if self.store_app.is_some() {
                    self.versions.borrow()[2].clone().unwrap()
                } else {
                    self::app("110")
                }
            } else {
                cli(
                    if self.newer_release
                        && self
                            .calls
                            .borrow()
                            .iter()
                            .filter(|call| *call == "fetch-false")
                            .count()
                            > 1
                    {
                        "0.161.0"
                    } else {
                        "0.160.0"
                    },
                )
            },
            url: String::new(),
            length: 0,
        })
    }
    fn preflight(&self, _: bool, _: bool) -> Result<(), String> {
        if self.unsupported {
            Err("unsupported full package".into())
        } else {
            Ok(())
        }
    }
    fn require_stopped(&self) -> Result<(), String> {
        if self.running
            || self.primary_running
            || self.secondary_running.iter().any(|running| *running)
        {
            Err("quit running TUI first".into())
        } else {
            Ok(())
        }
    }
    fn require_secondary_stopped(&self, cli: bool, app: bool) -> Result<(), String> {
        if self.running || (cli && self.secondary_running[0]) || (app && self.secondary_running[1])
        {
            Err("quit affected Dodex first".into())
        } else {
            Ok(())
        }
    }
    fn update_primary(&self, app: bool, release: &Release) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("update-{app}"));
        self.versions.borrow_mut()[if app { 2 } else { 0 }] = Some(if !app {
            self.updater_cli
                .clone()
                .unwrap_or_else(|| release.version.clone())
        } else {
            self.store_app
                .clone()
                .unwrap_or_else(|| release.version.clone())
        });
        Ok(())
    }
    fn align_secondary(&self, app: bool, expected: &Version) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("align-{app}"));
        if app && self.fail_align_app {
            return Err("app mirror failed".into());
        }
        if !app && self.fail_align_cli.get() {
            return Err("terminal publication failed".into());
        }
        self.versions.borrow_mut()[if app { 3 } else { 1 }] = Some(expected.clone());
        Ok(())
    }
}
fn run(fake: &Fake, action: Option<Action>) -> (Snapshot, Result<(), String>) {
    let mut snapshot = Snapshot::default();
    let result = execute(fake, action, |change| change(&mut snapshot));
    (snapshot, result)
}

#[test]
fn observing_and_alignment_do_not_contact_upstream_and_preserve_primary() {
    let fake = Fake::default();
    let original = fake.versions.borrow()[0].clone();
    assert!(run(&fake, None).1.is_ok());
    assert!(fake.calls.borrow().is_empty());
    let (snapshot, result) = run(&fake, Some(Action::Align));
    result.unwrap();
    assert_eq!(*fake.calls.borrow(), ["align-true", "align-false"]);
    assert_eq!(fake.versions.borrow()[0], original);
    assert_eq!(snapshot.rows[0].current, snapshot.rows[1].current);
    assert_eq!(snapshot.rows[2].current, snapshot.rows[3].current);
}

#[test]
fn opening_settings_automatically_compares_each_local_pair_without_maintenance() {
    let fake = Fake {
        offline: true,
        running: true,
        unsupported: true,
        ..Fake::default()
    };
    let original = fake.versions.borrow().clone();
    let (snapshot, result) = run(&fake, None);
    result.unwrap();
    assert_eq!(snapshot.rows[0].target, "0.159.3");
    assert_eq!(snapshot.rows[1].target, "0.159.3");
    assert_eq!(snapshot.rows[2].target, "26.9.100 (100)");
    assert_eq!(snapshot.rows[3].target, "26.9.100 (100)");
    for index in [0, 2] {
        assert!(snapshot.rows[index].message.contains("本机对齐参考"));
    }
    for index in [1, 3] {
        assert!(snapshot.rows[index].message.contains("可对齐"));
    }
    assert!(fake.calls.borrow().is_empty());
    assert_eq!(*fake.versions.borrow(), original);
}

#[test]
fn matching_bundled_alpha_reference_is_visible_and_alignment_is_a_read_only_noop() {
    let fake = Fake {
        bundled_tui: true,
        running: true,
        unsupported: true,
        ..Fake::default()
    };
    for index in [0, 1] {
        fake.versions.borrow_mut()[index] = Some(cli("1.2.3-alpha.2"));
    }
    fake.versions.borrow_mut()[3] = Some(app("100"));
    let original = fake.versions.borrow().clone();
    for action in [None, Some(Action::Align)] {
        let (snapshot, result) = run(&fake, action);
        result.unwrap();
        for row in &snapshot.rows[..2] {
            assert_eq!(row.current, "1.2.3-alpha.2");
            assert_eq!(row.target, "1.2.3-alpha.2");
            assert!(!row.error);
        }
        assert!(snapshot.rows[0].message.contains("App 内置 CLI"));
        assert!(!snapshot.error);
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(*fake.versions.borrow(), original);
    }
}

#[test]
fn local_comparison_reports_matching_and_newer_secondary_versions_without_downgrading() {
    for (tui, desktop, expected) in [
        ("0.159.3", "100", "版本一致"),
        ("0.160.1", "101", "较新，保留，不降级"),
    ] {
        let fake = Fake::default();
        fake.versions.borrow_mut()[1] = Some(cli(tui));
        fake.versions.borrow_mut()[3] = Some(app(desktop));
        let original = fake.versions.borrow().clone();
        let (snapshot, result) = run(&fake, None);
        result.unwrap();
        for index in [1, 3] {
            assert!(
                snapshot.rows[index].message.contains(expected),
                "{:?}",
                snapshot.rows[index]
            );
            assert!(!snapshot.rows[index].error);
        }
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(*fake.versions.borrow(), original);
    }
}

#[test]
fn local_comparison_explicitly_reports_missing_secondary_and_missing_reference() {
    let fake = Fake::default();
    fake.versions.borrow_mut()[1] = None;
    fake.versions.borrow_mut()[3] = None;
    let (snapshot, result) = run(&fake, None);
    result.unwrap();
    for index in [1, 3] {
        assert!(snapshot.rows[index].message.contains("未找到 Dodex"));
        assert!(snapshot.rows[index].message.contains("安装并配置 Dodex"));
        assert_ne!(snapshot.rows[index].target, "尚未检查");
    }
    assert!(snapshot.message.contains("安装并配置 Dodex"));

    let fake = Fake::default();
    fake.versions.borrow_mut()[0] = None;
    fake.versions.borrow_mut()[2] = None;
    let (snapshot, result) = run(&fake, None);
    result.unwrap();
    for index in [0, 2] {
        assert!(snapshot.rows[index].message.contains("未找到本机 Codex"));
    }
    for index in [1, 3] {
        assert!(snapshot.rows[index].message.contains("无法比较"));
        assert_eq!(snapshot.rows[index].target, "缺少本机参考");
    }
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn local_comparison_preserves_read_errors_and_does_not_call_them_missing_installations() {
    let fake = Fake {
        unreadable: Some(Target::DodexTui),
        ..Fake::default()
    };
    let (snapshot, result) = run(&fake, None);
    result.unwrap();
    assert!(snapshot.error && snapshot.rows[1].error);
    assert_eq!(snapshot.rows[1].target, "0.159.3");
    assert_eq!(snapshot.rows[1].message, "无法读取版本元数据");
    assert!(!snapshot.rows[1].message.contains("未找到"));
    assert!(snapshot.rows[3].message.contains("可对齐"));
    assert!(fake.calls.borrow().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn fresh_setup_completes_both_components_before_monitoring_and_is_idempotent() {
    let fake = Fake {
        primary_running: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[1] = None;
    fake.versions.borrow_mut()[3] = None;
    let primary = [
        fake.versions.borrow()[0].clone(),
        fake.versions.borrow()[2].clone(),
    ];
    let complete = || {
        assert_eq!(fake.versions.borrow()[0], fake.versions.borrow()[1]);
        assert_eq!(fake.versions.borrow()[2], fake.versions.borrow()[3]);
        Ok(())
    };
    install_local(&fake, |_| {}, complete).unwrap();
    assert_eq!(*fake.calls.borrow(), ["align-true", "align-false"]);
    install_local(&fake, |_| {}, complete).unwrap();
    assert_eq!(*fake.calls.borrow(), ["align-true", "align-false"]);
    assert_eq!(
        [
            fake.versions.borrow()[0].clone(),
            fake.versions.borrow()[2].clone()
        ],
        primary
    );
}

#[cfg(target_os = "macos")]
#[test]
fn setup_resumes_after_app_success_without_publishing_monitoring_on_tui_failure() {
    let fake = Fake {
        fail_align_cli: Cell::new(true),
        ..Fake::default()
    };
    fake.versions.borrow_mut()[1] = None;
    fake.versions.borrow_mut()[3] = None;
    let enabled = Cell::new(false);
    let complete = || {
        enabled.set(true);
        Ok(())
    };
    assert!(install_local(&fake, |_| {}, complete).is_err());
    assert!(!enabled.get());
    assert_eq!(fake.versions.borrow()[2], fake.versions.borrow()[3]);
    assert!(fake.versions.borrow()[1].is_none());
    fake.fail_align_cli.set(false);
    install_local(&fake, |_| {}, complete).unwrap();
    assert!(enabled.get());
    assert_eq!(
        *fake.calls.borrow(),
        ["align-true", "align-false", "align-false"]
    );
}

#[test]
fn alignment_allows_primary_and_untouched_secondary_but_stops_affected_dodex() {
    let fake = Fake {
        primary_running: true,
        secondary_running: [false, true],
        ..Fake::default()
    };
    let current_app = fake.versions.borrow()[2].clone();
    fake.versions.borrow_mut()[3] = current_app;
    run(&fake, Some(Action::Align)).1.unwrap();
    assert_eq!(*fake.calls.borrow(), ["align-false"]);
    assert!(run(&fake, Some(Action::UpdateAll)).1.is_err());
    let fake = Fake {
        secondary_running: [true, false],
        ..Fake::default()
    };
    let original = fake.versions.borrow().clone();
    assert!(run(&fake, Some(Action::Align)).1.is_err());
    assert_eq!(*fake.versions.borrow(), original);
    assert!(fake.calls.borrow().is_empty());
}
#[test]
fn alignment_never_downgrades_newer_dodex() {
    let fake = Fake::default();
    fake.versions.borrow_mut()[1] = Some(cli("0.160.0-alpha.1"));
    fake.versions.borrow_mut()[3] = Some(app("101"));
    let (snapshot, result) = run(&fake, Some(Action::Align));
    result.unwrap();
    assert!(fake.calls.borrow().is_empty());
    assert!(snapshot.message.contains("未降级"));
    assert!(!snapshot.message.contains("已分别对齐"));
}
#[test]
fn all_updates_both_primary_components_before_verifying_corresponding_secondary() {
    let fake = Fake::default();
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    result.unwrap();
    assert_eq!(
        *fake.calls.borrow(),
        [
            "fetch-false",
            "fetch-true",
            "update-true",
            "align-true",
            "update-false",
            "align-false"
        ]
    );
    assert_eq!(snapshot.rows[0].current, "0.160.0");
    assert_eq!(snapshot.rows[1].current, "0.160.0");
    assert_eq!(snapshot.rows[2].current, "26.9.110 (110)");
    assert_eq!(snapshot.rows[3].current, "26.9.110 (110)");
}
#[test]
fn offline_running_and_unsupported_preflight_never_mutate_either_instance() {
    for fake in [
        Fake {
            offline: true,
            ..Fake::default()
        },
        Fake {
            running: true,
            ..Fake::default()
        },
        Fake {
            unsupported: true,
            ..Fake::default()
        },
    ] {
        let original = fake.versions.borrow().clone();
        assert!(run(&fake, Some(Action::UpdateAll)).1.is_err());
        assert_eq!(*fake.versions.borrow(), original);
        assert!(
            fake.calls
                .borrow()
                .iter()
                .all(|call| call.starts_with("fetch"))
        );
    }
}
#[test]
fn partial_failure_reports_actual_versions_without_claiming_both_updated() {
    let fake = Fake {
        fail_align_app: true,
        ..Fake::default()
    };
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    assert!(result.unwrap_err().contains("未全部完成"));
    assert_eq!(snapshot.rows[2].current, "26.9.110 (110)");
    assert_eq!(snapshot.rows[3].current, "26.9.90 (90)");
    assert_ne!(snapshot.rows[3].current, snapshot.rows[3].target);
    assert_eq!(snapshot.rows[0].current, "0.159.3");
    assert_eq!(snapshot.rows[1].current, "0.155.0-alpha.16.4");
    assert_eq!(
        *fake.calls.borrow(),
        ["fetch-false", "fetch-true", "update-true", "align-true"]
    );
    assert!(!snapshot.message.contains("已检查官方稳定版"));
}

#[test]
fn newer_dodex_app_with_old_bootstrap_blocks_tui_migration_before_any_changes() {
    let fake = Fake {
        app_needs_sync: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[3] = Some(app("120"));
    let original = fake.versions.borrow().clone();
    for action in [Action::Align, Action::UpdateAll] {
        assert!(
            run(&fake, Some(action))
                .1
                .unwrap_err()
                .contains("启动器需要更新")
        );
        assert_eq!(*fake.versions.borrow(), original);
        assert!(
            fake.calls
                .borrow()
                .iter()
                .all(|call| call.starts_with("fetch"))
        );
    }
}
#[test]
fn startup_request_waits_for_inspection_and_repeated_actions_are_not_replayed() {
    let mut requests = Requests {
        pending: Some(Action::UpdateAll),
        repeated: true,
    };
    let mut state = Snapshot {
        busy: true,
        initializing: true,
        ..Snapshot::default()
    };
    assert_eq!(queued_action(&mut requests, &mut state), None);
    assert_eq!(requests.pending, Some(Action::UpdateAll));
    state.initializing = false;
    state.busy = false;
    assert_eq!(
        queued_action(&mut requests, &mut state),
        Some(Action::UpdateAll)
    );
    assert!(state.notice.contains("重复请求已忽略"));
    assert_eq!(queued_action(&mut requests, &mut state), None);
    state.busy = true;
    requests.pending = Some(Action::Align);
    assert_eq!(queued_action(&mut requests, &mut state), None);
    state.busy = false;
    assert_eq!(queued_action(&mut requests, &mut state), None);
}
#[test]
fn unknown_version_comparisons_fail_without_guessing() {
    assert!(cli("unknown").compare(&cli("0.160.0")).is_err());
    assert!(app("unknown").compare(&app("100")).is_err());
    assert_eq!(app("100.0").compare(&app("100")).unwrap(), Ordering::Equal);
}

#[test]
fn already_aligned_running_instances_are_a_read_only_noop() {
    let fake = Fake {
        running: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[1] = Some(cli("0.159.3"));
    fake.versions.borrow_mut()[3] = Some(app("100"));
    run(&fake, Some(Action::Align)).1.unwrap();
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn matching_versions_still_migrate_legacy_terminal_entry() {
    let fake = Fake {
        cli_needs_sync: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[1] = Some(cli("0.159.3"));
    fake.versions.borrow_mut()[3] = Some(app("100"));
    run(&fake, Some(Action::Align)).1.unwrap();
    assert_eq!(*fake.calls.borrow(), ["align-false"]);
}

#[test]
fn updater_can_advance_to_a_newly_confirmed_stable_release() {
    let fake = Fake {
        updater_cli: Some(cli("0.161.0")),
        newer_release: true,
        ..Fake::default()
    };
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    result.unwrap();
    assert_eq!(snapshot.rows[0].current, "0.161.0");
    assert_eq!(snapshot.rows[1].current, "0.161.0");
    assert_eq!(snapshot.rows[0].target, "0.161.0");
    assert_eq!(snapshot.rows[1].target, "0.161.0");
}

#[test]
fn unconfirmed_updater_version_reports_partial_completion_and_preserves_secondary() {
    let fake = Fake {
        updater_cli: Some(cli("0.161.0")),
        ..Fake::default()
    };
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    assert!(result.unwrap_err().contains("未全部完成"));
    assert_eq!(snapshot.rows[0].current, "0.161.0");
    assert_eq!(snapshot.rows[1].current, "0.155.0-alpha.16.4");
    assert!(!fake.calls.borrow().iter().any(|call| call == "align-false"));
}

#[test]
fn store_version_floor_does_not_prevent_a_newer_desktop_and_bootstrap_from_converging() {
    let fake = Fake {
        store_app: Some(app("110")),
        app_needs_sync: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[3] = Some(app("105"));
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    result.unwrap();
    assert_eq!(snapshot.rows[2].current, "26.9.110 (110)");
    assert_eq!(snapshot.rows[3].current, "26.9.110 (110)");
    assert!(!snapshot.message.contains("较新"));
}

#[test]
fn updater_advance_recomputes_a_previous_newer_secondary_decision() {
    let fake = Fake {
        updater_cli: Some(cli("0.161.0")),
        newer_release: true,
        ..Fake::default()
    };
    fake.versions.borrow_mut()[1] = Some(cli("0.160.1"));
    let (snapshot, result) = run(&fake, Some(Action::UpdateAll));
    result.unwrap();
    assert_eq!(snapshot.rows[1].current, "0.161.0");
    assert!(!snapshot.message.contains("未降级"));
}
