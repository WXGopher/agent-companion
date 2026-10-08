//! A real popup for the embedded taskbar readout. Slint/winit's tooltip is an
//! overlay in the readout's render surface, so its text is clipped to that tiny
//! surface. The common control owns rendering and wrapping; we own its lifetime
//! and use the app's existing pointer poll to avoid stale Slint hover events.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, DeleteObject, HFONT};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR};

use super::{Rect, hwnd, rect_of, work_area_at};

pub struct UsageTooltip {
    window: HWND,
    owner: HWND,
    // The control may retain this pointer until the next text update or destroy.
    text: Vec<u16>,
    font: Option<HFONT>,
    dpi: u32,
    shown: bool,
    geometry: Option<(Rect, Rect, u32)>,
}

impl UsageTooltip {
    pub fn new(owner: isize, text: &str) -> Option<Self> {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        if !unsafe { InitCommonControlsEx(&controls) }.as_bool() {
            return None;
        }
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX | TTS_NOANIMATE | TTS_NOFADE),
                0,
                0,
                0,
                0,
                // The readout's top-level parent belongs to Explorer. Do not
                // make Explorer the owner of our tooltip when it is embedded.
                None,
                None,
                None,
                None,
            )
        }
        .ok()?;
        let tooltip = Self {
            window,
            owner: hwnd(owner),
            text: wide(text),
            font: None,
            dpi: 0,
            shown: false,
            geometry: None,
        };
        if tooltip.send_tool(TTM_ADDTOOLW, 0) == 0 {
            return None;
        }
        Some(tooltip)
    }

    pub fn is_valid_for(&self, owner: isize) -> bool {
        self.owner == hwnd(owner)
            && unsafe { IsWindow(Some(self.window)) }.as_bool()
            && unsafe { IsWindow(Some(self.owner)) }.as_bool()
    }

    fn tool(&self) -> TTTOOLINFOW {
        TTTOOLINFOW {
            // TTTOOLINFOW_V2_SIZE: the unused trailing lpReserved was added
            // for common-controls v6. This prefix also works without a v6
            // activation manifest (including Rust's unit-test executable).
            cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
            uFlags: TTF_IDISHWND | TTF_TRACK | TTF_ABSOLUTE,
            hwnd: self.owner,
            uId: self.owner.0 as usize,
            lpszText: PWSTR(self.text.as_ptr().cast_mut()),
            ..Default::default()
        }
    }

    fn send_tool(&self, message: u32, value: usize) -> isize {
        let tool = self.tool();
        unsafe {
            SendMessageW(
                self.window,
                message,
                Some(WPARAM(value)),
                Some(LPARAM(&tool as *const TTTOOLINFOW as isize)),
            )
            .0
        }
    }

    pub fn set_text(&mut self, text: &str) {
        let replacement = wide(text);
        if replacement == self.text {
            return;
        }
        // Keep the old allocation alive until the synchronous update returns.
        let previous = std::mem::replace(&mut self.text, replacement);
        self.send_tool(TTM_UPDATETIPTEXTW, 0);
        drop(previous);
        self.geometry = None;
        if text.is_empty() {
            self.hide();
        }
    }

    pub fn show(&mut self, scale: f32) {
        if !unsafe { IsWindowVisible(self.owner) }.as_bool() {
            self.hide();
            return;
        }
        let Some(anchor) = rect_of(self.owner) else {
            self.hide();
            return;
        };
        self.show_in(anchor, work_area_at(anchor.left, anchor.top), scale);
    }

    fn show_in(&mut self, anchor: Rect, area: Rect, scale: f32) {
        let dpi = (scale.max(1.0) * 96.0).round() as u32;
        if self.shown && self.geometry == Some((anchor, area, dpi)) {
            return;
        }
        self.set_dpi(dpi);
        // Native wrapping uses physical pixels, with room left for its border.
        let width =
            ((480.0 * scale.max(1.0)).round() as i32).min((area.right - area.left - 40).max(1));
        unsafe {
            SendMessageW(
                self.window,
                TTM_SETMAXTIPWIDTH,
                None,
                Some(LPARAM(width as isize)),
            );
        }
        if !self.shown {
            self.track_position(anchor.right, anchor.top);
            self.send_tool(TTM_TRACKACTIVATE, 1);
            self.shown = true;
        }
        // Get the laid-out native rectangle. GETBUBBLESIZE is unsafe before
        // activation in the unmanifested common-controls implementation.
        unsafe { SendMessageW(self.window, TTM_UPDATE, None, None) };
        let Some(rect) = rect_of(self.window) else {
            self.hide();
            return;
        };
        let size = (rect.right - rect.left, rect.bottom - rect.top);
        if size.0 == 0 || size.1 == 0 {
            self.hide();
            return;
        }
        let (x, y) = super::super::place_flyout_beside(anchor, size, area);
        self.track_position(x, y);
        // TRACKPOSITION encodes signed 16-bit coordinates. Also apply the full
        // coordinates for larger multi-monitor desktops.
        let positioned = unsafe {
            SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            )
        }
        .is_ok();
        self.shown = positioned;
        self.geometry = positioned.then_some((anchor, area, dpi));
        if !positioned {
            self.send_tool(TTM_TRACKACTIVATE, 0);
        }
    }

    fn track_position(&self, x: i32, y: i32) {
        let packed = ((y as u16 as u32) << 16) | x as u16 as u32;
        unsafe {
            SendMessageW(
                self.window,
                TTM_TRACKPOSITION,
                None,
                Some(LPARAM(packed as isize)),
            );
        }
    }

    fn set_dpi(&mut self, dpi: u32) {
        if self.dpi == dpi {
            return;
        }
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        if unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
                0,
                dpi,
            )
        }
        .is_err()
        {
            return;
        }
        let font = unsafe { CreateFontIndirectW(&metrics.lfStatusFont) };
        if font.is_invalid() {
            return;
        }
        unsafe {
            SendMessageW(self.window, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
        }
        if let Some(previous) = self.font.replace(font) {
            let _ = unsafe { DeleteObject(previous.into()) };
        }
        self.dpi = dpi;
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.send_tool(TTM_TRACKACTIVATE, 0);
        }
        self.shown = false;
        self.geometry = None;
    }
}

impl Drop for UsageTooltip {
    fn drop(&mut self) {
        // Destroy the consumer before freeing the UTF-16 text and selected font.
        let _ = unsafe { DestroyWindow(self.window) };
        if let Some(font) = self.font.take() {
            let _ = unsafe { DeleteObject(font.into()) };
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .filter(|unit| *unit != 0)
        .chain([0])
        .collect()
}

#[cfg(test)]
mod tests;
