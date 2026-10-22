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
        assert_eq!(select_taskbar(&[secondary, primary]), Some((2, BAR)));
        assert_eq!(select_taskbar(&[primary, secondary]), Some((2, BAR)));
    }
}

#[test]
fn taskbar_selection_follows_a_primary_display_change_and_a_recreated_shell() {
    let mut bars = [
        candidate(1, TaskbarClass::Main, Some(true)),
        candidate(2, TaskbarClass::Secondary, Some(false)),
    ];
    assert_eq!(select_taskbar(&bars), Some((1, BAR)));

    // Windows changes the primary monitor while the taskbar handles survive.
    bars[0].is_primary = Some(false);
    bars[1].is_primary = Some(true);
    assert_eq!(select_taskbar(&bars), Some((2, BAR)));

    // Explorer restarts: no cached handle should influence the next lookup.
    assert_eq!(select_taskbar(&[]), None);
    bars[1].handle = 3;
    assert_eq!(select_taskbar(&bars), Some((3, BAR)));
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
    assert_eq!(select_taskbar(&[secondary, primary]), Some((2, negative)));

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
    assert_eq!(select_taskbar(&[hidden]), Some((2, thin)));
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
    assert_eq!(select_taskbar(&candidates), None);
    candidates.push(primary);
    assert_eq!(select_taskbar(&candidates), Some((9, BAR)));
}

#[test]
fn taskbar_selection_falls_back_to_the_main_class_only_when_its_monitor_is_unknown() {
    let fallback = candidate(1, TaskbarClass::Main, None);
    let unknown_secondary = candidate(2, TaskbarClass::Secondary, None);
    let primary = candidate(3, TaskbarClass::Secondary, Some(true));
    assert_eq!(select_taskbar(&[unknown_secondary]), None);
    assert_eq!(
        select_taskbar(&[unknown_secondary, fallback]),
        Some((1, BAR))
    );
    assert_eq!(select_taskbar(&[fallback, primary]), Some((3, BAR)));
    assert_eq!(select_taskbar(&[primary, fallback]), Some((3, BAR)));

    let known_secondary = TaskbarCandidate {
        is_primary: Some(false),
        ..fallback
    };
    let unavailable_primary = TaskbarCandidate {
        rect: None,
        ..primary
    };
    assert_eq!(
        select_taskbar(&[known_secondary, unknown_secondary, unavailable_primary]),
        None,
        "a missing primary taskbar must not send the readout to another display"
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
