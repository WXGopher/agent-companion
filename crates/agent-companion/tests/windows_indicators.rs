//! Render the real Windows indicators on any host, with a controllable clock.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

slint::slint! {
    import { TaskbarBar } from "../ui/taskbar.slint";
    import { SessionBlock } from "../ui/common.slint";
    export { TaskbarBar }

    export component SessionFixture inherits Window {
        width: 40px;
        height: 40px;
        background: #070d19;
        in property <string> phase;
        in property <bool> pulse;
        SessionBlock {
            x: 8px;
            y: 8px;
            phase: root.phase;
            pulse: root.pulse;
        }
    }
}

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<u64>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(self.now.get())
    }
}

fn fixture() -> (Rc<MinimalSoftwareWindow>, Rc<Cell<u64>>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let now = Rc::new(Cell::new(0));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: window.clone(),
        now: now.clone(),
    }))
    .unwrap();
    (window, now)
}

fn draw(window: &MinimalSoftwareWindow) -> Vec<Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let size = window.size();
    let mut pixels = vec![Rgb8Pixel::default(); (size.width * size.height) as usize];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, size.width as usize);
    });
    pixels
}

fn rgb(r: u8, g: u8, b: u8) -> Rgb8Pixel {
    Rgb8Pixel { r, g, b }
}

#[test]
fn taskbar_breathes_from_bright_status_colour_to_opaque_gray() {
    let (window, _) = fixture();
    let bar = TaskbarBar::new().unwrap();
    bar.window().set_size(slint::LogicalSize::new(61.0, 39.0));
    bar.show().unwrap();
    let pulse_gray = rgb(0x92, 0x97, 0x9f);
    // Pending retains priority over working. Static outcomes keep their
    // status colour at both ends of the shared pulse.
    for (pending, running, done, failed, stopped, expected, active) in [
        (0, 1, 0, 0, 0, rgb(0x32, 0xb8, 0xff), true),
        (1, 1, 0, 0, 0, rgb(0xe3, 0xbf, 0x52), true),
        (0, 0, 1, 0, 0, rgb(0x55, 0xc9, 0x8d), false),
        (0, 0, 1, 1, 0, rgb(0xe3, 0xbf, 0x52), false),
        (0, 0, 1, 0, 1, rgb(0x6a, 0x6a, 0x80), false),
        (0, 0, 0, 0, 0, rgb(0x6a, 0x6a, 0x80), false),
    ] {
        bar.set_chips(ModelRc::new(VecModel::from(vec![UsageChip {
            agent: "codex".into(),
            value: "72%".into(),
            pending,
            running,
            done,
            failed,
            stopped,
            ..Default::default()
        }])));
        bar.set_pulse(1.0);
        let high = draw(&window);
        let width = window.size().width as usize;
        let center = 11 * width + 7;
        assert_eq!(high[center], expected);
        bar.set_pulse(0.0);
        let low = draw(&window);
        assert_eq!(low[center], if active { pulse_gray } else { expected });
        if running > 0 && pending == 0 {
            assert_eq!(
                high[27 * width + 7],
                expected,
                "task-line dot is bright blue"
            );
            assert_eq!(
                low[27 * width + 7],
                pulse_gray,
                "task-line dot stays opaque"
            );
        }
        if !active {
            assert_eq!(low, high, "inactive marks remain completely still");
        }
    }
}

#[test]
fn panel_session_blocks_use_the_same_opaque_gray_endpoints() {
    let (window, now) = fixture();
    let panel = SessionFixture::new().unwrap();
    panel.window().set_size(slint::LogicalSize::new(40.0, 40.0));
    panel.show().unwrap();
    for (phase, expected, active) in [
        ("running", rgb(0x32, 0xb8, 0xff), true),
        ("waitingForApproval", rgb(0xe3, 0xbf, 0x52), true),
        ("waitingForAnswer", rgb(0xe3, 0xbf, 0x52), true),
        ("completed", rgb(0x55, 0xc9, 0x8d), false),
        ("failed", rgb(0xe3, 0xbf, 0x52), false),
        ("stopped", rgb(0x6a, 0x6a, 0x80), false),
        ("idle", rgb(0x6a, 0x6a, 0x80), false),
    ] {
        panel.set_phase(phase.into());
        for pulse in [true, false] {
            panel.set_pulse(pulse);
            let _ = draw(&window);
            now.set(now.get() + 1_000);
            let pixels = draw(&window);
            let center = 17 * window.size().width as usize + 17;
            assert_eq!(
                pixels[center],
                if active && !pulse {
                    rgb(0x92, 0x97, 0x9f)
                } else {
                    expected
                },
                "{phase}, pulse={pulse}"
            );
        }
    }
}
