//! Exercise the real Windows release footer on any host, without a network.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
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
