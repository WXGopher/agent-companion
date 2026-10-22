//! Exercise the real Windows release footer on any host, without a network.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::private_unstable_api::re_exports::{
    AccessibleRole, AccessibleStringProperty, ItemRc, WindowInner,
};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

slint::slint! {
    import { FlyoutWindow } from "../ui/flyout.slint";
    import { SessionRow } from "../ui/common.slint";
    export { FlyoutWindow, SessionRow }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::ZERO
    }
}

fn accessible_items(root: ItemRc) -> Vec<ItemRc> {
    let mut pending = vec![root];
    let mut items = Vec::new();
    while let Some(item) = pending.pop() {
        let mut child = item.first_child();
        while let Some(next) = child {
            child = next.next_sibling();
            pending.push(next);
        }
        if item.is_accessible() && item.is_visible() {
            items.push(item);
        }
    }
    items
}

fn control_bounds(
    window: &MinimalSoftwareWindow,
    label: &str,
    role: AccessibleRole,
) -> Option<(f32, f32, f32, f32)> {
    let root = ItemRc::new_root(WindowInner::from_pub(window.window()).component());
    let matches = accessible_items(root)
        .into_iter()
        .filter(|item| {
            item.accessible_role() == role
                && item
                    .accessible_string_property(AccessibleStringProperty::Label)
                    .is_some_and(|text| text == label)
        })
        .collect::<Vec<_>>();
    assert!(matches.len() <= 1, "one visible control named {label:?}");
    let item = matches.first()?;
    let geometry = item.geometry();
    let origin = item.map_to_window(geometry.origin);
    let size = window.size().to_logical(window.window().scale_factor());
    assert!(
        geometry.size.width > 0.0
            && geometry.size.height > 0.0
            && origin.x >= 0.0
            && origin.y >= 0.0
            && origin.x + geometry.size.width <= size.width
            && origin.y + geometry.size.height <= size.height,
        "{label:?} must fit in the panel at {}x",
        window.window().scale_factor()
    );
    Some((
        origin.x,
        origin.y,
        geometry.size.width,
        geometry.size.height,
    ))
}

fn click_control(window: &MinimalSoftwareWindow, label: &str) {
    let (x, y, width, height) = control_bounds(window, label, AccessibleRole::Button)
        .unwrap_or_else(|| panic!("missing visible button {label:?}"));
    // Deliver normal pointer events to the control's real geometry. Invoking
    // its callback directly would bypass the disabled-state regression.
    click(window, x + width / 2.0, y + height / 2.0);
}

fn save_render(window: &MinimalSoftwareWindow, pixels: &[Rgb8Pixel], name: &str) {
    if let Some(dir) = std::env::var_os("AGENT_COMPANION_RENDER_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let size = window.size();
        let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
        bytes.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
        std::fs::write(dir.join(format!("{name}.ppm")), bytes).unwrap();
    }
}

fn draw(window: &MinimalSoftwareWindow) -> Vec<Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut pixels = Vec::new();
    window.draw_if_needed(|renderer| {
        let size = window.size();
        pixels.resize((size.width * size.height) as usize, Rgb8Pixel::default());
        renderer.render(&mut pixels, size.width as usize);
    });
    assert!(!pixels.is_empty(), "the panel must render");
    pixels
}

fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

#[test]
fn release_footer_is_explicit_and_fits_both_pages_without_changing_preview() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let panel = FlyoutWindow::new().unwrap();
    let opened = Rc::new(Cell::new(0));
    panel.on_view_update({
        let opened = opened.clone();
        move || opened.set(opened.get() + 1)
    });
    let settings = Rc::new(Cell::new(0));
    panel.on_settings({
        let settings = settings.clone();
        move || settings.set(settings.get() + 1)
    });
    let refreshed = Rc::new(Cell::new(0));
    panel.on_refresh_usage({
        let refreshed = refreshed.clone();
        move || refreshed.set(refreshed.get() + 1)
    });
    panel.show().unwrap();

    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        panel.set_compact(true);
        panel.set_sessions(ModelRc::default());
        panel
            .window()
            .set_size(slint::LogicalSize::new(320.0, 82.0));
        panel.set_update_version("".into());
        let preview = draw(&window);
        panel.set_update_version("0.4.2".into());
        assert_eq!(
            draw(&window),
            preview,
            "a cached release must not change the hover preview at {scale}x"
        );

        panel.set_compact(false);
        panel.set_sessions(ModelRc::new(VecModel::from(
            (0..20)
                .map(|n| SessionRow {
                    id: format!("session-{n}").into(),
                    title: "Task with a long title to exercise the scroll area".into(),
                    detail: "Working".into(),
                    source: "codex".into(),
                    phase: "running".into(),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        for (width, height) in [(380.0, 520.0), (320.0, 360.0)] {
            panel
                .window()
                .set_size(slint::LogicalSize::new(width, height));
            draw(&window);
            let size = window.size();
            for usage in [false, true] {
                let opened_before = opened.get();
                click(&window, if usage { 116.0 } else { 42.0 }, 28.0);
                assert_eq!(panel.get_usage_page(), usage);
                panel.set_update_version("".into());
                let without_release = draw(&window);
                panel.set_update_version("0.4.2".into());
                assert_ne!(draw(&window), without_release);
                // Long valid numeric versions must elide rather than push
                // either footer action beyond the fixed panel width.
                panel.set_update_version("1234567890.1234567890.1234567890".into());
                draw(&window);
                if usage {
                    let refresh_before = refreshed.get();
                    click(&window, width - 46.0, 80.0);
                    assert_eq!(refreshed.get(), refresh_before + 1);
                }
                assert_eq!(
                    opened.get(),
                    opened_before,
                    "rendering, page navigation and quota refresh must not open a release"
                );
                click(&window, width - 116.0, height - 28.0);
                assert_eq!(opened.get(), opened_before + 1);
                let settings_before = settings.get();
                click(&window, width - 46.0, height - 28.0);
                assert_eq!(settings.get(), settings_before + 1);
                assert_eq!(window.size(), size, "the footer must not resize the panel");
            }
        }
    }
    panel.hide().unwrap();
}

#[test]
fn one_click_update_progress_failure_and_retry_fit_both_pages_at_common_scales() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let panel = FlyoutWindow::new().unwrap();
    let starts = Rc::new(Cell::new(0));
    panel.on_view_update({
        let starts = starts.clone();
        let weak = panel.as_weak();
        move || {
            starts.set(starts.get() + 1);
            let panel = weak.upgrade().unwrap();
            panel.set_update_busy(true);
            panel.set_update_action_available(false);
            panel.set_update_action_label("下载中…".into());
            panel.set_update_message("正在下载更新 · 5.0 / 20.0 MiB · 25%".into());
            panel.set_update_progress_visible(true);
            panel.set_update_progress(0.25);
            panel.set_update_details_open(false);
            panel.set_update_details_scroll_y(0.0);
        }
    });
    let settings = Rc::new(Cell::new(0));
    panel.on_settings({
        let settings = settings.clone();
        move || settings.set(settings.get() + 1)
    });
    panel.show().unwrap();

    for scale in [1.0, 1.25, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        for (width, height) in [(380.0, 520.0), (320.0, 360.0)] {
            panel
                .window()
                .set_size(slint::LogicalSize::new(width, height));
            for usage in [false, true] {
                window.dispatch_event(WindowEvent::PointerExited);
                panel.set_compact(false);
                panel.set_usage_page(usage);
                panel.set_update_version("".into());
                panel.set_update_message("".into());
                panel.set_update_action_label("一键更新".into());
                panel.set_update_busy(false);
                panel.set_update_failed(false);
                panel.set_update_action_available(true);
                panel.set_update_progress_visible(false);
                draw(&window);
                assert!(control_bounds(&window, "一键更新", AccessibleRole::Button).is_none());

                let before = starts.get();
                panel.set_update_version("0.4.2".into());
                draw(&window);
                assert_eq!(starts.get(), before, "rendering must not start an update");
                click_control(&window, "一键更新");
                draw(&window);
                assert_eq!(starts.get(), before + 1);
                click_control(&window, "下载中…");
                assert_eq!(
                    starts.get(),
                    before + 1,
                    "a busy control must reject a real click"
                );

                let quarter = draw(&window);
                panel.set_update_progress(0.75);
                let three_quarters = draw(&window);
                assert_ne!(
                    quarter, three_quarters,
                    "download progress must be rendered"
                );
                let settings_before = settings.get();
                click_control(&window, "Settings");
                assert_eq!(
                    settings.get(),
                    settings_before + 1,
                    "settings stays responsive"
                );

                for label in ["校验中…", "准备中…", "重启中…"] {
                    panel.set_update_action_label(label.into());
                    panel.set_update_message(format!("{label}请稍候").into());
                    panel.set_update_progress_visible(false);
                    draw(&window);
                    click_control(&window, label);
                    assert_eq!(starts.get(), before + 1);
                    control_bounds(&window, "Settings", AccessibleRole::Button).unwrap();
                }

                let error = "更新失败：The destination is not writable. Check the installation directory permissions, close programs using these files, then retry. 完整错误结尾：权限检查后重试。";
                panel.set_update_busy(false);
                panel.set_update_failed(true);
                panel.set_update_action_available(true);
                panel.set_update_action_label("重试更新".into());
                panel.set_update_message(error.into());
                let failed = draw(&window);
                save_render(
                    &window,
                    &failed,
                    &format!("windows-update-error-{width}-{height}-{scale}-{usage}"),
                );
                let (_, status_y, _, status_height) =
                    control_bounds(&window, error, AccessibleRole::Text).expect(
                        "the complete failure must remain accessible, even when visually elided",
                    );
                control_bounds(&window, "Settings", AccessibleRole::Button).unwrap();
                control_bounds(&window, "重试更新", AccessibleRole::Button).unwrap();
                let size = window.size();

                let task_scroll = panel.get_task_scroll_y();
                let usage_scroll = panel.get_usage_scroll_y();
                click_control(&window, "错误详情");
                let detail_pixels = draw(&window);
                save_render(
                    &window,
                    &detail_pixels,
                    &format!("windows-update-details-{width}-{height}-{scale}-{usage}"),
                );
                assert!(panel.get_update_details_open());
                assert_ne!(
                    detail_pixels, failed,
                    "the complete error must be visibly expanded"
                );
                let (_, _, detail_width, detail_height) = control_bounds(
                    &window,
                    &format!("完整更新错误：{error}"),
                    AccessibleRole::Text,
                )
                .expect("the expanded error must contain the complete text and fit the viewport");
                assert!(
                    detail_width <= width - 28.0 && detail_height > status_height,
                    "full error text must wrap and grow beyond the elided status"
                );
                control_bounds(&window, "Settings", AccessibleRole::Button).unwrap();
                control_bounds(&window, "重试更新", AccessibleRole::Button).unwrap();

                if scale == 1.0 && !usage {
                    let long_error = format!(
                        "{error}\n{}完整错误结尾：长错误也可滚动阅读。",
                        "Directory permission detail\n".repeat(80)
                    );
                    panel.set_update_message(long_error.as_str().into());
                    draw(&window);
                    window.dispatch_event(WindowEvent::PointerScrolled {
                        position: slint::LogicalPosition::new(width / 2.0, 200.0),
                        delta_x: 0.0,
                        delta_y: -100_000.0,
                    });
                    let bottom_pixels = draw(&window);
                    save_render(
                        &window,
                        &bottom_pixels,
                        &format!("windows-update-long-details-{width}-{height}"),
                    );
                    assert!(
                        panel.get_update_details_scroll_y() < 0.0,
                        "long errors must scroll through real pointer input"
                    );
                    let root = ItemRc::new_root(WindowInner::from_pub(window.window()).component());
                    let items = accessible_items(root);
                    let full_text = items
                        .iter()
                        .find(|item| {
                            item.accessible_string_property(AccessibleStringProperty::Label)
                                .is_some_and(|label| label == format!("完整更新错误：{long_error}"))
                        })
                        .expect("the long error must retain its complete text");
                    let text_geometry = full_text.geometry();
                    let text_origin = full_text.map_to_window(text_geometry.origin);
                    let text_bottom = text_origin.y + text_geometry.size.height;
                    assert!(
                        text_bottom > 70.0 && text_bottom < status_y,
                        "scrolling must bring the final line into the visible viewport: {text_bottom} / {status_y}"
                    );
                    panel.set_update_message(error.into());
                }
                click_control(&window, "收起错误");
                draw(&window);
                assert!(!panel.get_update_details_open());
                assert_eq!(panel.get_usage_page(), usage);
                assert_eq!(panel.get_task_scroll_y(), task_scroll);
                assert_eq!(panel.get_usage_scroll_y(), usage_scroll);
                click_control(&window, "错误详情");
                draw(&window);
                click_control(&window, "重试更新");
                assert_eq!(starts.get(), before + 2);
                assert!(
                    !panel.get_update_details_open(),
                    "retry restores the selected Tasks/Usage page"
                );
                assert_eq!(panel.get_usage_page(), usage);
                assert_eq!(
                    window.size(),
                    size,
                    "update status keeps the panel size fixed"
                );
            }
        }
    }
    panel.hide().unwrap();
}
