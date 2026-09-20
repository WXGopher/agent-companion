//! Render the compact panel at common scales and exercise its full-details link.

use std::{cell::RefCell, rc::Rc};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

struct TestPlatform(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);

thread_local! {
    static RENDER_TIME: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

impl Platform for TestPlatform {
    fn duration_since_start(&self) -> std::time::Duration {
        std::time::Duration::from_millis(RENDER_TIME.get())
    }

    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(window.clone());
        Ok(window)
    }
}

fn draw(window: &MinimalSoftwareWindow, name: &str) -> Vec<Rgb8Pixel> {
    RENDER_TIME.set(RENDER_TIME.get() + 250);
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut rendered = false;
    let mut pixels = Vec::new();
    window.draw_if_needed(|renderer| {
        let size = window.size();
        let mut buffer = vec![Rgb8Pixel::default(); (size.width * size.height) as usize];
        renderer.render(&mut buffer, size.width as usize);
        assert!(buffer.iter().any(|pixel| pixel.r != pixel.b));
        if let Some(dir) = agent_companion_core::compat::var_os("AGENT_COMPANION_RENDER_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
            bytes.extend(buffer.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
            std::fs::write(dir.join(format!("{name}.ppm")), bytes).unwrap();
        }
        rendered = true;
        pixels = buffer;
    });
    assert!(rendered);
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
fn waiting_preview_renders_and_opens_full_details_at_common_scales() {
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(TestPlatform(windows.clone()))).unwrap();
    let panel = super::ui::FlyoutWindow::new().unwrap();
    panel.set_compact(true);
    panel.set_waiting_total(8);
    let expanded = Rc::new(std::cell::Cell::new(false));
    panel.on_expand({
        let expanded = expanded.clone();
        move || expanded.set(true)
    });
    panel.show().unwrap();
    let window = windows.borrow().last().unwrap().clone();
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        for count in [1, super::flyout::PEEK_LIMIT] {
            let rows = (0..count)
                .map(|n| super::ui::SessionRow {
                    id: format!("session-{n}").into(),
                    title: if n % 2 == 0 {
                        "agent-companion · Update background notifications".into()
                    } else {
                        "项目 · 等待确认终端命令".into()
                    },
                    detail: "Waiting for permission · shell".into(),
                    source: if n % 2 == 0 {
                        "codex".into()
                    } else {
                        "claude".into()
                    },
                    phase: "waitingForApproval".into(),
                    jumpable: true,
                })
                .collect::<Vec<_>>();
            panel.set_sessions(ModelRc::new(VecModel::from(rows)));
            let height = super::flyout::peek_height(count);
            panel
                .window()
                .set_size(slint::LogicalSize::new(super::FLYOUT_WIDTH, height));
            draw(&window, &format!("preview-{count}-{scale}"));
            assert!(
                window
                    .size()
                    .height
                    .abs_diff((height * scale).round() as u32)
                    <= 1
            );
            expanded.set(false);
            click(&window, 120.0, height - 24.0);
            assert!(
                expanded.get(),
                "full-details link must remain clickable at {scale}x with {count} rows"
            );
        }
    }
    panel.hide().unwrap();

    flyout_pages_render_and_preserve_scroll(&panel, &windows);

    let card = super::ui::CardWindow::new().unwrap();
    card.set_card(3);
    card.set_card_source("codex".into());
    card.set_card_title("agent-companion · Codex".into());
    card.set_card_tool("Question".into());
    card.set_form_progress("1 / 3 · 实现范围".into());
    card.set_form_question(
        "这次优先实现哪些能力？选项说明和自由文本都应完整显示，较长内容可以滚动查看。".into(),
    );
    card.set_form_free_text(true);
    card.set_form_options(ModelRc::new(VecModel::from(vec![
        super::ui::FormOption {
            label: "Windows Terminal".into(),
            description: "返回对应窗口、隐藏标签页和原来的分屏。".into(),
            selected: true,
        },
        super::ui::FormOption {
            label: "Codex desktop".into(),
            description: "Open the exact local conversation in the desktop app.".into(),
            selected: false,
        },
    ])));
    card.set_form_text("暂时只支持 Codex\n保留当前终端的操作方式。".into());
    card.set_form_can_next(true);
    card.window().set_size(slint::LogicalSize::new(
        super::cardview::CARD_WIDTH,
        super::cardview::card_height(super::cardview::CardKind::Form),
    ));
    card.show().unwrap();
    let window = windows.borrow().last().unwrap().clone();
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        card.window().set_size(slint::LogicalSize::new(
            super::cardview::CARD_WIDTH,
            super::cardview::card_height(super::cardview::CardKind::Form),
        ));
        draw(&window, &format!("codex-question-{scale}"));
    }
    card.set_form_options(ModelRc::default());
    card.set_form_secret(true);
    card.set_form_text("hidden secret text".into());
    draw(&window, "codex-secret-question");
    card.hide().unwrap();
    codex_tui_editor_renders_and_applies_only_explicit_actions(&windows);
}

fn codex_tui_editor_renders_and_applies_only_explicit_actions(
    windows: &Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
) {
    use agent_companion_core::install::codex_tui as config;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# keep my config\n[features]\nhooks = true\n[tui]\ntheme = \"nord\"\n";
    std::fs::write(&path, original).unwrap();
    let editor = super::codex_tui::Editor::new_isolated(path.clone()).unwrap();
    editor.window.set_windows_preferences(true);
    editor
        .window
        .set_window_title("Agent Companion · Settings".into());
    editor.window.set_claude_status("Not installed".into());
    editor
        .window
        .set_codex_status("8 of 8 hooks installed".into());
    editor.window.set_codex_installed(true);
    editor
        .window
        .set_taskbar_status("Sitting in the taskbar, above the notification area.".into());
    editor.show().unwrap();
    // The displayed path is illustrative in the screenshots; all writes still
    // go to the explicit temporary path held by the controller.
    editor.window.set_config_path("~/.codex/config.toml".into());
    let window = windows.borrow().last().unwrap().clone();
    assert_eq!(editor.window.get_selected_count(), 3);
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    assert!(!editor.window.get_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        for (width, height) in [(820.0, 720.0), (720.0, 640.0)] {
            editor
                .window
                .window()
                .set_size(slint::LogicalSize::new(width, height));
            draw(&window, &format!("codex-tui-default-{width}-{scale}"));
            click(&window, 49.0, 339.0);
            assert_eq!(
                editor.window.get_selected_count(),
                2,
                "checkbox at {width}px / {scale}x"
            );
            assert!(editor.window.get_dirty());
            window.dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
            window.dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
            assert_eq!(
                editor.window.get_selected_count(),
                3,
                "checkbox retains keyboard focus after toggling"
            );
            assert!(!editor.window.get_dirty());
        }
    }
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    editor
        .window
        .window()
        .set_size(slint::LogicalSize::new(820.0, 720.0));
    editor
        .window
        .invoke_toggle("model-with-reasoning".into(), false);
    assert!(editor.window.get_dirty());
    assert!(!editor.window.get_preview().contains("gpt-6-astra"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    editor.window.invoke_toggle("git-branch".into(), true);
    let pixels = draw(&window, "codex-tui-draft");
    let checked_blue = (334..344)
        .flat_map(|y| (44..54).map(move |x| y * 820 + x))
        .filter(|index| pixels[*index].b as i16 - pixels[*index].r as i16 > 60)
        .count();
    assert_eq!(
        checked_blue, 0,
        "the checkbox must repaint when the draft deselects it"
    );
    let draft = editor.window.get_preview();
    for scheme in [
        slint::language::ColorScheme::Light,
        slint::language::ColorScheme::Dark,
    ] {
        editor
            .window
            .global::<super::ui::Palette>()
            .set_color_scheme(scheme);
        for page in [1, 2, 0] {
            click(&window, [64.0, 162.0, 265.0][page as usize], 88.0);
            assert_eq!(editor.window.get_settings_page(), page);
            draw(&window, &format!("settings-page-{page}-{scheme:?}"));
            assert!(editor.window.get_dirty());
            assert_eq!(editor.window.get_preview(), draft);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
    click(&window, 743.0, 655.0);
    assert!(
        !editor.window.get_error(),
        "{}",
        editor.window.get_message()
    );
    assert!(!editor.window.get_dirty());
    assert_eq!(
        config::read(&path).unwrap().visible_items(),
        vec!["current-dir", "thread-name", "git-branch"]
    );
    for component in config::COMPONENTS {
        editor.window.invoke_toggle(component.id.into(), false);
    }
    assert_eq!(editor.window.get_selected_count(), 0);
    draw(&window, "codex-tui-hidden");
    editor.window.invoke_apply();
    assert_eq!(config::read(&path).unwrap().items, Some(vec![]));
    click(&window, 618.0, 655.0);
    assert_eq!(config::read(&path).unwrap().items, None);
    assert!(!editor.window.get_custom());
    assert_eq!(editor.window.get_selected_count(), 3);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("theme = \"nord\"")
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("hooks = true")
    );
    for component in config::COMPONENTS {
        editor.window.invoke_toggle(component.id.into(), true);
    }
    draw(&window, "codex-tui-all");
    let external = "[tui]\nstatus_line = [\"future-component\", \"model\"]\n";
    std::fs::write(&path, external).unwrap();
    editor.window.invoke_apply();
    assert!(editor.window.get_error());
    assert!(editor.window.get_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
    draw(&window, "codex-tui-conflict");
    editor.window.hide().unwrap();
    editor.show().unwrap();
    assert!(!editor.window.get_error());
    assert_eq!(editor.window.get_selected_count(), 2);
    assert!(editor.window.get_preview().contains("[future-component]"));
    let secondary = dir.path().join("dodex.toml");
    std::fs::write(&secondary, "[tui]\nstatus_line = ['model']\n").unwrap();
    editor.add_isolated_secondary(secondary.clone());
    editor.window.invoke_select_instance("Dodex".into());
    editor.window.invoke_toggle("git-branch".into(), true);
    let secondary_draft = editor.window.get_preview();
    editor.window.invoke_select_instance("Codex".into());
    assert!(editor.window.get_preview().contains("[future-component]"));
    editor.window.invoke_select_instance("Dodex".into());
    assert_eq!(editor.window.get_preview(), secondary_draft);
    editor.window.invoke_apply();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
    assert_eq!(
        config::read(&secondary).unwrap().visible_items(),
        vec!["model", "git-branch"]
    );
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        window.set_size(slint::PhysicalSize::new(
            (820.0 * scale) as u32,
            (720.0 * scale) as u32,
        ));
        editor.window.set_settings_page(0);
        draw(&window, &format!("windows-dual-editor-{scale}"));
        editor
            .window
            .set_dual_message("环境已就绪。打开 Dodex 后，请使用第二个账号登录。".into());
        editor.window.set_dual_deployed(true);
        editor.window.set_settings_page(3);
        draw(&window, &format!("windows-dual-settings-{scale}"));
    }
    editor.window.hide().unwrap();
}

fn flyout_pages_render_and_preserve_scroll(
    panel: &super::ui::FlyoutWindow,
    windows: &Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
) {
    panel.set_compact(false);
    panel.set_active_count(20);
    panel.set_finished_count(2);
    panel.set_sessions(ModelRc::new(VecModel::from(
        (0..20)
            .map(|n| super::ui::SessionRow {
                id: format!("session-{n}").into(),
                title: "项目 · Windows settings and usage".into(),
                detail: "Working · review changes".into(),
                phase: "running".into(),
                source: "codex".into(),
                jumpable: true,
            })
            .collect::<Vec<_>>(),
    )));
    panel.set_subscription(super::ui::SubscriptionView {
        updated: "Updated 14:32".into(),
        has_reading: true,
        has_tokens: true,
        total: "12.3M".into(),
        total_exact: "12345678".into(),
        peak: "1.2M".into(),
        peak_exact: "1234567".into(),
        current_streak: "8 days".into(),
        longest_streak: "24 days".into(),
        longest_turn: "1h 12m".into(),
        ..Default::default()
    });
    panel.set_subscription_limits(ModelRc::new(VecModel::from(vec![
        super::ui::UsageRow {
            heading: true,
            label: "Codex · plus".into(),
            ..Default::default()
        },
        super::ui::UsageRow {
            label: "5 hours".into(),
            value: "81%".into(),
            fill: 0.81,
            tier: "good".into(),
            resets: "Resets 18:30".into(),
            ..Default::default()
        },
        super::ui::UsageRow {
            label: "Weekly".into(),
            value: "34%".into(),
            fill: 0.34,
            tier: "warn".into(),
            resets: "Resets Mon 00:40".into(),
            ..Default::default()
        },
    ])));
    panel.set_usage_days(ModelRc::new(VecModel::from(
        (1..=7)
            .map(|day| super::ui::UsageDay {
                date: format!("2026-09-{day:02}").into(),
                label: format!("{day:02}").into(),
                value: format!("{day}00K").into(),
                exact: format!("{day}00000").into(),
                fill: day as f32 / 7.0,
            })
            .collect::<Vec<_>>(),
    )));
    panel.show().unwrap();
    let window = windows.borrow().last().unwrap().clone();
    let selected = Rc::new(std::cell::Cell::new(false));
    panel.on_select_page({
        let selected = selected.clone();
        move |usage| selected.set(usage)
    });
    let refreshed = Rc::new(std::cell::Cell::new(0));
    panel.on_refresh_usage({
        let refreshed = refreshed.clone();
        move || refreshed.set(refreshed.get() + 1)
    });
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        for height in [super::DETAIL_HEIGHT, 360.0] {
            panel
                .window()
                .set_size(slint::LogicalSize::new(super::DETAIL_WIDTH, height));
            panel.set_usage_page(false);
            panel.set_task_scroll_y(0.0);
            panel.set_usage_scroll_y(0.0);
            draw(&window, &format!("tasks-{height}-{scale}"));
            let size = window.size();
            panel.set_task_scroll_y(-120.0);
            draw(&window, "tasks-scrolled");
            click(&window, 116.0, 28.0);
            assert!(
                selected.get() && panel.get_usage_page(),
                "Usage tab must be clickable"
            );
            draw(&window, &format!("usage-{height}-{scale}"));
            panel.set_usage_scroll_y(-100.0);
            draw(&window, &format!("usage-scrolled-{height}-{scale}"));
            click(&window, 42.0, 28.0);
            draw(&window, "tasks-return");
            assert!(!selected.get() && !panel.get_usage_page());
            assert_eq!(panel.get_task_scroll_y(), -120.0);
            click(&window, 116.0, 28.0);
            draw(&window, "usage-return");
            assert_eq!(panel.get_usage_scroll_y(), -100.0);
            assert_eq!(
                window.size(),
                size,
                "switching pages must not resize the panel"
            );
            let before = refreshed.get();
            click(&window, 330.0, 80.0);
            assert_eq!(refreshed.get(), before + 1);
        }
    }
    if agent_companion_core::compat::var_os("AGENT_COMPANION_RENDER_DIR").is_some() {
        render_readme_flyout(panel, windows);
    }
    let mut subscription = panel.get_subscription();
    subscription.loading = true;
    panel.set_subscription(subscription);
    draw(&window, "usage-loading");
    let before = refreshed.get();
    click(&window, 330.0, 80.0);
    assert_eq!(
        refreshed.get(),
        before,
        "loading disables duplicate refreshes"
    );
    panel.set_subscription(super::ui::SubscriptionView {
        message: "Sign in to Codex with your ChatGPT subscription, then refresh. Subscription usage does not use an API key.".into(),
        ..Default::default()
    });
    draw(&window, "usage-signed-out");
    panel.set_dual_enabled(true);
    panel.set_secondary_selected(true);
    panel.set_instance_quotas(ModelRc::new(VecModel::from(vec![
        super::ui::UsageRow {
            agent: "codex".into(),
            label: "Codex".into(),
            value: "82%".into(),
            tier: "good".into(),
            resets: "Week · resets Tue 09:00".into(),
            ..Default::default()
        },
        super::ui::UsageRow {
            agent: "dodex".into(),
            label: "Dodex".into(),
            value: "27%".into(),
            tier: "warn".into(),
            resets: "Week · resets Fri 18:00".into(),
            ..Default::default()
        },
    ])));
    let instance = Rc::new(std::cell::Cell::new(false));
    panel.on_select_instance({
        let instance = instance.clone();
        move |secondary| instance.set(secondary)
    });
    for scale in [1.0, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        panel.window().set_size(slint::LogicalSize::new(
            super::DETAIL_WIDTH,
            super::DETAIL_HEIGHT,
        ));
        panel.set_usage_page(false);
        panel.set_task_scroll_y(0.0);
        draw(&window, &format!("windows-dual-tasks-{scale}"));
        click(&window, 120.0, 195.0);
        assert!(instance.get(), "Dodex quota card opens its own account");
        panel.set_usage_page(true);
        panel.set_usage_scroll_y(0.0);
        draw(&window, &format!("windows-dual-usage-{scale}"));
        click(&window, 42.0, 80.0);
        assert!(
            !instance.get(),
            "Codex account remains independently selectable"
        );
    }
    panel.hide().unwrap();
}

fn render_readme_flyout(
    source: &super::ui::FlyoutWindow,
    windows: &Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
) {
    let panel = super::ui::FlyoutWindow::new().unwrap();
    panel.set_active_count(4);
    panel.set_finished_count(2);
    panel.set_sessions(ModelRc::new(VecModel::from(
        [
            (
                "Website · Polish the settings page",
                "Working · editing components",
                "running",
            ),
            (
                "CLI · Run regression tests",
                "Waiting for approval · run tests",
                "waitingForApproval",
            ),
            (
                "Docs · Update the installation guide",
                "Working · reviewing changes",
                "running",
            ),
            (
                "Dashboard · Add usage filters",
                "Waiting for input · choose a date range",
                "waitingForAnswer",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (title, detail, phase))| super::ui::SessionRow {
            id: format!("example-{n}").into(),
            title: title.into(),
            detail: detail.into(),
            phase: phase.into(),
            source: "codex".into(),
            jumpable: true,
        })
        .collect::<Vec<_>>(),
    )));
    panel.set_usage_rows(source.get_subscription_limits());
    panel.set_subscription(source.get_subscription());
    panel.set_subscription_limits(source.get_subscription_limits());
    panel.set_usage_days(source.get_usage_days());
    panel.show().unwrap();
    let window = windows.borrow().last().unwrap().clone();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    panel.window().set_size(slint::LogicalSize::new(
        super::DETAIL_WIDTH,
        super::DETAIL_HEIGHT,
    ));
    draw(&window, "readme-tasks");
    panel.set_usage_page(true);
    panel.set_usage_scroll_y(-170.0);
    draw(&window, "readme-usage");
    panel.hide().unwrap();
}
