//! Settings feedback for explicit release checks. The service never installs
//! anything; only the separate release-link callback can open a browser.
use std::{cell::Cell, io, rc::Rc, time::Duration};

use slint::ComponentHandle;

use crate::ui::CodexTuiWindow;
use crate::update_service::{ManualCheck, UpdateService};

pub struct Controller {
    // Retain the timer for the lifetime of the settings window.
    _poll: slint::Timer,
}

impl Controller {
    pub fn bind(window: &CodexTuiWindow, service: UpdateService) -> Self {
        Self::with_opener(window, service, open_release)
    }

    pub(crate) fn with_opener(
        window: &CodexTuiWindow,
        service: UpdateService,
        open: impl Fn(&str) -> io::Result<()> + 'static,
    ) -> Self {
        let open_failed = Rc::new(Cell::new(false));
        render(window, &service.manual_snapshot(), false);
        window.on_check_updates({
            let service = service.clone();
            let window = window.as_weak();
            let open_failed = open_failed.clone();
            move || {
                open_failed.set(false);
                service.check_now(agent_companion_core::now_unix_secs());
                if let Some(window) = window.upgrade() {
                    render(&window, &service.manual_snapshot(), false);
                }
            }
        });
        window.on_view_release({
            let service = service.clone();
            let window = window.as_weak();
            let open_failed = open_failed.clone();
            move || {
                let snapshot = service.manual_snapshot();
                // Read the result again at the action boundary. A stale UI
                // link cannot open a cached release during a newer request.
                if let ManualCheck::Available(update) = &snapshot
                    && let Some(url) = &update.release_url
                {
                    open_failed.set(open(url).is_err());
                    if let Some(window) = window.upgrade() {
                        render(&window, &snapshot, open_failed.get());
                    }
                }
            }
        });
        let poll = slint::Timer::default();
        let weak = window.as_weak();
        poll.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(150),
            move || {
                if let Some(window) = weak.upgrade()
                    && window.window().is_visible()
                {
                    render(&window, &service.manual_snapshot(), open_failed.get());
                }
            },
        );
        Self { _poll: poll }
    }
}

fn render(window: &CodexTuiWindow, snapshot: &ManualCheck, open_failed: bool) {
    window.set_update_checking(matches!(snapshot, ManualCheck::Checking));
    window.set_update_available(matches!(snapshot, ManualCheck::Available(_)));
    window.set_update_failed(matches!(snapshot, ManualCheck::Failed) || open_failed);
    let message = if open_failed {
        "无法打开浏览器，请重试“查看 GitHub Release”。".to_owned()
    } else {
        match snapshot {
            ManualCheck::Idle => String::new(),
            ManualCheck::Disabled => format!(
                "本地版本 v{}，已关闭更新检查。",
                env!("AGENT_COMPANION_VERSION")
            ),
            ManualCheck::Checking => "正在检查 GitHub 最新版本…".into(),
            ManualCheck::UpToDate => "当前已是最新版本。".into(),
            ManualCheck::Available(update) => format!(
                "发现新版本 v{}，可前往 GitHub Release 查看和下载。",
                update.latest_version.as_deref().unwrap_or_default()
            ),
            // Do not expose response bodies, network diagnostics or paths.
            ManualCheck::Failed => "检查更新失败，请稍后重试。".into(),
        }
    };
    window.set_update_message(message.into());
}

/// Call only from an explicit release-link action with a service-validated URL.
pub(crate) fn open_release(url: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{PCWSTR, w};

        let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(url.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return Err(io::Error::other("Could not open release page"));
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // Pass one argument directly; no shell interprets the URL.
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
}
