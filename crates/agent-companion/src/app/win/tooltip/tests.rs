//! Only test-owned windows and synthetic text/desktop rectangles are used.
//! Keep native controls outside the visible desktop; no shell or account state.

use super::*;

struct TestWindow(HWND);

impl TestWindow {
    fn new(style: WINDOW_STYLE, parent: Option<HWND>) -> Self {
        Self(unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("Tooltip regression fixture"),
                style,
                -20_000,
                -20_000,
                48,
                36,
                parent,
                None,
                None,
                None,
            )
            .unwrap()
        })
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn text_of(tooltip: &UsageTooltip) -> String {
    let mut text = vec![0_u16; tooltip.text.len() + 1];
    let mut tool = tooltip.tool();
    tool.lpszText = PWSTR(text.as_mut_ptr());
    unsafe {
        SendMessageW(
            tooltip.window,
            TTM_GETTEXTW,
            Some(WPARAM(text.len())),
            Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
        );
    }
    let end = text.iter().position(|unit| *unit == 0).unwrap();
    String::from_utf16(&text[..end]).unwrap()
}

const AREA: Rect = Rect {
    left: -21_000,
    top: -21_000,
    right: -18_000,
    bottom: -19_000,
};

const ANCHOR: Rect = Rect {
    left: -21_048,
    top: -20_800,
    right: -21_000,
    bottom: -20_764,
};

const QUOTAS: &str = "Codex (C) week 73% left · Dodex (D) week 26% left\n\nCodex: Week · resets tomorrow · Last query succeeded\n\nDodex: Week · resets in 3 days · Last query succeeded";

#[test]
fn tooltip_is_a_separate_nonactivating_popup_and_owns_its_text_until_destroyed() {
    let host = TestWindow::new(WS_POPUP, None);
    let child = TestWindow::new(WS_CHILD, Some(host.0));
    let foreground = unsafe { GetForegroundWindow() };
    let initial = QUOTAS.to_owned();
    let mut tooltip = UsageTooltip::new(child.0.0 as isize, &initial).unwrap();
    drop(initial);
    let native = tooltip.window;
    let style = unsafe { GetWindowLongPtrW(native, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongPtrW(native, GWL_EXSTYLE) } as u32;
    assert_eq!(style & WS_CHILD.0, 0);
    assert_ne!(style & WS_POPUP.0, 0);
    assert_ne!(extended & WS_EX_NOACTIVATE.0, 0);
    assert_ne!(extended & WS_EX_TOOLWINDOW.0, 0);
    assert!(unsafe { GetParent(native) }.is_err());
    assert_eq!(text_of(&tooltip), QUOTAS);

    tooltip.show_in(ANCHOR, AREA, 1.0);
    let rect = rect_of(native).unwrap();
    assert!(rect.right - rect.left > 48, "popup escapes the small child");
    assert!(
        rect.bottom - rect.top > 36,
        "all paragraphs have their own surface"
    );
    assert_eq!(unsafe { GetForegroundWindow() }, foreground);

    let replacement =
        "Codex (C) week 61%* left · Dodex (D) week — left\n\nDodex: 请重新登录 & retry".to_owned();
    tooltip.set_text(&replacement);
    drop(replacement);
    assert!(text_of(&tooltip).ends_with("请重新登录 & retry"));
    tooltip.hide();
    assert!(!unsafe { IsWindowVisible(native) }.as_bool());
    tooltip.show_in(ANCHOR, AREA, 1.0);
    assert!(unsafe { IsWindowVisible(native) }.as_bool());
    tooltip.set_text("");
    assert!(!unsafe { IsWindowVisible(native) }.as_bool());
    drop(tooltip);
    assert!(!unsafe { IsWindow(Some(native)) }.as_bool());
}

#[test]
fn native_wrapping_and_dpi_measurement_fit_each_edge_of_a_synthetic_work_area() {
    let owner = TestWindow::new(WS_POPUP, None);
    let mut tooltip = UsageTooltip::new(owner.0.0 as isize, QUOTAS).unwrap();
    for scale in [1.0, 1.25, 1.5, 2.0] {
        for anchor in [
            ANCHOR,
            Rect {
                left: AREA.right,
                right: AREA.right + 48,
                ..ANCHOR
            },
            Rect {
                left: AREA.left + 100,
                right: AREA.left + 148,
                top: AREA.top - 36,
                bottom: AREA.top,
            },
            Rect {
                left: AREA.right - 100,
                right: AREA.right - 52,
                top: AREA.bottom,
                bottom: AREA.bottom + 36,
            },
        ] {
            tooltip.show_in(anchor, AREA, scale);
            let rect = rect_of(tooltip.window).unwrap();
            assert!(
                rect.left >= AREA.left && rect.right <= AREA.right,
                "{rect:?}, scale {scale}"
            );
            assert!(
                rect.top >= AREA.top && rect.bottom <= AREA.bottom,
                "{rect:?}, scale {scale}"
            );
            assert_eq!(text_of(&tooltip), QUOTAS);
            assert_eq!(tooltip.dpi, (96.0 * scale) as u32);
            assert!(tooltip.font.is_some());
        }
    }
    // A narrower work area forces more lines, not clipping to the readout.
    let narrow = Rect {
        right: AREA.left + 360,
        ..AREA
    };
    tooltip.show_in(ANCHOR, narrow, 1.0);
    let rect = rect_of(tooltip.window).unwrap();
    assert!(rect.right <= narrow.right);
    assert!(rect.bottom <= narrow.bottom);
    assert_eq!(text_of(&tooltip), QUOTAS);
}

#[test]
fn losing_or_replacing_the_readout_invalidates_the_tooltip_and_hides_it() {
    let owner = TestWindow::new(WS_POPUP, None);
    let other = TestWindow::new(WS_POPUP, None);
    let mut tooltip = UsageTooltip::new(owner.0.0 as isize, QUOTAS).unwrap();
    assert!(tooltip.is_valid_for(owner.0.0 as isize));
    assert!(!tooltip.is_valid_for(other.0.0 as isize));
    tooltip.show_in(ANCHOR, AREA, 1.0);
    let owner_handle = owner.0.0 as isize;
    drop(owner);
    assert!(!tooltip.is_valid_for(owner_handle));
    tooltip.show(1.0);
    assert!(!unsafe { IsWindowVisible(tooltip.window) }.as_bool());
}
