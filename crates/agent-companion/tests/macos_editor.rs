//! Run the real Slint editor on the main thread. Offscreen SwiftUI tests do
//! not exercise AppKit initialization performed by the separate editor process.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/codex_tui.rs"]
mod codex_tui;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub mod ui {
    slint::include_modules!();
}

fn main() {
    #[cfg(target_os = "macos")]
    check_editor_startup();
}

#[cfg(target_os = "macos")]
fn check_editor_startup() {
    use slint::{
        ComponentHandle,
        language::ColorScheme,
        winit_030::{
            WinitWindowAccessor,
            winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
        },
    };
    use std::{sync::mpsc, time::Duration};

    let (finished, deadline) = mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if deadline.recv_timeout(Duration::from_secs(15)).is_err() {
            eprintln!("The macOS editor did not create a usable window within 15 seconds");
            std::process::exit(1);
        }
    });
    let home = tempfile::tempdir().unwrap();
    macos::prepare_editor().unwrap();
    let editor = codex_tui::Editor::new(home.path().join("config.toml")).unwrap();
    editor
        .window
        .global::<ui::Palette>()
        .set_color_scheme(ColorScheme::Light);
    editor.show().unwrap();
    slint::Timer::single_shot(Duration::ZERO, macos::start_editor_preferences);
    let weak = editor.window.as_weak();
    let timer = slint::Timer::default();
    let mut phase = 0;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let window = weak.upgrade().unwrap();
        assert!(window.get_macos_preferences());
        assert!(window.get_ready());
        let mut created = false;
        window.window().with_winit_window(|native| {
            created = native.is_visible() == Some(true);
            let RawWindowHandle::AppKit(handle) = native.window_handle().unwrap().as_raw() else {
                panic!("The settings window is not an AppKit window");
            };
            // A Slint snapshot covers only content, so it misses a clear native
            // title bar. Inspect the actual window backing the current view.
            // SAFETY: winit owns this live NSView, and this timer runs on the
            // AppKit main thread while the native window is borrowed.
            unsafe {
                let view = handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref();
                let frame = view.window().expect("The view has no native window");
                assert!(frame.isOpaque(), "The settings frame must be opaque");
                assert_eq!(frame.backgroundColor().alphaComponent(), 1.0);
                assert!(!frame.titlebarAppearsTransparent());
            }
        });
        assert!(created, "The settings window was not created and shown");
        let pixels = window.window().take_snapshot().unwrap();
        assert!(pixels.width() >= 620 && pixels.height() >= 620);
        if let Some(path) = std::env::var_os("AGENT_COMPANION_EDITOR_SNAPSHOT") {
            use std::io::Write;
            let path = std::path::PathBuf::from(path);
            let path = if phase == 0 {
                path
            } else {
                path.with_file_name(format!(
                    "{}-{}.pam",
                    path.file_stem().unwrap().to_string_lossy(),
                    ["light", "dark", "compact-dirty", "compact-error"][phase]
                ))
            };
            let mut image = std::fs::File::create(path).unwrap();
            write!(
                image,
                "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
                pixels.width(),
                pixels.height()
            )
            .unwrap();
            image.write_all(pixels.as_bytes()).unwrap();
        }
        match phase {
            0 => window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark),
            1 => {
                window.window().set_size(slint::LogicalSize::new(820.0, 660.0));
                window.invoke_toggle("git-branch".into(), true);
                assert!(window.get_dirty());
                assert!(window.get_preview().contains("main"));
            }
            2 => {
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
                window.set_error(true);
                window.set_message("Could not save: the status bar was changed outside Agent Companion. Reopen Settings to load the latest configuration.".into());
            }
            _ => slint::quit_event_loop().unwrap(),
        }
        phase += 1;
    });
    slint::run_event_loop().unwrap();
    assert!(!home.path().join("config.toml").exists());
    finished.send(()).unwrap();
    watchdog.join().unwrap();
    println!(
        "PASS: opaque AppKit settings frame; Slint light/dark, minimum size, live draft and save error; no Codex config writes"
    );
}
