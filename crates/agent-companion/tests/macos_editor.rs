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

    // The fixture has 29 validated phases. The watchdog measures a stalled
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
        assert!(window.get_macos_preferences());
        assert!(window.get_ready());
        assert!((0..=1).contains(&window.get_settings_page()));
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
                window.set_dual_scroll_y(-1000.0);
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
                assert!(window.get_dual_scroll_y() > -1000.0 && window.get_dual_scroll_y() < 0.0,
                        "The Dual page scrolled beyond its actual content");
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
            _ => {
                for (path, value) in [(&config_path, "primary-login"), (&second_config_path, "secondary-login")] {
                    assert_eq!(std::fs::read_to_string(path.with_file_name("auth.json")).unwrap(), value);
                    assert_eq!(std::fs::read_to_string(path.with_file_name("state.sqlite")).unwrap(), "database fixture");
                    assert_eq!(std::fs::read_to_string(path.with_file_name("session.log")).unwrap(), "log fixture");
                }
                assert_eq!(std::fs::read_to_string(second_config_path.with_file_name("AGENTS.override.md")).unwrap(), "Secondary override stays local\n");
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
        "PASS: opaque AppKit editor; two menu-only settings sections, light/dark/minimum-size layouts, separate status-bar drafts; manual bidirectional config/AGENTS sync with backups, no-op/error feedback, disabled-monitoring and missing-target support, override warnings, dirty/busy guards and retained selection; all file writes stayed in isolated fixtures"
    );
}
