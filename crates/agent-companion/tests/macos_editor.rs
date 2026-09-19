//! Run the real Slint editor on the main thread. Offscreen SwiftUI tests do
//! not exercise AppKit initialization performed by the separate editor process.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/codex_tui.rs"]
mod codex_tui;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/macos_deployment.rs"]
mod macos_deployment;
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
    let config_path = home.path().join("config.toml");
    let second_config_path = home.path().join("dodex/config.toml");
    macos::prepare_editor().unwrap();
    let editor = codex_tui::Editor::new_isolated(config_path.clone()).unwrap();
    // Freeze live preferences while rendering synthetic UI states. This never
    // writes the user's display choice or changes the actual primary screen.
    editor.preference_timer.stop();
    editor
        .window
        .set_config_path("/Users/example/.codex/config.toml".into());
    editor
        .window
        .set_display_options(slint::ModelRc::new(slint::VecModel::from(vec![
            "Follow primary display".into(),
            "Built-in Retina Display".into(),
            "Studio Display (Primary)".into(),
            "Office Display (Disconnected)".into(),
        ])));
    editor.window.set_selected_display(0);
    editor.window.set_display_status(
        "Follows the primary display set in macOS. Changes save automatically.".into(),
    );
    editor
        .window
        .global::<ui::Palette>()
        .set_color_scheme(ColorScheme::Light);
    editor.show().unwrap();
    editor
        .window
        .set_config_path("/Users/example/.codex/config.toml".into());
    editor.window.set_show_dock_icon(true);
    editor.window.set_show_menu_bar(true);
    editor.window.set_show_notch(false);
    slint::Timer::single_shot(Duration::ZERO, macos::start_editor_preferences);
    let weak = editor.window.as_weak();
    let weak_editor = std::rc::Rc::downgrade(&editor);
    let timer = slint::Timer::default();
    let mut phase = 0;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let window = weak.upgrade().unwrap();
        assert!(window.get_macos_preferences());
        assert!(window.get_ready());
        let displays = macos::display_settings().expect("Display preferences did not cross the native bridge");
        assert_eq!(displays.options.first().unwrap().id, "");
        assert!(displays.options.iter().any(|option| option.id == displays.selected_id));
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
                    [
                        "light",
                        "notch",
                        "dark",
                        "compact-dirty",
                        "notch-dirty",
                        "notch-error",
                        "compact-error",
                        "all-components",
                        "hidden",
                        "dual-default",
                        "dual-progress",
                        "dual-error",
                        "dual-deployed",
                        "dual-instance",
                        "all-entries-hidden",
                    ][phase]
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
            0 => window.set_settings_page(1),
            1 => {
                window.set_settings_page(0);
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark);
            }
            2 => {
                window.window().set_size(slint::LogicalSize::new(820.0, 660.0));
                window.set_selected_display(3);
                window.set_display_status("Display disconnected. Using the primary display until it returns.".into());
                window.invoke_toggle("git-branch".into(), true);
                assert!(window.get_dirty());
                assert!(window.get_preview().contains("main"));
            }
            3 => window.set_settings_page(1),
            4 => {
                assert!(window.get_dirty());
                assert!(window.get_preview().contains("main"));
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
                window.set_display_error(true);
                window.set_dock_error(true);
                window.set_menu_bar_error(true);
                window.set_notch_error(true);
                window.set_error(true);
                window.set_message("Could not save: the status bar was changed outside Agent Companion. Reopen Settings to load the latest configuration.".into());
            }
            5 => window.set_settings_page(0),
            6 => {
                assert!(window.get_dirty(), "Switching settings sections lost the draft");
                assert!(window.get_preview().contains("main"));
                assert!(!config_path.exists(), "Switching sections saved the draft");
                window.set_error(false);
                for item in agent_companion_core::install::codex_tui::COMPONENTS {
                    window.invoke_toggle(item.id.into(), true);
                }
            }
            7 => {
                assert_eq!(window.get_selected_count() as usize, agent_companion_core::install::codex_tui::COMPONENTS.len());
                for item in agent_companion_core::install::codex_tui::COMPONENTS {
                    window.invoke_toggle(item.id.into(), false);
                }
            }
            8 => {
                assert_eq!(window.get_selected_count(), 0);
                // A selection that disappeared before saving must restore the
                // persisted choice, without touching real display preferences.
                window.set_display_error(false);
                window.invoke_select_display("Unavailable display fixture".into());
                assert!(window.get_display_error());
                assert_eq!(macos::display_settings().unwrap(), displays);
                assert_eq!(
                    window.get_selected_display() as usize,
                    displays.options.iter().position(|option| option.id == displays.selected_id).unwrap()
                );
                window.set_settings_page(2);
                window.set_dual_message("创建独立环境，首次使用需要自行登录。".into());
            }
            9 => {
                assert!(!window.get_dual_enabled());
                assert!(!window.get_dual_deployed());
                assert!(!window.get_dual_busy());
                window.set_dual_busy(true);
                window.set_dual_phase("正在验证隔离与签名…".into());
                window.set_dual_message("正在检查应用与隔离目录…".into());
            }
            10 => {
                assert!(window.get_dual_busy());
                assert!(window.get_dirty());
                assert!(!config_path.exists());
                window.set_dual_busy(false);
                window.set_dual_error(true);
                window.set_dual_message("Codex 应用签名无效或不是 OpenAI 官方签名；未修改现有环境。请检查后重试。".into());
            }
            11 => {
                assert!(window.get_dual_error());
                assert!(!window.get_dual_busy(), "A failed operation must allow retry");
                window.set_dual_deployed(true);
                window.set_dual_enabled(true);
                window.set_dual_error(false);
                window.set_dual_message("在应用程序中打开 Dodex，首次使用请登录。".into());
                weak_editor.upgrade().unwrap().add_isolated_secondary(second_config_path.clone());
            }
            12 => {
                assert!(window.get_dual_deployed());
                window.set_settings_page(0);
                window.invoke_select_instance("Dodex".into());
                assert_eq!(window.get_selected_instance(), 1);
                assert_eq!(window.get_config_path(), second_config_path.to_string_lossy().as_ref());
                assert!(!window.get_dirty(), "Codex draft must not become the Dodex draft");
                window.invoke_toggle("weekly-limit".into(), true);
                window.invoke_apply();
                assert!(!window.get_dirty());
                assert!(second_config_path.exists());
                assert!(!config_path.exists(), "Saving Dodex wrote the primary configuration");
                assert!(agent_companion_core::install::codex_tui::read(&second_config_path).unwrap().visible_items().contains(&"weekly-limit".to_string()));
                window.set_config_path("/Users/example/Library/Application Support/AgentCompanion/Dodex/codex-home/config.toml".into());
            }
            13 => {
                window.invoke_select_instance("Codex".into());
                assert!(window.get_dirty(), "Switching instances discarded the Codex draft");
                assert_eq!(window.get_selected_count(), 0);
                assert!(!config_path.exists());
                window.set_settings_page(1);
                window.set_show_dock_icon(false);
                window.set_show_menu_bar(false);
                window.set_show_notch(false);
                window.set_display_error(false);
                window.set_dock_error(false);
                window.set_menu_bar_error(false);
                window.set_notch_error(false);
            }
            _ => {
                assert!(!window.get_show_dock_icon());
                assert!(!window.get_show_menu_bar());
                assert!(!window.get_show_notch());
                slint::quit_event_loop().unwrap();
            }
        }
        phase += 1;
    });
    slint::run_event_loop().unwrap();
    assert!(!home.path().join("config.toml").exists());
    finished.send(()).unwrap();
    watchdog.join().unwrap();
    println!(
        "PASS: opaque AppKit settings frame; three settings sections, light/dark, minimum size, long/hidden preview, entry/display errors, deployment default/progress/retry/success, isolated instance saves and hidden-entry recovery; navigation preserves separate drafts; no user config, deployment or display preference writes"
    );
}
