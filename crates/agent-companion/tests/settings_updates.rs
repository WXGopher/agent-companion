//! Real settings markup and controller with a synthetic, offline release feed.
#![cfg(any(windows, target_os = "macos"))]

use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::private_unstable_api::re_exports::{
    AccessibleRole, AccessibleStringProperty, ItemRc, WindowInner,
};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

#[allow(dead_code)]
#[path = "../src/settings_update.rs"]
mod settings_update;
#[allow(dead_code)]
#[path = "../src/update_service.rs"]
mod update_service;

mod ui {
    slint::slint! {
        #[style = "fluent"]
        import { CodexTuiWindow, StatusComponent } from "../ui/codex-tui.slint";
        export { CodexTuiWindow, StatusComponent }
        export { Palette } from "std-widgets.slint";
    }
}

thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(CLOCK.get())
    }
}

fn pump() {
    CLOCK.set(CLOCK.get() + 200);
    slint::platform::update_timers_and_animations();
}

fn draw(window: &MinimalSoftwareWindow, name: Option<&str>) {
    pump();
    window.request_redraw();
    let mut rendered = false;
    window.draw_if_needed(|renderer| {
        let size = window.size();
        let mut pixels = vec![Rgb8Pixel::default(); (size.width * size.height) as usize];
        renderer.render(&mut pixels, size.width as usize);
        if let Some(name) = name
            && let Some(directory) = std::env::var_os("AGENT_COMPANION_RENDER_DIR")
        {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
            bytes.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
            std::fs::write(directory.join(format!("{name}.ppm")), bytes).unwrap();
        }
        rendered = true;
    });
    assert!(rendered);
}

#[derive(Debug)]
struct Bounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn button(window: &MinimalSoftwareWindow, label: &str) -> Bounds {
    let root = ItemRc::new_root(WindowInner::from_pub(window.window()).component());
    let mut pending = vec![root];
    let mut found = Vec::new();
    while let Some(item) = pending.pop() {
        let mut child = item.first_child();
        while let Some(next) = child {
            child = next.next_sibling();
            pending.push(next);
        }
        if item.is_accessible()
            && item.is_visible()
            && item.accessible_role() == AccessibleRole::Button
            && item
                .accessible_string_property(AccessibleStringProperty::Label)
                .is_some_and(|value| value == label)
        {
            let geometry = item.geometry();
            let origin = item.map_to_window(geometry.origin);
            found.push(Bounds {
                x: origin.x,
                y: origin.y,
                width: geometry.size.width,
                height: geometry.size.height,
            });
        }
    }
    assert_eq!(found.len(), 1, "one visible button named {label}");
    let bounds = found.remove(0);
    let size = window.size().to_logical(window.window().scale_factor());
    assert!(
        bounds.x >= 0.0
            && bounds.y >= 0.0
            && bounds.width > 0.0
            && bounds.height > 0.0
            && bounds.x + bounds.width <= size.width
            && bounds.y + bounds.height <= size.height,
        "{label} must fit inside {size:?}: {bounds:?}"
    );
    bounds
}

fn click(window: &MinimalSoftwareWindow, label: &str) {
    let bounds = button(window, label);
    let position = slint::LogicalPosition::new(
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    );
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn wait_result(settings: &ui::CodexTuiWindow) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while settings.get_update_checking() {
        assert!(Instant::now() < deadline, "the release check must complete");
        std::thread::sleep(Duration::from_millis(1));
        pump();
    }
}

fn release(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "tag_name": tag, "draft": false, "prerelease": false,
        "html_url": "https://untrusted.example/not-the-release"
    }))
    .unwrap()
}

#[test]
fn settings_check_feedback_is_explicit_persistent_and_visible_on_every_page() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let settings = ui::CodexTuiWindow::new().unwrap();
    settings.set_windows_preferences(true);
    settings.set_ready(true);
    settings.set_selected_count(2);
    settings.set_config_path("~/.codex/config.toml".into());
    settings.set_preview("sample-project · main".into());
    settings.set_left_components(ModelRc::new(VecModel::from(vec![
        ui::StatusComponent {
            id: "current-dir".into(),
            label: "Current directory".into(),
            description: "Synthetic current directory".into(),
            selected: true,
        },
        ui::StatusComponent {
            id: "git-branch".into(),
            label: "Git branch".into(),
            description: "Synthetic branch".into(),
            selected: true,
        },
    ])));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (response_tx, response_rx) = mpsc::channel::<io::Result<Vec<u8>>>();
    let response_rx = Mutex::new(response_rx);
    let (started_tx, started_rx) = mpsc::channel();
    let service = update_service::UpdateService::fixture("1.2.3", {
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            started_tx.send(()).unwrap();
            response_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
        }
    });
    let opened = Rc::new(RefCell::new(Vec::new()));
    let fail_browser = Rc::new(Cell::new(false));
    let _controller = settings_update::Controller::with_opener(&settings, service, {
        let opened = opened.clone();
        let fail_browser = fail_browser.clone();
        move |url| {
            opened.borrow_mut().push(url.to_owned());
            if fail_browser.get() {
                Err(io::Error::other("Synthetic browser failure"))
            } else {
                Ok(())
            }
        }
    });
    settings.show().unwrap();
    settings
        .window()
        .set_size(slint::LogicalSize::new(820.0, 720.0));
    draw(&window, Some("updates-idle-windows"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(settings.get_update_message().is_empty());

    click(&window, "检查更新");
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(settings.get_update_checking());
    draw(&window, Some("updates-checking-windows"));
    click(&window, "检查中…");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    response_tx
        .send(Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Synthetic private network diagnostic",
        )))
        .unwrap();
    wait_result(&settings);
    assert!(settings.get_update_failed());
    assert!(!settings.get_update_available());
    assert_eq!(settings.get_update_message(), "检查更新失败，请稍后重试。");
    draw(&window, Some("updates-failed-windows"));

    click(&window, "检查更新");
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    response_tx.send(Ok(release("v1.2.3"))).unwrap();
    wait_result(&settings);
    assert!(!settings.get_update_failed());
    assert!(!settings.get_update_available());
    assert_eq!(settings.get_update_message(), "当前已是最新版本。");
    draw(&window, Some("updates-current-windows"));

    click(&window, "检查更新");
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    response_tx.send(Ok(release("v1.2.10"))).unwrap();
    wait_result(&settings);
    assert!(settings.get_update_available());
    let notice = settings.get_update_message();
    assert_eq!(
        notice,
        "发现新版本 v1.2.10，可前往 GitHub Release 查看和下载。"
    );
    assert!(
        opened.borrow().is_empty(),
        "a check must not open a browser"
    );

    for macos in [false, true] {
        settings.set_windows_preferences(!macos);
        settings.set_macos_preferences(macos);
        let platform = if macos { "macos" } else { "windows" };
        let sizes = if macos {
            [(920.0, 760.0), (820.0, 720.0)]
        } else {
            [(820.0, 720.0), (720.0, 640.0)]
        };
        for (width, height) in sizes {
            for scale in [1.0, 1.5, 2.0] {
                window.dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
                settings
                    .window()
                    .set_size(slint::LogicalSize::new(width, height));
                for page in 0..if macos { 3 } else { 4 } {
                    settings.set_settings_page(page);
                    let filename = format!("updates-available-{platform}-{width}-page{page}");
                    draw(&window, (scale == 1.0).then_some(filename.as_str()));
                    let check = button(&window, "检查更新");
                    let link = button(&window, "查看 GitHub Release");
                    assert!(link.y >= check.y + check.height);
                    if page == 0 {
                        button(&window, "Apply changes");
                        button(&window, "Restore defaults");
                    }
                    assert_eq!(settings.get_update_message(), notice);
                }
            }
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(opened.borrow().is_empty());
    settings.hide().unwrap();
    pump();
    settings.show().unwrap();
    draw(&window, None);
    assert_eq!(settings.get_update_message(), notice);

    fail_browser.set(true);
    click(&window, "查看 GitHub Release");
    pump();
    assert!(settings.get_update_failed());
    assert!(settings.get_update_message().contains("无法打开浏览器"));
    fail_browser.set(false);
    draw(&window, None);
    click(&window, "查看 GitHub Release");
    assert!(!settings.get_update_failed());
    assert_eq!(settings.get_update_message(), notice);
    assert!(
        opened.borrow().iter().all(|url| {
            url == "https://github.com/WXGopher/agent-companion/releases/tag/v1.2.10"
        })
    );
    assert_eq!(opened.borrow().len(), 2);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    settings.hide().unwrap();
}
