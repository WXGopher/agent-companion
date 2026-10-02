use super::*;
use std::cell::RefCell;

struct Fake {
    versions: RefCell<[Option<Version>; 4]>,
    calls: RefCell<Vec<String>>,
    offline: bool,
    running: bool,
    fail_align_app: bool,
    unsupported: bool,
    app_needs_sync: bool,
    cli_needs_sync: bool,
    updater_cli: Option<Version>,
    newer_release: bool,
    store_app: Option<Version>,
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
            offline: false,
            running: false,
            fail_align_app: false,
            unsupported: false,
            app_needs_sync: false,
            cli_needs_sync: false,
            updater_cli: None,
            newer_release: false,
            store_app: None,
        }
    }
}
impl Operations for Fake {
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
        if self.running {
            Err("quit running TUI first".into())
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
