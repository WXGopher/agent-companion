//! Windows panel update state. Rendering only reads snapshots; only a click
//! may ask the worker to download, and only an initialized helper may quit us.

use std::cell::{Cell, RefCell};

use crate::update_service::UpdateSnapshot;
use crate::windows_update::{Manager, Snapshot, Stage};

pub(super) trait Backend {
    fn snapshot(&self) -> Snapshot;
    fn start(&self, release: &UpdateSnapshot) -> Result<(), String>;
    fn take_restart_ready(&self) -> bool;
}

impl Backend for Manager {
    fn snapshot(&self) -> Snapshot {
        self.snapshot()
    }

    fn start(&self, release: &UpdateSnapshot) -> Result<(), String> {
        self.start(release)
    }

    fn take_restart_ready(&self) -> bool {
        self.take_restart_ready()
    }
}

pub(super) struct Controller<B: Backend = Manager> {
    backend: B,
    enabled: bool,
    start_error: RefCell<Option<String>>,
    restart_error: RefCell<Option<String>>,
    restart_dispatched: Cell<bool>,
}

impl Controller {
    pub fn new() -> Self {
        Self::with_backend(Manager::new(), !cfg!(companion_local_build))
    }
}

impl<B: Backend> Controller<B> {
    fn with_backend(backend: B, enabled: bool) -> Self {
        Self {
            backend,
            enabled,
            start_error: RefCell::new(None),
            restart_error: RefCell::new(None),
            restart_dispatched: Cell::new(false),
        }
    }

    pub fn start(&self, release: &UpdateSnapshot) {
        if !self.enabled || self.backend.snapshot().stage.is_busy() {
            return;
        }
        if !has_release(release) {
            *self.start_error.borrow_mut() =
                Some("未找到可安装的新版本。请在 Settings 中点击“检查更新”后重试。".into());
            return;
        }
        self.start_error.borrow_mut().take();
        self.restart_error.borrow_mut().take();
        self.restart_dispatched.set(false);
        // The backend sets its busy stage before returning, closing the gap
        // between two pointer events as well as guarding programmatic invokes.
        if let Err(error) = self.backend.start(release) {
            *self.start_error.borrow_mut() =
                Some(format!("无法开始更新：{error}\n请按错误说明处理后重试。"));
        }
    }

    /// Called by the app's unconditional housekeeping timer, including while
    /// the panel is hidden. The caller quits its own Slint event loop once.
    pub fn take_restart_ready(&self) -> bool {
        if !self.enabled {
            return false;
        }
        // Keep polling after dispatch as well: if quitting failed, the backend
        // must observe a helper timeout even while the panel remains hidden.
        let snapshot = self.backend.snapshot();
        if self.restart_dispatched.get()
            || snapshot.stage != Stage::Restarting
            || !self.backend.take_restart_ready()
        {
            return false;
        }
        self.restart_dispatched.set(true);
        true
    }

    pub fn restart_failed(&self, error: &str) {
        *self.restart_error.borrow_mut() = Some(format!(
            "更新已准备好，但无法自动退出：{error}\n请从任务栏菜单正常退出 Agent Companion，以完成更新。"
        ));
    }

    pub fn presentation(&self, release: &UpdateSnapshot) -> Presentation {
        let mut view = Presentation::default();
        if !self.enabled {
            return view;
        }
        let snapshot = self.backend.snapshot();
        if has_release(release) {
            view.version = release
                .latest_version
                .as_deref()
                .unwrap_or_default()
                .trim_start_matches('v')
                .into();
            view.available = true;
        }
        view.busy = snapshot.stage.is_busy();
        view.label = match snapshot.stage {
            Stage::Idle => "一键更新",
            Stage::Downloading => "下载中…",
            Stage::Verifying => "校验中…",
            Stage::Preparing => "准备中…",
            Stage::Restarting => "重启中…",
            Stage::Failed => "重试更新",
        }
        .into();
        view.message = match snapshot.stage {
            Stage::Idle => String::new(),
            Stage::Downloading => {
                let downloaded = snapshot.downloaded as f64 / (1024.0 * 1024.0);
                if let Some(total) = snapshot.total.filter(|total| *total > 0) {
                    let fraction = snapshot.downloaded.min(total) as f64 / total as f64;
                    view.progress = Some(fraction as f32);
                    format!(
                        "正在下载更新 · {downloaded:.1} / {:.1} MiB · {:.0}%\n可关闭面板，下载会继续。",
                        total as f64 / (1024.0 * 1024.0),
                        fraction * 100.0,
                    )
                } else {
                    format!("正在下载更新 · {downloaded:.1} MiB\n可关闭面板，下载会继续。")
                }
            }
            Stage::Verifying => "正在校验下载文件…".into(),
            Stage::Preparing => "正在准备更新，随后将自动重启 Agent Companion…".into(),
            Stage::Restarting => "更新已就绪，正在重启 Agent Companion…".into(),
            Stage::Failed => {
                view.failed = true;
                format!(
                    "更新失败：{}\n{}",
                    snapshot.message,
                    if view.available {
                        "请点击“错误详情”查看完整说明，处理后重试。"
                    } else {
                        "请点击“错误详情”查看说明，并在 Settings 中检查更新后重试。"
                    }
                )
            }
        };
        if let Some(error) = self.start_error.borrow().as_ref() {
            view.message = error.clone();
            view.failed = true;
            view.label = "重试更新".into();
        }
        if snapshot.stage != Stage::Failed
            && let Some(error) = self.restart_error.borrow().as_ref()
        {
            view.message = error.clone();
            view.failed = true;
        }
        view.available &= !view.busy;
        view
    }
}

fn has_release(release: &UpdateSnapshot) -> bool {
    release
        .latest_version
        .as_ref()
        .is_some_and(|v| !v.is_empty())
        && release.release_url.as_ref().is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Default, PartialEq)]
pub(super) struct Presentation {
    version: String,
    label: String,
    message: String,
    available: bool,
    busy: bool,
    failed: bool,
    progress: Option<f32>,
}

impl Presentation {
    pub fn apply(&self, panel: &super::ui::FlyoutWindow) {
        panel.set_update_version(self.version.as_str().into());
        panel.set_update_action_label(self.label.as_str().into());
        panel.set_update_action_available(self.available);
        panel.set_update_busy(self.busy);
        panel.set_update_message(self.message.as_str().into());
        panel.set_update_failed(self.failed);
        panel.set_update_progress_visible(self.progress.is_some());
        panel.set_update_progress(self.progress.unwrap_or_default());
        if !self.failed {
            panel.set_update_details_open(false);
            panel.set_update_details_scroll_y(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeBackend {
        snapshot: RefCell<Snapshot>,
        starts: Cell<usize>,
        rejection: RefCell<Option<String>>,
        ready: Cell<bool>,
        readiness_polls: Cell<usize>,
    }

    impl Backend for FakeBackend {
        fn snapshot(&self) -> Snapshot {
            self.snapshot.borrow().clone()
        }

        fn start(&self, _: &UpdateSnapshot) -> Result<(), String> {
            self.starts.set(self.starts.get() + 1);
            if let Some(error) = self.rejection.borrow_mut().take() {
                return Err(error);
            }
            self.snapshot.borrow_mut().stage = Stage::Downloading;
            Ok(())
        }

        fn take_restart_ready(&self) -> bool {
            self.readiness_polls.set(self.readiness_polls.get() + 1);
            // Deliberately keep returning true: the controller must dispatch
            // only once even if an implementation repeats its ready signal.
            self.ready.get()
        }
    }

    fn release() -> UpdateSnapshot {
        UpdateSnapshot {
            latest_version: Some("v0.4.2".into()),
            release_url: Some(
                "https://github.com/WXGopher/agent-companion/releases/tag/v0.4.2".into(),
            ),
        }
    }

    #[test]
    fn rendering_never_starts_an_update_and_disabled_local_builds_ignore_releases() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        for _ in 0..4 {
            let view = controller.presentation(&release());
            assert_eq!(view.version, "0.4.2");
            assert_eq!(view.label, "一键更新");
            assert!(view.available);
            assert!(view.message.is_empty());
        }
        assert_eq!(controller.backend.starts.get(), 0);

        let disabled = Controller::with_backend(FakeBackend::default(), false);
        disabled.start(&release());
        disabled.backend.ready.set(true);
        disabled.backend.snapshot.borrow_mut().stage = Stage::Restarting;
        assert_eq!(disabled.presentation(&release()), Presentation::default());
        assert!(!disabled.take_restart_ready());
        assert_eq!(disabled.backend.starts.get(), 0);
        assert_eq!(disabled.backend.readiness_polls.get(), 0);
    }

    #[test]
    fn absent_release_never_starts_a_download_and_explains_how_to_check_again() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        for release in [
            UpdateSnapshot::default(),
            UpdateSnapshot {
                latest_version: Some("0.4.2".into()),
                release_url: None,
            },
            UpdateSnapshot {
                latest_version: None,
                release_url: release().release_url,
            },
        ] {
            controller.start(&release);
            let view = controller.presentation(&release);
            assert!(view.failed);
            assert!(view.message.contains("检查更新"));
            assert!(!view.available);
        }
        assert_eq!(controller.backend.starts.get(), 0);
    }

    #[test]
    fn rejected_start_is_visible_retryable_and_busy_requests_are_ignored() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        *controller.backend.rejection.borrow_mut() = Some("安装目录不可写，请检查目录权限".into());
        controller.start(&release());
        let rejected = controller.presentation(&release());
        assert!(rejected.failed && rejected.available && !rejected.busy);
        assert!(rejected.message.contains("安装目录不可写"));
        assert_eq!(rejected.label, "重试更新");
        controller.start(&release());
        controller.start(&release());
        assert_eq!(controller.backend.starts.get(), 2);
        let downloading = controller.presentation(&release());
        assert!(downloading.busy && !downloading.available && !downloading.failed);
        assert!(!downloading.message.contains("安装目录不可写"));
    }

    #[test]
    fn progress_handles_known_unknown_empty_and_overshooting_lengths() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        *controller.backend.snapshot.borrow_mut() = Snapshot {
            stage: Stage::Downloading,
            downloaded: 5 * 1024 * 1024,
            total: Some(10 * 1024 * 1024),
            message: String::new(),
        };
        let halfway = controller.presentation(&release());
        assert_eq!(halfway.progress, Some(0.5));
        assert!(halfway.message.contains("5.0 / 10.0 MiB · 50%"));
        for total in [None, Some(0)] {
            controller.backend.snapshot.borrow_mut().total = total;
            let unknown = controller.presentation(&release());
            assert_eq!(unknown.progress, None);
            assert!(unknown.message.contains("5.0 MiB"));
        }
        controller.backend.snapshot.borrow_mut().total = Some(1);
        assert_eq!(controller.presentation(&release()).progress, Some(1.0));
    }

    #[test]
    fn every_busy_phase_blocks_duplicates_and_failure_retains_complete_error_for_retry() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        for (stage, label) in [
            (Stage::Downloading, "下载中…"),
            (Stage::Verifying, "校验中…"),
            (Stage::Preparing, "准备中…"),
            (Stage::Restarting, "重启中…"),
        ] {
            controller.backend.snapshot.borrow_mut().stage = stage;
            controller.start(&release());
            let view = controller.presentation(&release());
            assert!(view.busy && !view.available);
            assert_eq!(view.label, label);
            assert!(!view.message.is_empty());
        }
        assert_eq!(controller.backend.starts.get(), 0);
        let error = "The Windows package checksum does not match SHA256SUMS.txt; download again.";
        *controller.backend.snapshot.borrow_mut() = Snapshot {
            stage: Stage::Failed,
            message: error.into(),
            ..Snapshot::default()
        };
        let failed = controller.presentation(&release());
        assert!(failed.failed && failed.available && !failed.busy);
        assert!(failed.message.contains(error));
        controller.start(&release());
        assert_eq!(controller.backend.starts.get(), 1);
        assert!(!controller.presentation(&release()).failed);
    }

    #[test]
    fn readiness_dispatches_once_without_rendering_or_reopening_the_panel() {
        let controller = Controller::with_backend(FakeBackend::default(), true);
        controller.backend.snapshot.borrow_mut().stage = Stage::Downloading;
        controller.backend.ready.set(true);
        assert!(!controller.take_restart_ready());
        assert_eq!(controller.backend.readiness_polls.get(), 0);
        controller.backend.snapshot.borrow_mut().stage = Stage::Restarting;
        assert!(controller.take_restart_ready());
        assert!(!controller.take_restart_ready());
        assert_eq!(controller.backend.readiness_polls.get(), 1);
        controller.restart_failed("event loop unavailable");
        let failure = controller.presentation(&release());
        assert!(failure.failed && !failure.available);
        assert!(failure.message.contains("正常退出"));
        assert!(failure.message.contains("event loop unavailable"));
        controller.backend.snapshot.borrow_mut().stage = Stage::Failed;
        controller.backend.snapshot.borrow_mut().message =
            "The helper timed out; try again.".into();
        let expired = controller.presentation(&release());
        assert!(expired.available && !expired.busy);
        assert!(expired.message.contains("helper timed out"));
        assert!(!expired.message.contains("正常退出"));
    }
}
