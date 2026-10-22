//! Synthetic monitor snapshots exercise selection without changing the user's
//! displays. The native smoke test only reads the current shell.

use super::*;

const BAR: Rect = Rect {
    left: 0,
    top: 1040,
    right: 1920,
    bottom: 1080,
};

fn candidate(handle: isize, class: TaskbarClass, is_primary: Option<bool>) -> TaskbarCandidate {
    TaskbarCandidate {
        handle,
        class,
        rect: Some(BAR),
        monitor: Some(handle + 100),
        is_primary,
    }
}

#[test]
fn taskbar_selection_uses_the_primary_monitor_regardless_of_class_or_enumeration_order() {
    for primary_class in [TaskbarClass::Main, TaskbarClass::Secondary] {
        let secondary_class = match primary_class {
            TaskbarClass::Main => TaskbarClass::Secondary,
            TaskbarClass::Secondary => TaskbarClass::Main,
        };
        let primary = candidate(2, primary_class, Some(true));
        let secondary = candidate(1, secondary_class, Some(false));
        assert_eq!(select_taskbar(&[secondary, primary], None), Some((2, BAR)));
        assert_eq!(select_taskbar(&[primary, secondary], None), Some((2, BAR)));
    }
}

#[test]
fn taskbar_selection_follows_a_primary_display_change_and_a_recreated_shell() {
    let mut bars = [
        candidate(1, TaskbarClass::Main, Some(true)),
        candidate(2, TaskbarClass::Secondary, Some(false)),
    ];
    assert_eq!(select_taskbar(&bars, None), Some((1, BAR)));

    // Windows changes the primary monitor while the taskbar handles survive.
    bars[0].is_primary = Some(false);
    bars[1].is_primary = Some(true);
    assert_eq!(select_taskbar(&bars, None), Some((2, BAR)));

    // Explorer restarts: no cached handle should influence the next lookup.
    assert_eq!(select_taskbar(&[], None), None);
    bars[1].handle = 3;
    assert_eq!(select_taskbar(&bars, None), Some((3, BAR)));
}

#[test]
fn taskbar_selection_preserves_negative_coordinates_and_thin_auto_hidden_bars() {
    let negative = Rect {
        left: -1920,
        top: -40,
        right: 0,
        bottom: -1,
    };
    let primary = TaskbarCandidate {
        rect: Some(negative),
        ..candidate(2, TaskbarClass::Secondary, Some(true))
    };
    let secondary = candidate(1, TaskbarClass::Main, Some(false));
    assert_eq!(
        select_taskbar(&[secondary, primary], None),
        Some((2, negative))
    );

    let thin = Rect {
        left: -2,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    let hidden = TaskbarCandidate {
        rect: Some(thin),
        ..primary
    };
    assert_eq!(select_taskbar(&[hidden], None), Some((2, thin)));
}

#[test]
fn taskbar_selection_ignores_unavailable_handles_and_invalid_rectangles() {
    let primary = candidate(9, TaskbarClass::Secondary, Some(true));
    let mut candidates = vec![TaskbarCandidate {
        handle: 0,
        ..primary
    }];
    for rect in [
        None,
        Some(Rect { right: 0, ..BAR }),
        Some(Rect {
            bottom: BAR.top,
            ..BAR
        }),
        Some(Rect { right: -1, ..BAR }),
        Some(Rect {
            bottom: BAR.top - 1,
            ..BAR
        }),
    ] {
        candidates.push(TaskbarCandidate { rect, ..primary });
    }
    assert_eq!(select_taskbar(&candidates, None), None);
    candidates.push(primary);
    assert_eq!(select_taskbar(&candidates, None), Some((9, BAR)));
}

#[test]
fn taskbar_selection_falls_back_to_the_main_class_only_when_its_monitor_is_unknown() {
    let fallback = candidate(1, TaskbarClass::Main, None);
    let unknown_secondary = candidate(2, TaskbarClass::Secondary, None);
    let primary = candidate(3, TaskbarClass::Secondary, Some(true));
    assert_eq!(select_taskbar(&[unknown_secondary], None), None);
    assert_eq!(
        select_taskbar(&[unknown_secondary, fallback], None),
        Some((1, BAR))
    );
    assert_eq!(select_taskbar(&[fallback, primary], None), Some((3, BAR)));
    assert_eq!(select_taskbar(&[primary, fallback], None), Some((3, BAR)));

    let known_secondary = TaskbarCandidate {
        is_primary: Some(false),
        ..fallback
    };
    let unavailable_primary = TaskbarCandidate {
        rect: None,
        ..primary
    };
    assert_eq!(
        select_taskbar(
            &[known_secondary, unknown_secondary, unavailable_primary],
            None
        ),
        None,
        "a missing primary taskbar must not send the readout to another display"
    );
}

fn display(id: &str, monitor: isize, name: &str, primary: bool) -> ConnectedDisplay {
    ConnectedDisplay {
        monitor,
        display: TaskbarDisplay {
            id: id.to_owned(),
            name: name.to_owned(),
            primary,
            has_taskbar: false,
        },
    }
}

const PRIMARY_ID: &str = r"\\?\DISPLAY#ACM0001#PRIMARY#{monitor}";
const CHOSEN_ID: &str = r"\\?\DISPLAY#ACM0002#CHOSEN#{monitor}";

#[test]
fn fixed_display_selection_wins_over_primary_in_either_order() {
    let displays = [
        display(PRIMARY_ID, 101, "Display 1", true),
        display(CHOSEN_ID, 102, "Display 2", false),
    ];
    let requested = requested_monitor(&displays, Some(&CHOSEN_ID.to_ascii_lowercase()));
    let primary = candidate(1, TaskbarClass::Main, Some(true));
    let chosen = candidate(2, TaskbarClass::Secondary, Some(false));
    assert_eq!(
        requested,
        Some(102),
        "interface identity is case-insensitive"
    );
    assert_eq!(
        select_taskbar(&[primary, chosen], requested),
        Some((2, BAR))
    );
    assert_eq!(
        select_taskbar(&[chosen, primary], requested),
        Some((2, BAR))
    );
}

#[test]
fn a_remembered_display_survives_number_monitor_handle_and_shell_handle_changes() {
    let initial = [display(CHOSEN_ID, 102, "Display 2", false)];
    let old = candidate(2, TaskbarClass::Secondary, Some(false));
    assert_eq!(
        select_taskbar(&[old], requested_monitor(&initial, Some(CHOSEN_ID))),
        Some((2, BAR))
    );

    // Reconnection changes the desktop number and native monitor/taskbar
    // handles. The old native monitor handle now belongs to another display.
    let reconnected = [
        display(PRIMARY_ID, 102, "Display 2", true),
        display(CHOSEN_ID, 400, "Display 1", false),
    ];
    let replacement = TaskbarCandidate {
        monitor: Some(400),
        ..candidate(8, TaskbarClass::Main, Some(false))
    };
    let reused = TaskbarCandidate {
        is_primary: Some(true),
        ..old
    };
    assert_eq!(
        select_taskbar(
            &[reused, replacement],
            requested_monitor(&reconnected, Some(CHOSEN_ID))
        ),
        Some((8, BAR))
    );
    // Changing Windows' primary flag does not change a fixed choice either.
    let replacement = TaskbarCandidate {
        is_primary: Some(true),
        ..replacement
    };
    assert_eq!(
        select_taskbar(
            &[replacement, reused],
            requested_monitor(&reconnected, Some(CHOSEN_ID))
        ),
        Some((8, BAR))
    );
}

#[test]
fn unavailable_fixed_display_falls_back_to_primary_and_returns_when_its_taskbar_returns() {
    let displays = [
        display(PRIMARY_ID, 101, "Display 1", true),
        display(CHOSEN_ID, 102, "Display 2", false),
    ];
    let primary = candidate(1, TaskbarClass::Secondary, Some(true));
    let chosen = candidate(2, TaskbarClass::Main, Some(false));
    let requested = requested_monitor(&displays, Some(CHOSEN_ID));
    assert_eq!(
        select_taskbar(&[primary, chosen], requested),
        Some((2, BAR))
    );
    assert_eq!(select_taskbar(&[primary], requested), Some((1, BAR)));
    for rect in [None, Some(Rect { right: 0, ..BAR })] {
        let unavailable = TaskbarCandidate { rect, ..chosen };
        assert_eq!(
            select_taskbar(&[unavailable, primary], requested),
            Some((1, BAR))
        );
    }
    let disconnected = &displays[..1];
    assert_eq!(
        select_taskbar(&[primary], requested_monitor(disconnected, Some(CHOSEN_ID))),
        Some((1, BAR))
    );
    assert_eq!(
        select_taskbar(
            &[chosen, primary],
            requested_monitor(&displays, Some(CHOSEN_ID))
        ),
        Some((2, BAR))
    );
    assert_eq!(
        select_taskbar(
            &[primary, chosen],
            requested_monitor(&displays, Some("unknown saved display"))
        ),
        Some((1, BAR))
    );
    assert_eq!(select_taskbar(&[], requested), None);
}

#[test]
fn display_inventory_includes_missing_taskbars_and_clones_without_duplicate_ids() {
    let displays = vec![
        display(CHOSEN_ID, 102, "Display 2", false),
        display(PRIMARY_ID, 101, "Display 1", true),
        display(&CHOSEN_ID.to_ascii_lowercase(), 102, "duplicate", false),
        display("cloned monitor identity", 102, "Display 2.2", false),
        display("display without taskbar", 103, "Display 3", false),
    ];
    let chosen = candidate(2, TaskbarClass::Main, Some(false));
    let invalid = TaskbarCandidate {
        rect: None,
        ..candidate(3, TaskbarClass::Secondary, Some(false))
    };
    let inventory = describe_displays(displays, &[chosen, invalid]);
    assert_eq!(inventory.len(), 4);
    assert_eq!(inventory[0].id, PRIMARY_ID);
    assert!(inventory[0].primary);
    assert!(!inventory[0].has_taskbar);
    assert!(
        inventory
            .iter()
            .find(|item| item.id == CHOSEN_ID)
            .unwrap()
            .has_taskbar
    );
    assert!(
        inventory
            .iter()
            .find(|item| item.name == "Display 2.2")
            .unwrap()
            .has_taskbar
    );
    assert!(
        !inventory
            .iter()
            .find(|item| item.name == "Display 3")
            .unwrap()
            .has_taskbar
    );
}

#[test]
fn native_taskbar_lookup_reads_the_primary_display_without_modifying_the_shell() {
    let Some(bar) = taskbar() else {
        // A headless Windows runner or a restarting shell may have no taskbar.
        eprintln!("No recognized primary taskbar is currently available; native check skipped.");
        return;
    };
    assert!(taskbar_class(hwnd(bar.handle)).is_some());
    assert!(bar.rect.right > bar.rect.left && bar.rect.bottom > bar.rect.top);

    // Windows defines the primary monitor's origin as (0, 0). Check discovery
    // independently of the primary-flag helper used to select the shell bar.
    let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST) };
    let selected = unsafe { MonitorFromWindow(hwnd(bar.handle), MONITOR_DEFAULTTONEAREST) };
    assert!(!primary.is_invalid());
    assert_eq!(selected, primary);
    eprintln!("Read-only primary taskbar check: {bar:?}");
}

#[test]
fn native_display_inventory_can_target_each_available_taskbar_without_moving_windows() {
    let displays = taskbar_displays();
    if displays.is_empty() {
        eprintln!(
            "No identifiable desktop monitors are available; native inventory check skipped."
        );
        return;
    }
    let connected = connected_displays();
    for display in &displays {
        assert!(!display.id.is_empty());
        assert!(!display.name.is_empty());
        assert!(
            !display.name.contains(&display.id),
            "UI labels must not expose device paths"
        );
        if display.has_taskbar {
            let bar = taskbar_on_display(Some(&display.id)).expect("display has a taskbar");
            let selected_monitor =
                unsafe { MonitorFromWindow(hwnd(bar.handle), MONITOR_DEFAULTTONEAREST) };
            assert_eq!(
                Some(selected_monitor.0 as isize),
                requested_monitor(&connected, Some(&display.id))
            );
            eprintln!("Read-only fixed display check: {} -> {bar:?}", display.name);
        }
    }
    if let Some(primary) = taskbar() {
        assert_eq!(
            taskbar_on_display(Some("not-a-connected-monitor-identity")).map(|bar| bar.handle),
            Some(primary.handle)
        );
    }
    eprintln!("Read-only display inventory: {displays:?}");
}
