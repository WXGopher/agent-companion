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
#[path = "../src/macos_primary_app.rs"]
mod macos_primary_app;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/settings_update.rs"]
mod settings_update;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/software_updates.rs"]
mod software_updates;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/tui_deployment.rs"]
mod tui_deployment;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/update_service.rs"]
mod update_service;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/usage_service.rs"]
mod usage_service;
#[cfg(target_os = "macos")]
pub mod ui {
    slint::include_modules!();
}

fn main() {
    #[cfg(target_os = "macos")]
    check_editor_startup();
}

/// Keep pointer acceptance tied to the visible control when the common
/// settings header grows. This still sends real pointer events, not callbacks.
#[cfg(target_os = "macos")]
fn click_control(
    window: &ui::CodexTuiWindow,
    label: &str,
    role: slint::private_unstable_api::re_exports::AccessibleRole,
) {
    use slint::ComponentHandle;
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::private_unstable_api::re_exports::{AccessibleStringProperty, ItemRc, WindowInner};

    let root = ItemRc::new_root(WindowInner::from_pub(window.window()).component());
    let mut pending = vec![root];
    let mut controls = Vec::new();
    while let Some(item) = pending.pop() {
        let mut child = item.first_child();
        while let Some(next) = child {
            child = next.next_sibling();
            pending.push(next);
        }
        if item.is_accessible()
            && item.is_visible()
            && item.accessible_role() == role
            && item
                .accessible_string_property(AccessibleStringProperty::Label)
                .is_some_and(|value| value == label)
        {
            let geometry = item.geometry();
            controls.push((item.map_to_window(geometry.origin), geometry.size));
        }
    }
    assert_eq!(controls.len(), 1, "one visible {role:?} named {label}");
    let (origin, size) = controls.remove(0);
    let bounds = window
        .window()
        .size()
        .to_logical(window.window().scale_factor());
    assert!(
        size.width > 0.0
            && size.height > 0.0
            && origin.x >= 0.0
            && origin.y >= 0.0
            && origin.x + size.width <= bounds.width
            && origin.y + size.height <= bounds.height,
        "{label} must fit inside the visible settings window"
    );
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
}

#[cfg(target_os = "macos")]
fn check_editor_startup() {
    use slint::{
        ComponentHandle, Model,
        language::ColorScheme,
        winit_030::{
            WinitWindowAccessor,
            winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
        },
    };
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    let started = Instant::now();
    eprintln!("[editor-fixture] startup at {:?}", started.elapsed());

    // The watchdog measures a stalled
    // startup or phase, not their cumulative render and filesystem work.
    let (finished, deadline) = mpsc::channel::<Option<usize>>();
    let watchdog = std::thread::spawn(move || {
        let mut last_phase = None;
        loop {
            match deadline.recv_timeout(Duration::from_secs(15)) {
                Ok(Some(phase)) => last_phase = Some(phase),
                Ok(None) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "[editor-fixture] no validated progress for 15 seconds; last completed phase: {last_phase:?}"
                    );
                    std::process::exit(1);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    eprintln!(
                        "[editor-fixture] progress channel closed before completion; last completed phase: {last_phase:?}"
                    );
                    std::process::exit(1);
                }
            }
        }
    });
    let home = tempfile::tempdir().unwrap();
    let fixture_root = home.path().canonicalize().unwrap();
    let config_path = fixture_root.join("primary-profile-with-a-long-path-for-copying/config.toml");
    let second_config_path =
        fixture_root.join("dodex-profile-with-an-independent-home-and-login/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(second_config_path.parent().unwrap()).unwrap();
    macos::prepare_editor().unwrap();
    eprintln!(
        "[editor-fixture] backend prepared at {:?}",
        started.elapsed()
    );
    let editor = codex_tui::Editor::new_isolated(config_path.clone()).unwrap();
    eprintln!(
        "[editor-fixture] editor constructed at {:?}",
        started.elapsed()
    );
    // Isolated callbacks must not read or mutate real login items, even when
    // explicitly invoked by the UI fixture.
    assert!(!editor.login_item_timer.running());
    editor.window.invoke_set_run_at_login(true);
    editor.window.invoke_open_login_item_settings();
    assert!(!editor.window.get_run_at_login());
    assert!(!editor.window.get_login_item_available());
    assert!(editor.window.get_login_item_message().is_empty());
    let usage_path = config_path.parent().unwrap().join("usage.json");
    for invalid in ["", "0", "61", "1.5", "1e1", "abc"] {
        editor
            .window
            .invoke_set_usage_refresh_minutes(invalid.into());
        assert_eq!(editor.window.get_usage_refresh_minutes(), 5);
        assert!(!usage_path.exists(), "Invalid interval must not be saved");
    }
    editor.window.invoke_set_usage_refresh_minutes("12".into());
    assert_eq!(editor.window.get_usage_refresh_minutes(), 12);
    assert_eq!(
        agent_companion_core::usage_service::UsageSettings::load(&usage_path)
            .refresh_interval_minutes,
        12
    );
    editor.window.invoke_set_usage_refresh_minutes("5".into());
    // Freeze deployment polling while rendering synthetic UI states; all
    // configuration and sync actions remain inside the temporary profiles.
    editor.deployment_timer.stop();
    editor
        .window
        .set_config_path("/Users/example/.codex/config.toml".into());
    editor
        .window
        .global::<ui::Palette>()
        .set_color_scheme(ColorScheme::Light);
    editor.show().unwrap();
    eprintln!("[editor-fixture] window shown at {:?}", started.elapsed());
    editor
        .window
        .set_config_path("/Users/example/.codex/config.toml".into());
    let weak = editor.window.as_weak();
    let weak_editor = std::rc::Rc::downgrade(&editor);
    let timer = slint::Timer::default();
    let completed = std::rc::Rc::new(std::cell::Cell::new(false));
    let completed_check = completed.clone();
    let phase_progress = finished.clone();
    // Capture the production Slint callback chain without starting installers,
    // launching apps, or touching the user's accounts from this UI fixture.
    let setup_requests = std::rc::Rc::new(std::cell::Cell::new(0));
    let setup_calls = setup_requests.clone();
    editor
        .window
        .on_deploy_dual(move || setup_calls.set(setup_calls.get() + 1));
    let open_requests = std::rc::Rc::new(std::cell::Cell::new(0));
    let open_calls = open_requests.clone();
    editor
        .window
        .on_open_dual(move || open_calls.set(open_calls.get() + 1));
    let maintenance_requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let maintenance_calls = maintenance_requests.clone();
    editor.window.on_maintain_software(move |action| {
        maintenance_calls.borrow_mut().push(action.to_string())
    });
    let mut phase = 0;
    let mut previous_phase = None;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let fresh_phase = previous_phase != Some(phase);
        if fresh_phase {
            eprintln!("[editor-fixture] phase {phase} begin at {:?}", started.elapsed());
            previous_phase = Some(phase);
        }
        let phase_started = Instant::now();
        let window = weak.upgrade().unwrap();
        assert!(window.get_ready());
        if phase < 42 {
            assert!(window.get_macos_preferences());
            assert!((0..=2).contains(&window.get_settings_page()));
        } else {
            assert!(window.get_windows_preferences());
            assert_eq!(window.get_settings_page(), 3);
        }
        let mut created = false;
        window.window().with_winit_window(|native| {
            created = native.is_visible() == Some(true);
            // This accessory fixture may be behind another application, where
            // the display-link throttle stops scheduling frames. Request a
            // native frame so each synthetic phase receives layout/binding
            // updates before the next timer examines its snapshot.
            native.request_redraw();
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
        if fresh_phase {
            eprintln!("[editor-fixture] phase {phase} snapshot took {:?}", phase_started.elapsed());
        }
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
                        "dual-light",
                        "dark",
                        "compact-dirty",
                        "dual-dirty-dark",
                        "dual-error-light",
                        "compact-error",
                        "all-components",
                        "hidden",
                        "dual-default",
                        "dual-progress",
                        "dual-error",
                        "dual-deployed",
                        "dual-instance",
                        "dual-primary-draft",
                        "sync-ready-light",
                        "sync-dirty-dark",
                        "sync-progress",
                        "sync-success-dark",
                        "sync-noop-progress",
                        "sync-noop-dark",
                        "sync-reverse-progress",
                        "sync-instructions-progress",
                        "sync-instructions-reverse",
                        "sync-disabled-missing-light",
                        "sync-create-target",
                        "sync-error-progress",
                        "sync-error-light",
                        "sync-instructions-light",
                        "dual-package-only-light",
                        "dual-monitor-disabled-light",
                        "dual-monitor-enabled-dark",
                        "software-progress-dark",
                        "software-error-light",
                        "general-light",
                        "general-enabled-dark",
                        "general-approval-error-light",
                        "setup-fresh",
                        "setup-partial",
                        "setup-ready",
                        "setup-updating",
                        "setup-advanced",
                        "setup-windows-pending",
                        "setup-windows",
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
        let click = |label| {
            click_control(
                &window, label,
                slint::private_unstable_api::re_exports::AccessibleRole::Button,
            );
        };
        match phase {
            0 => window.set_settings_page(1),
            1 => {
                window.set_settings_page(0);
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark);
            }
            2 => {
                window.window().set_size(slint::LogicalSize::new(820.0, 720.0));
                window.invoke_toggle("git-branch".into(), true);
                assert!(window.get_dirty());
                assert!(window.get_preview().contains("main"));
            }
            3 => window.set_settings_page(1),
            4 => {
                assert!(window.get_dirty());
                assert!(window.get_preview().contains("main"));
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
                window.set_dual_error(true);
                window.set_dual_message("部署未完成。现有配置和未应用的草稿均已保留。".into());
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
                window.set_settings_page(1);
                window.set_dual_error(false);
                window.set_dual_message("创建独立环境，首次使用需要自行登录。".into());
            }
            9 => {
                assert!(!window.get_dual_advanced_open(), "Advanced controls must not add setup steps to the main flow");
                assert!(!window.get_dual_enabled());
                assert!(!window.get_dual_deployed());
                assert!(!window.get_dual_tui_installed());
                assert!(window.get_dual_command_path().is_empty());
                assert!(window.get_dual_profile_home().is_empty());
                assert!(!window.get_dual_tui_available());
                assert!(!window.get_dual_tui_configured());
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
                window.set_dual_tui_installed(true);
                window.set_dual_tui_available(true);
                window.set_dual_tui_configured(false);
                window.set_dual_message("Dodex TUI 包已安装；TUI 配置未完成。已完成的部分保留，请继续安装并配置。".into());
            }
            11 => {
                assert!(window.get_dual_error());
                assert!(!window.get_dual_busy(), "A failed operation must allow retry");
                assert!(window.get_dual_tui_installed() && window.get_dual_tui_available());
                assert!(!window.get_dual_tui_configured(), "A native package must not imply a configured public command");
                window.set_dual_tui_installed(false);
                window.set_dual_deployed(true);
                window.set_dual_enabled(true);
                window.set_dual_error(false);
                window.set_dual_message("副账号 CLI 已接入 Companion。".into());
                weak_editor.upgrade().unwrap().add_isolated_secondary(second_config_path.clone());
            }
            12 => {
                assert!(window.get_dual_deployed());
                assert!(!window.get_dual_tui_installed(), "A saved profile must not imply an installed native TUI package");
                assert!(window.get_dual_tui_available());
                assert_eq!(window.get_dual_profile_home(), second_config_path.parent().unwrap().to_string_lossy().as_ref());
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
            }
            14 => {
                assert_eq!(window.get_settings_page(), 1);
                assert!(window.get_dirty(), "The dual page lost the primary draft");
                assert!(!config_path.exists(), "Navigation must not have saved the original draft");
                std::fs::write(&config_path, "model = 'primary-model'\napi_key = 'synthetic-config-token'\n[tui]\nstatus_line = ['model']\n").unwrap();
                std::fs::write(&second_config_path, "model = 'secondary-model'\n[tui]\nstatus_line = ['current-dir']\n").unwrap();
                for (path, value) in [(&config_path, "primary-login"), (&second_config_path, "secondary-login")] {
                    std::fs::write(path.with_file_name("auth.json"), value).unwrap();
                    std::fs::write(path.with_file_name("state.sqlite"), "database fixture").unwrap();
                    std::fs::write(path.with_file_name("session.log"), "log fixture").unwrap();
                }
                std::fs::write(config_path.with_file_name("AGENTS.md"), "Primary global instructions\n").unwrap();
                std::fs::write(second_config_path.with_file_name("AGENTS.md"), "Old secondary instructions\n").unwrap();
                std::fs::write(second_config_path.with_file_name("AGENTS.override.md"), "Secondary override stays local\n").unwrap();
                let editor = weak_editor.upgrade().unwrap();
                editor.add_isolated_secondary(second_config_path.clone());
                editor.reload_isolated_drafts();
                window.set_settings_page(1);
                window.set_dual_advanced_open(true);
                window.set_dual_scroll_y(-330.0);
            }
            15 => {
                let config = window.get_sync_files().row_data(0).unwrap();
                let instructions = window.get_sync_files().row_data(1).unwrap();
                assert_eq!(config.primary_path, config_path.to_string_lossy().as_ref());
                assert_eq!(config.secondary_path, second_config_path.to_string_lossy().as_ref());
                assert!(config.to_primary && config.to_secondary);
                assert!(instructions.note.contains("AGENTS.override.md") && instructions.note.contains("可能优先"));
                window.invoke_toggle("git-branch".into(), true);
                assert!(window.get_dirty());
                assert!(!window.get_sync_files().row_data(0).unwrap().to_secondary);
                assert!(window.get_sync_files().row_data(1).unwrap().to_secondary);
                let original = std::fs::read(&second_config_path).unwrap();
                window.invoke_sync_profile("config".into(), true);
                assert!(!window.get_sync_busy() && window.get_dirty());
                assert_eq!(std::fs::read(&second_config_path).unwrap(), original);
                assert!(window.get_sync_files().row_data(0).unwrap().message.contains("未应用"));
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark);
            }
            16 => {
                window.invoke_toggle("git-branch".into(), false);
                assert!(!window.get_dirty());
                // An inactive instance's dirty draft must block config sync too.
                window.invoke_select_instance("Dodex".into());
                window.invoke_toggle("weekly-limit".into(), true);
                window.invoke_select_instance("Codex".into());
                assert!(!window.get_dirty());
                assert!(!window.get_sync_files().row_data(0).unwrap().to_secondary);
                window.invoke_select_instance("Dodex".into());
                window.invoke_toggle("weekly-limit".into(), false);
                window.invoke_select_instance("Codex".into());
                let source = std::fs::read(&config_path).unwrap();
                window.invoke_sync_profile("config".into(), true);
                assert!(window.get_sync_busy() && window.get_dual_busy());
                assert_eq!(window.get_sync_files().row_data(0).unwrap().secondary_path, second_config_path.to_string_lossy().as_ref());
                window.invoke_toggle("git-branch".into(), true);
                window.invoke_apply();
                window.invoke_restore_defaults();
                window.invoke_sync_profile("instructions".into(), true);
                assert!(!window.get_dirty(), "Editing while a file operation was pending changed a draft");
                assert_eq!(std::fs::read(&config_path).unwrap(), source);
                assert_eq!(std::fs::read_to_string(second_config_path.with_file_name("AGENTS.md")).unwrap(), "Old secondary instructions\n");
            }
            17 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(0).unwrap();
                assert!(!row.error, "{}", row.message);
                assert!(row.message.contains("已覆盖"));
                assert!(!row.backup_path.is_empty());
                assert!(std::fs::read_to_string(row.backup_path.as_str()).unwrap().contains("secondary-model"));
                assert!(std::fs::read_to_string(&second_config_path).unwrap().contains("synthetic-config-token"));
                assert_eq!(window.get_selected_instance(), 0, "Sync changed the selected instance");
                window.invoke_select_instance("Dodex".into());
                assert_eq!(window.get_selected_count(), 1);
                assert_eq!(agent_companion_core::install::codex_tui::read(&second_config_path).unwrap().visible_items(), ["model"]);
                assert!(window.get_preview().contains("gpt"), "The target draft was not reloaded after overwrite");
                window.invoke_select_instance("Codex".into());
            }
            18 => { window.invoke_sync_profile("config".into(), true); }
            19 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(0).unwrap();
                assert!(!row.error && row.message.contains("一致") && row.backup_path.is_empty());
            }
            20 => {
                let mut document = std::fs::read_to_string(&second_config_path).unwrap().parse::<toml_edit::DocumentMut>().unwrap();
                document["model"] = toml_edit::value("reverse-model");
                let mut items = toml_edit::Array::new();
                items.push("current-dir");
                document["tui"]["status_line"] = toml_edit::value(items);
                std::fs::write(&second_config_path, document.to_string()).unwrap();
                window.invoke_select_instance("Dodex".into());
                window.invoke_sync_profile("config".into(), false);
            }
            21 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(0).unwrap();
                assert!(!row.error, "{}", row.message);
                assert!(std::fs::read_to_string(row.backup_path.as_str()).unwrap().contains("primary-model"));
                assert!(std::fs::read_to_string(&config_path).unwrap().contains("reverse-model"));
                assert_eq!(window.get_selected_instance(), 1);
                window.invoke_select_instance("Codex".into());
                assert_eq!(window.get_selected_count(), 1);
                assert!(window.get_preview().contains("~/agent-companion"));
                window.invoke_toggle("git-branch".into(), true);
                assert!(window.get_dirty());
                window.set_dual_scroll_y(-10000.0);
                window.invoke_sync_profile("instructions".into(), true);
            }
            22 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(1).unwrap();
                assert!(!row.error, "{}", row.message);
                assert_eq!(std::fs::read_to_string(row.backup_path.as_str()).unwrap(), "Old secondary instructions\n");
                assert!(window.get_dirty(), "Instruction sync discarded a status-bar draft");
                assert_eq!(std::fs::read_to_string(second_config_path.with_file_name("AGENTS.md")).unwrap(), "Primary global instructions\n");
                std::fs::write(second_config_path.with_file_name("AGENTS.md"), "Updated Dodex instructions\n").unwrap();
                window.invoke_sync_profile("instructions".into(), false);
            }
            23 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                assert_eq!(std::fs::read_to_string(config_path.with_file_name("AGENTS.md")).unwrap(), "Updated Dodex instructions\n");
                assert_eq!(std::fs::read_to_string(window.get_sync_files().row_data(1).unwrap().backup_path.as_str()).unwrap(), "Primary global instructions\n");
                assert!(window.get_dirty());
                window.invoke_toggle("git-branch".into(), false);
                weak_editor.upgrade().unwrap().set_isolated_monitoring(false);
                std::fs::remove_file(config_path.with_file_name("AGENTS.md")).unwrap();
                window.invoke_refresh_sync();
                assert!(!window.get_dual_enabled() && window.get_dual_deployed());
                let row = window.get_sync_files().row_data(1).unwrap();
                assert!(!row.to_secondary && row.to_primary, "A missing source or disabled monitoring changed the wrong action");
                assert!(window.get_sync_files().row_data(0).unwrap().to_secondary);
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
            }
            24 => {
                assert!(window.get_dual_scroll_y() > -10000.0 && window.get_dual_scroll_y() < 0.0,
                        "The Dual page scrolled beyond its actual content: {}", window.get_dual_scroll_y());
                window.invoke_sync_profile("instructions".into(), true);
                assert!(!window.get_sync_busy());
                assert!(window.get_sync_files().row_data(1).unwrap().error);
                window.invoke_sync_profile("instructions".into(), false);
            }
            25 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(1).unwrap();
                assert!(!row.error && row.backup_path.is_empty());
                assert_eq!(std::fs::read_to_string(config_path.with_file_name("AGENTS.md")).unwrap(), "Updated Dodex instructions\n");
                std::fs::write(&config_path, "api_key = 'synthetic-parser-secret'\n[broken =").unwrap();
                window.invoke_sync_profile("config".into(), true);
            }
            26 => {
                window.invoke_refresh_sync();
                if window.get_sync_busy() { return; }
                let row = window.get_sync_files().row_data(0).unwrap();
                assert!(row.error && row.backup_path.is_empty());
                assert!(!row.message.contains("synthetic-parser-secret") && !row.message.contains("api_key"));
                assert!(std::fs::read_to_string(&second_config_path).unwrap().contains("reverse-model"));
                window.set_dual_scroll_y(-330.0);
            }
            27 => {
                window.invoke_refresh_sync();
                assert!(window.get_sync_files().row_data(0).unwrap().error, "Refreshing file state erased the operation result");
                window.set_dual_scroll_y(-1000.0);
            }
            28 => {
                for (path, value) in [(&config_path, "primary-login"), (&second_config_path, "secondary-login")] {
                    assert_eq!(std::fs::read_to_string(path.with_file_name("auth.json")).unwrap(), value);
                    assert_eq!(std::fs::read_to_string(path.with_file_name("state.sqlite")).unwrap(), "database fixture");
                    assert_eq!(std::fs::read_to_string(path.with_file_name("session.log")).unwrap(), "log fixture");
                }
                assert_eq!(std::fs::read_to_string(second_config_path.with_file_name("AGENTS.override.md")).unwrap(), "Secondary override stays local\n");
                window.set_dual_advanced_open(false);
                window.set_dual_scroll_y(0.0);
                window.set_dual_tui_installed(true);
                window.set_dual_command_path("/Users/example/.local/bin/dodex".into());
                window.set_dual_package_path("/Users/example/dodex/packages/standalone/releases/0.160.0".into());
                window.set_dual_tui_available(false);
                window.set_dual_tui_configured(false);
                window.set_dual_message("Dodex TUI 包已安装；Companion 监控未启用。".into());
            }
            29 => {
                assert!(window.get_dual_tui_installed() && window.get_dual_deployed());
                assert!(!window.get_dual_enabled() && !window.get_dual_tui_available());
                assert!(!window.get_dual_command_path().is_empty());
                assert_eq!(window.get_dual_profile_home(), second_config_path.parent().unwrap().to_string_lossy().as_ref());
                window.set_dual_tui_available(true);
            }
            30 => {
                assert!(window.get_dual_tui_installed() && window.get_dual_tui_available());
                assert!(!window.get_dual_tui_configured(), "Discovery alone must not claim that installation completed");
                assert!(!window.get_dual_enabled(), "Detecting a CLI must not enable monitoring");
                window.set_dual_tui_configured(true);
                weak_editor.upgrade().unwrap().set_isolated_monitoring(true);
                window.set_dual_message("Companion 已接入副账号 CLI。".into());
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark);
            }
            31 => {
                assert!(window.get_dual_enabled() && window.get_dual_tui_available());
                assert!(window.get_dual_tui_configured());
                assert!(window.get_dual_tui_installed());
                assert_eq!(window.get_dual_command_path(), "/Users/example/.local/bin/dodex");
                window.set_software_versions(slint::ModelRc::new(slint::VecModel::from(vec![
                    ui::SoftwareVersion { name: "Codex TUI".into(), current: "0.159.3".into(), target: "0.160.0".into(), message: "目标：官方稳定版".into(), error: false },
                    ui::SoftwareVersion { name: "Dodex TUI".into(), current: "0.155.0-alpha.16.4".into(), target: "0.160.0".into(), message: "保留已有副账号会话与完整原生安装包".into(), error: false },
                ])));
                window.set_software_busy(true);
                window.set_software_message("正在下载并验证官方稳定版；所选 TUI 使用自己的版本，账号目录、SQLite、会话和日志均保持原样。现有终端会话保持运行。".into());
                window.set_dual_scroll_y(0.0);
            }
            32 => {
                assert!(window.get_software_busy());
                assert_eq!(window.get_software_versions().row_count(), 2);
                window.set_software_busy(false);
                window.set_software_error(true);
                window.set_software_message("操作未全部完成：另一个目标实例更新正在进行。请等待完成后重试。已完成的更新会保留，尚未更新的项目不会标记为最新。".into());
                window.set_software_notice("已有操作正在等待或进行中；重复请求已忽略。请等待结果后再操作。".into());
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
            }
            33 => {
                assert!(window.get_software_error() && !window.get_software_busy());
                window.set_settings_page(2);
                window.set_login_item_available(true);
                window.set_login_item_message("未开启；开启后会在登录此 Mac 时启动菜单栏应用。".into());
            }
            34 => {
                assert!(!window.get_run_at_login());
                // Exercise the real Switch, including its internal tentative
                // checked change. Failed writes must restore both directions;
                // later OS changes must retain the binding after a click.
                let requested = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let actual = std::rc::Rc::new(std::cell::Cell::new(false));
                let requests = requested.clone();
                let state = actual.clone();
                let settings = window.as_weak();
                window.on_set_run_at_login(move |enabled| {
                    let settings = settings.upgrade().unwrap();
                    assert_eq!(settings.get_run_at_login(), enabled);
                    requests.borrow_mut().push(enabled);
                    settings.set_run_at_login(state.get());
                });
                // The fixture uses the minimum 820×720 logical layout here.
                let click_switch = || {
                    click_control(
                        &window, "登录时自动启动",
                        slint::private_unstable_api::re_exports::AccessibleRole::Switch,
                    );
                };
                click_switch();
                assert!(!window.get_run_at_login());
                click_switch();
                assert_eq!(*requested.borrow(), vec![true, true]);
                actual.set(true);
                window.set_run_at_login(true);
                click_switch();
                assert!(window.get_run_at_login());
                assert_eq!(*requested.borrow(), vec![true, true, false]);
                click_switch();
                assert_eq!(*requested.borrow(), vec![true, true, false, false]);
                window.set_run_at_login(true);
                window.set_login_item_message("已开启；登录此 Mac 时自动启动 Agent Companion。".into());
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Dark);
            }
            35 => {
                assert!(window.get_run_at_login());
                window.set_run_at_login(false);
                window.set_login_item_requires_approval(true);
                window.set_login_item_error(true);
                window.set_login_item_message("无法开启登录时自动启动：Operation not permitted（SMAppServiceErrorDomain / 1）。\n尚未生效；请在系统登录项设置中允许 Agent Companion。".into());
                window.global::<ui::Palette>().set_color_scheme(ColorScheme::Light);
            }
            36 => {
                assert!(!window.get_run_at_login() && window.get_login_item_requires_approval());
                window.set_settings_page(1);
                window.set_dual_advanced_open(false);
                window.set_dual_scroll_y(0.0);
                window.set_dual_enabled(false);
                window.set_dual_deployed(false);
                window.set_dual_tui_installed(false);
                window.set_dual_tui_available(false);
                window.set_dual_tui_configured(false);
                window.set_dual_busy(false);
                window.set_dual_error(false);
                window.set_dual_message("".into());
                window.set_software_versions(slint::ModelRc::new(slint::VecModel::from(vec![
                    ui::SoftwareVersion { name: "Codex TUI".into(), current: "0.160.1".into(), target: "Homebrew".into(), message: "".into(), error: false },
                    ui::SoftwareVersion { name: "Dodex TUI".into(), current: "0.159.3".into(), target: "官方 standalone".into(), message: "".into(), error: false },
                ])));
                window.set_software_busy(false);
                window.set_software_error(false);
                window.set_software_message("".into());
                window.set_software_notice("".into());
            }
            37 => {
                assert!(!window.get_dual_advanced_open());
                assert!(!window.get_dual_tui_installed() && !window.get_dual_tui_configured());
                // Real pointer events at the minimum 820×720 layout verify
                // forwarding through MacSettings → DualSettings → root.
                click("安装独立 Dodex TUI 与终端入口");
                assert_eq!(setup_requests.get(), 1);
                click("仅更新 Dodex TUI，保留 Codex 版本");
                assert!(maintenance_requests.borrow().is_empty(), "Update requires a configured TUI");
                window.set_dual_tui_installed(true);
                window.set_dual_tui_available(true);
                window.set_dual_error(true);
                window.set_dual_message("Dodex TUI 包已完成，TUI 入口配置失败。已完成内容保留，修正后继续安装并配置。".into());
            }
            38 => {
                assert!(window.get_dual_tui_installed() && window.get_dual_tui_available());
                assert!(!window.get_dual_tui_configured() && window.get_dual_error());
                click("修复独立 Dodex TUI 安装与终端入口");
                assert_eq!(setup_requests.get(), 2, "A partial installation must allow retry");
                click("仅更新 Dodex TUI，保留 Codex 版本");
                assert!(maintenance_requests.borrow().is_empty(), "The incomplete command is not a configured TUI entry");
                window.set_dual_tui_configured(true);
                window.set_dual_enabled(true);
                window.set_dual_error(false);
                window.set_dual_message("安装与配置已完成。打开终端后，请自行登录第二个账号。".into());
            }
            39 => {
                assert!(window.get_dual_tui_configured() && window.get_dual_enabled());
                assert!(!window.get_dual_error(), "Waiting for the user's login is a successful installation state");
                click("在终端中打开 Dodex TUI");
                assert_eq!(open_requests.get(), 1);
                click("仅更新 Dodex TUI，保留 Codex 版本");
                click("仅更新 Codex TUI，保留 Dodex 版本");
                assert_eq!(*maintenance_requests.borrow(), ["update-dodex", "update-codex"]);
                click("显示双开高级设置");
                assert!(window.get_dual_advanced_open(), "Advanced controls must be reachable through the visible button");
                click("收起双开高级设置");
                assert!(!window.get_dual_advanced_open());
                window.set_software_busy(true);
                window.set_software_message("正在更新 Dodex TUI，保留副账号配置与会话…".into());
            }
            40 => {
                assert!(window.get_software_busy() && !window.get_dual_busy(), "Update must not masquerade as installation progress");
                click("修复独立 Dodex TUI 安装与终端入口");
                click("在终端中打开 Dodex TUI");
                click("仅更新 Dodex TUI，保留 Codex 版本");
                assert_eq!(setup_requests.get(), 2, "Installing is disabled while update owns the maintenance lock");
                assert_eq!(open_requests.get(), 1);
                assert_eq!(*maintenance_requests.borrow(), ["update-dodex", "update-codex"], "Repeated update clicks must be disabled");
                window.set_software_busy(false);
                window.set_software_message("".into());
                window.set_dual_advanced_open(true);
            }
            41 => {
                assert!(window.get_dual_advanced_open());
                window.set_dual_advanced_open(false);
                window.set_macos_preferences(false);
                window.set_windows_preferences(true);
                window.set_settings_page(3);
                window.set_dual_scroll_y(0.0);
                window.window().set_size(slint::LogicalSize::new(820.0, 720.0));
            }
            42 => {
                // Switching the complete platform layout and default font can
                // need a native redraw after the resize event; let it settle.
                assert!(window.get_windows_preferences() && !window.get_macos_preferences());
            }
            _ => {
                assert!(window.get_windows_preferences() && !window.get_macos_preferences());
                assert!(window.get_dual_tui_installed() && window.get_dual_tui_configured());
                completed_check.set(true);
                slint::quit_event_loop().unwrap();
            }
        }
        eprintln!("[editor-fixture] phase {phase} completed at {:?}", started.elapsed());
        // Busy sync retries return above. Only a completed phase earns a new
        // deadline, so a responsive UI cannot conceal a stuck file operation.
        phase_progress.send(Some(phase)).unwrap();
        phase += 1;
    });
    slint::run_event_loop().unwrap();
    assert!(
        completed.get(),
        "The editor event loop exited before all scenarios completed"
    );
    finished.send(None).unwrap();
    watchdog.join().unwrap();
    println!(
        "PASS: opaque AppKit editor; two main dual-instance actions with fresh/partial/ready/update states and collapsed advanced settings on macOS and Windows layouts; login-item states and isolated callbacks; separate status-bar drafts; manual config/AGENTS sync with backups and dirty/busy guards; all file writes stayed in isolated fixtures"
    );
}
