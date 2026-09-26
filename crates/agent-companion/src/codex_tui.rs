//! Status-bar draft state and the native editor. Opening/toggling never writes;
//! Apply/Restore save status-line drafts; explicit profile sync overwrites one
//! selected file after backing up the target. Opening/toggling never writes.

#[cfg(target_os = "macos")]
use crate::macos_deployment as deployment;
#[cfg(windows)]
use crate::windows_deployment as deployment;

use std::{cell::RefCell, collections::HashSet, path::PathBuf, rc::Rc};

use agent_companion_core::install::codex_tui::{self as config, StatusLine};
#[cfg(any(target_os = "macos", windows))]
use agent_companion_core::install::profile_sync::{
    self, Direction, FileKind, ProfilePair, SyncOutcome,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::ui;

pub(crate) struct Draft {
    saved: StatusLine,
    order: Vec<String>,
    selected: HashSet<String>,
}

impl Draft {
    fn new(saved: StatusLine) -> Self {
        let mut order = saved.visible_items();
        let selected = order.iter().cloned().collect();
        for component in config::COMPONENTS {
            if !order.iter().any(|id| id == component.id) {
                order.push(component.id.to_owned());
            }
        }
        Self {
            saved,
            order,
            selected,
        }
    }

    fn items(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|id| self.selected.contains(*id))
            .cloned()
            .collect()
    }

    fn dirty(&self) -> bool {
        self.items() != self.saved.visible_items()
    }

    fn toggle(&mut self, id: &str, enabled: bool) {
        if !self.order.iter().any(|item| item == id) {
            return;
        }
        if enabled {
            self.selected.insert(id.to_owned());
        } else {
            self.selected.remove(id);
        }
    }

    fn rows(&self) -> Vec<ui::StatusComponent> {
        let mut rows = config::COMPONENTS
            .iter()
            .map(|component| ui::StatusComponent {
                id: component.id.into(),
                label: component.label.into(),
                description: component.description.into(),
                selected: self.selected.contains(component.id),
            })
            .collect::<Vec<_>>();
        // Keep frequently used choices visible before the less common details.
        let quick = [
            "model-with-reasoning",
            "current-dir",
            "thread-name",
            "git-branch",
            "context-remaining",
            "five-hour-limit",
            "weekly-limit",
            "task-progress",
            "used-tokens",
            "codex-version",
        ];
        rows.sort_by_key(|row| {
            quick
                .iter()
                .position(|id| row.id == *id)
                .unwrap_or(quick.len())
        });
        for id in &self.order {
            if !rows.iter().any(|row| row.id == id.as_str()) {
                rows.push(ui::StatusComponent {
                    id: id.into(),
                    label: id.into(),
                    description: "Existing custom item · preview unavailable".into(),
                    selected: self.selected.contains(id),
                });
            }
        }
        rows
    }
}

struct InstanceDraft {
    path: PathBuf,
    draft: Option<Draft>,
}

impl InstanceDraft {
    fn new(path: PathBuf) -> Self {
        Self { path, draft: None }
    }

    fn load(&mut self) -> std::io::Result<()> {
        self.draft = None;
        self.draft = Some(Draft::new(config::read(&self.path)?));
        Ok(())
    }

    fn save(&mut self, restore: bool) -> std::io::Result<()> {
        let Some(draft) = self.draft.as_ref() else {
            return Ok(());
        };
        let items = draft.items();
        let saved = config::save(
            &self.path,
            &draft.saved,
            if restore { None } else { Some(&items) },
        )?;
        self.draft = Some(Draft::new(saved));
        Ok(())
    }
}

/// Paths and unsaved state travel together. Switching the visible instance can
/// never retarget a draft or copy one instance's configuration into the other.
struct InstanceDrafts {
    primary: InstanceDraft,
    secondary: Option<InstanceDraft>,
    #[cfg(any(target_os = "macos", windows, test))]
    secondary_available: bool,
    secondary_selected: bool,
}

#[cfg(any(target_os = "macos", windows))]
struct SyncOperation {
    kind: FileKind,
    target: PathBuf,
    receiver: std::sync::mpsc::Receiver<Result<SyncOutcome, String>>,
}

#[cfg(any(target_os = "macos", windows))]
#[derive(Clone, Default)]
struct SyncFeedback {
    message: String,
    error: bool,
    backup: Option<PathBuf>,
}

impl InstanceDrafts {
    fn new(primary: PathBuf) -> Self {
        Self {
            primary: InstanceDraft::new(primary),
            secondary: None,
            #[cfg(any(target_os = "macos", windows, test))]
            secondary_available: false,
            secondary_selected: false,
        }
    }

    fn active(&self) -> &InstanceDraft {
        if self.secondary_selected {
            self.secondary.as_ref().unwrap()
        } else {
            &self.primary
        }
    }

    fn active_mut(&mut self) -> &mut InstanceDraft {
        if self.secondary_selected {
            self.secondary.as_mut().unwrap()
        } else {
            &mut self.primary
        }
    }

    #[cfg(any(target_os = "macos", windows, test))]
    fn set_secondary(&mut self, path: Option<PathBuf>) {
        let path = path.filter(|path| *path != self.primary.path);
        self.secondary_available = path.is_some();
        if let Some(path) = path {
            if self
                .secondary
                .as_ref()
                .is_none_or(|entry| entry.path != path)
            {
                self.secondary = Some(InstanceDraft::new(path));
            }
        } else {
            self.secondary_selected = false;
        }
    }

    #[cfg(any(target_os = "macos", windows, test))]
    fn select(&mut self, secondary: bool) -> bool {
        if secondary && !self.secondary_available {
            return false;
        }
        self.secondary_selected = secondary;
        true
    }
}

pub(crate) struct Editor {
    pub window: ui::CodexTuiWindow,
    drafts: RefCell<InstanceDrafts>,
    left_rows: Rc<VecModel<ui::StatusComponent>>,
    right_rows: Rc<VecModel<ui::StatusComponent>>,
    #[cfg(any(target_os = "macos", windows))]
    sync_rows: Rc<VecModel<ui::ProfileSyncFile>>,
    #[cfg(any(target_os = "macos", windows))]
    sync_paths: RefCell<Option<ProfilePair>>,
    #[cfg(any(target_os = "macos", windows))]
    sync_operation: RefCell<Option<SyncOperation>>,
    #[cfg(any(target_os = "macos", windows))]
    sync_feedback: RefCell<[SyncFeedback; 2]>,
    #[cfg(any(target_os = "macos", windows))]
    pub(crate) deployment_timer: slint::Timer,
    #[cfg(any(target_os = "macos", windows))]
    deployment_operation:
        RefCell<Option<std::sync::mpsc::Receiver<Result<deployment::DeploymentStatus, String>>>>,
    #[cfg(any(target_os = "macos", windows))]
    deployment_error: RefCell<Option<String>>,
    #[cfg(any(target_os = "macos", windows))]
    live_deployment: bool,
}

impl Editor {
    pub fn new(path: PathBuf) -> Result<Rc<Self>, slint::PlatformError> {
        Self::new_inner(path, true)
    }

    /// Native UI verification supplies synthetic configuration and deployment
    /// state. It must never discover or modify the user's real Dodex profile.
    #[cfg(all(any(target_os = "macos", windows), test))]
    #[allow(dead_code)]
    pub(crate) fn new_isolated(path: PathBuf) -> Result<Rc<Self>, slint::PlatformError> {
        Self::new_inner(path, false)
    }

    #[cfg(all(any(target_os = "macos", windows), test))]
    #[allow(dead_code)]
    pub(crate) fn add_isolated_secondary(&self, path: PathBuf) {
        assert!(!self.live_deployment);
        let primary = self.drafts.borrow().primary.path.clone();
        let home = path.parent().unwrap();
        let paths = |config: PathBuf| profile_sync::ProfilePaths {
            instructions: config.parent().unwrap().join("AGENTS.md"),
            instructions_override: config
                .parent()
                .unwrap()
                .join("AGENTS.override.md")
                .is_file()
                .then(|| config.parent().unwrap().join("AGENTS.override.md")),
            config,
        };
        *self.sync_paths.borrow_mut() = Some(ProfilePair {
            primary: paths(primary),
            secondary: paths(path.clone()),
            secondary_isolation: profile_sync::IsolationPaths {
                sqlite_home: home.join("sqlite"),
                log_dir: home.join("log"),
            },
        });
        self.drafts.borrow_mut().set_secondary(Some(path));
        self.window
            .set_instance_options(ModelRc::new(VecModel::from(vec![
                "Codex".into(),
                "Dodex".into(),
            ])));
        self.window.set_dual_enabled(true);
        self.window.set_dual_deployed(true);
        self.refresh_profile_sync();
    }

    #[cfg(all(any(target_os = "macos", windows), test))]
    #[allow(dead_code)]
    pub(crate) fn set_isolated_monitoring(&self, enabled: bool) {
        assert!(!self.live_deployment);
        let path = self
            .sync_paths
            .borrow()
            .as_ref()
            .unwrap()
            .secondary
            .config
            .clone();
        self.drafts
            .borrow_mut()
            .set_secondary(enabled.then_some(path));
        self.window.set_dual_enabled(enabled);
        self.refresh_profile_sync();
    }

    #[cfg(all(any(target_os = "macos", windows), test))]
    #[allow(dead_code)]
    pub(crate) fn reload_isolated_drafts(&self) {
        assert!(!self.live_deployment);
        {
            let mut drafts = self.drafts.borrow_mut();
            drafts.primary.load().unwrap();
            if let Some(secondary) = drafts.secondary.as_mut() {
                secondary.load().unwrap();
            }
        }
        self.refresh();
    }

    fn new_inner(path: PathBuf, _live_deployment: bool) -> Result<Rc<Self>, slint::PlatformError> {
        let editor = Rc::new(Self {
            window: ui::CodexTuiWindow::new()?,
            drafts: RefCell::new(InstanceDrafts::new(path)),
            left_rows: Rc::new(VecModel::default()),
            right_rows: Rc::new(VecModel::default()),
            #[cfg(any(target_os = "macos", windows))]
            sync_rows: Rc::new(VecModel::default()),
            #[cfg(any(target_os = "macos", windows))]
            sync_paths: RefCell::new(None),
            #[cfg(any(target_os = "macos", windows))]
            sync_operation: RefCell::new(None),
            #[cfg(any(target_os = "macos", windows))]
            sync_feedback: RefCell::new(Default::default()),
            #[cfg(any(target_os = "macos", windows))]
            deployment_timer: slint::Timer::default(),
            #[cfg(any(target_os = "macos", windows))]
            deployment_operation: RefCell::new(None),
            #[cfg(any(target_os = "macos", windows))]
            deployment_error: RefCell::new(None),
            #[cfg(any(target_os = "macos", windows))]
            live_deployment: _live_deployment,
        });
        editor
            .window
            .set_left_components(ModelRc::from(editor.left_rows.clone()));
        editor
            .window
            .set_right_components(ModelRc::from(editor.right_rows.clone()));
        editor.window.set_config_path(
            editor
                .drafts
                .borrow()
                .active()
                .path
                .to_string_lossy()
                .as_ref()
                .into(),
        );
        editor
            .window
            .set_catalog_version(config::CATALOG_VERSION.into());
        #[cfg(any(target_os = "macos", windows))]
        {
            editor
                .window
                .set_sync_files(ModelRc::from(editor.sync_rows.clone()));
            let weak = Rc::downgrade(&editor);
            editor.window.on_sync_profile(move |file, to_secondary| {
                if let Some(editor) = weak.upgrade() {
                    editor.start_profile_sync(file.as_str(), to_secondary);
                }
            });
            let weak = Rc::downgrade(&editor);
            editor
                .window
                .on_open_sync_file(move |file, secondary, edit| {
                    if let Some(editor) = weak.upgrade() {
                        editor.open_sync_file(file.as_str(), secondary, edit);
                    }
                });
            let weak = Rc::downgrade(&editor);
            editor.window.on_refresh_sync(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.refresh_profile_sync();
                }
            });
            editor.refresh_profile_sync();
        }
        #[cfg(target_os = "macos")]
        {
            editor.window.set_mono_font("Menlo".into());
            editor
                .window
                .set_window_title("Agent Companion · Settings".into());
            editor.window.set_macos_preferences(true);
            editor.refresh_deployment();
            let weak = Rc::downgrade(&editor);
            editor.window.on_deploy_dual(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.start_deployment(None);
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_toggle_dual(move |enabled| {
                if let Some(editor) = weak.upgrade() {
                    editor.start_deployment(Some(enabled));
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_select_instance(move |label| {
                if let Some(editor) = weak.upgrade() {
                    editor.select_instance(label.as_str());
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.deployment_timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(300),
                move || {
                    if let Some(editor) = weak.upgrade() {
                        editor.refresh_deployment();
                    }
                },
            );
        }
        #[cfg(windows)]
        {
            editor.refresh_deployment();
            let weak = Rc::downgrade(&editor);
            editor.window.on_deploy_dual(move || {
                if let Some(editor) = weak.upgrade() {
                    editor.start_deployment(None);
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_toggle_dual(move |enabled| {
                if let Some(editor) = weak.upgrade() {
                    editor.start_deployment(Some(enabled));
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_select_instance(move |label| {
                if let Some(editor) = weak.upgrade() {
                    editor.select_instance(label.as_str());
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_open_dual(move || {
                if let Some(editor) = weak.upgrade() {
                    if editor.deployment_operation.borrow().is_some()
                        || editor.sync_operation.borrow().is_some()
                    {
                        return;
                    }
                    let (sender, receiver) = std::sync::mpsc::channel();
                    *editor.deployment_operation.borrow_mut() = Some(receiver);
                    std::thread::spawn(move || {
                        let result = deployment::launch(None).map(|_| deployment::status());
                        let _ = sender.send(result);
                    });
                    editor.refresh_deployment();
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.deployment_timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(500),
                move || {
                    if let Some(editor) = weak.upgrade() {
                        editor.refresh_deployment();
                    }
                },
            );
        }
        let weak = Rc::downgrade(&editor);
        editor.window.on_toggle(move |id, enabled| {
            if let Some(editor) = weak.upgrade() {
                #[cfg(any(target_os = "macos", windows))]
                if editor.sync_operation.borrow().is_some() { return; }
                if let Some(draft) = editor.drafts.borrow_mut().active_mut().draft.as_mut() {
                    draft.toggle(&id, enabled);
                }
                editor.refresh();
                if !editor.window.get_error() {
                    editor.note(if editor.window.get_dirty() {
                        "Preview updated. Apply to save, then restart Codex CLI."
                    } else {
                        "Restart Codex CLI to load saved changes. Use codex resume to continue a session."
                    }, false);
                }
            }
        });
        let weak = Rc::downgrade(&editor);
        editor.window.on_apply(move || {
            if let Some(editor) = weak.upgrade() {
                editor.save(false);
            }
        });
        let weak = Rc::downgrade(&editor);
        editor.window.on_restore_defaults(move || {
            if let Some(editor) = weak.upgrade() {
                editor.save(true);
            }
        });
        Ok(editor)
    }

    #[cfg(any(target_os = "macos", windows))]
    fn start_profile_sync(&self, key: &str, to_secondary: bool) {
        let Some(kind) = sync_kind(key) else {
            return;
        };
        if self.sync_operation.borrow().is_some() || self.deployment_operation.borrow().is_some() {
            return;
        }
        self.refresh_profile_sync();
        if self.sync_operation.borrow().is_some()
            || self.deployment_operation.borrow().is_some()
            || self.window.get_dual_busy()
        {
            return;
        }
        let Some(pair) = self.sync_paths.borrow().clone() else {
            return;
        };
        let index = sync_index(&kind);
        if kind == FileKind::Config && self.config_sync_dirty(&pair) {
            self.sync_feedback.borrow_mut()[index] = SyncFeedback {
                message: "状态栏有未应用的更改。请先在「Codex CLI」页签应用更改，再覆盖配置。"
                    .into(),
                error: true,
                backup: None,
            };
            self.refresh_profile_sync();
            return;
        }
        let source = sync_path(&pair, &kind, !to_secondary);
        if !regular_file(source) {
            self.sync_feedback.borrow_mut()[index] = SyncFeedback {
                message: "源文件不存在或不是常规文件，未执行覆盖。".into(),
                error: true,
                backup: None,
            };
            self.refresh_profile_sync();
            return;
        }
        let target = sync_path(&pair, &kind, to_secondary).to_owned();
        let direction = if to_secondary {
            Direction::ToSecondary
        } else {
            Direction::ToPrimary
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.sync_operation.borrow_mut() = Some(SyncOperation {
            kind,
            target,
            receiver,
        });
        self.sync_feedback.borrow_mut()[index] = SyncFeedback {
            message: "正在备份目标并覆盖文件…".into(),
            error: false,
            backup: None,
        };
        let live = self.live_deployment;
        let worker = std::thread::Builder::new()
            .name("codex-profile-sync".into())
            .spawn(move || {
                let result = if live {
                    deployment::sync_profile_file(kind, direction)
                } else {
                    profile_sync::sync_file(&pair, kind, direction).map_err(profile_sync_error)
                };
                let _ = sender.send(result);
            });
        if worker.is_err() {
            self.sync_operation.borrow_mut().take();
            self.sync_feedback.borrow_mut()[index] = SyncFeedback {
                message: "无法开始文件同步，请重试。".into(),
                error: true,
                backup: None,
            };
        }
        self.update_profile_sync();
    }

    #[cfg(any(target_os = "macos", windows))]
    fn config_sync_dirty(&self, pair: &ProfilePair) -> bool {
        let drafts = self.drafts.borrow();
        std::iter::once(&drafts.primary)
            .chain(drafts.secondary.iter())
            .any(|state| {
                (state.path == pair.primary.config || state.path == pair.secondary.config)
                    && state.draft.as_ref().is_some_and(Draft::dirty)
            })
    }

    #[cfg(any(target_os = "macos", windows))]
    fn refresh_profile_sync(&self) {
        let completed =
            self.sync_operation.borrow().as_ref().and_then(|operation| {
                match operation.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("文件同步已中断，请刷新状态后重试。".into()))
                    }
                }
            });
        if let Some(result) = completed {
            let operation = self.sync_operation.borrow_mut().take().unwrap();
            let index = sync_index(&operation.kind);
            let feedback = match result {
                Ok(outcome) => {
                    if operation.kind == FileKind::Config {
                        // Both involved drafts were clean when work began, and
                        // edit/save callbacks stay blocked until it completes.
                        // Reload only the target; preserve the source and selection.
                        let mut drafts = self.drafts.borrow_mut();
                        if drafts.primary.path == operation.target {
                            let _ = drafts.primary.load();
                        } else if let Some(target) = drafts
                            .secondary
                            .as_mut()
                            .filter(|state| state.path == operation.target)
                        {
                            let _ = target.load();
                        }
                    }
                    SyncFeedback {
                        message: if outcome.changed {
                            "文件已覆盖。请重新启动相应 Codex / Dodex 会话以载入更改。"
                        } else {
                            "文件内容已一致，无需写入或创建备份。"
                        }
                        .into(),
                        error: false,
                        backup: outcome.backup_path,
                    }
                }
                Err(message) => SyncFeedback {
                    message,
                    error: true,
                    backup: None,
                },
            };
            self.sync_feedback.borrow_mut()[index] = feedback;
            self.refresh();
        }
        self.update_profile_sync();
    }

    #[cfg(any(target_os = "macos", windows))]
    fn update_profile_sync(&self) {
        let busy = self.sync_operation.borrow().is_some();
        let deployment_busy = self.deployment_operation.borrow().is_some()
            || (self.live_deployment && deployment::status().busy);
        // Validation shares the deployment lock. Keep the last validated paths
        // visible while an operation holds it, then rediscover on completion.
        if self.live_deployment && !busy && !deployment_busy {
            match deployment::profile_sync_paths() {
                Ok(pair) => {
                    *self.sync_paths.borrow_mut() = Some(pair);
                    self.window.set_sync_status(
                        "每次手动选择覆盖方向；不会自动同步或建立链接。目标文件存在时会先备份。"
                            .into(),
                    );
                    self.window.set_sync_error(false);
                }
                Err(error) => {
                    self.sync_paths.borrow_mut().take();
                    let deployed = deployment::status().deployed;
                    self.window.set_sync_status(if deployed {
                        error.into()
                    } else {
                        "请先部署并验证 Dodex 环境，再手动同步文件。".into()
                    });
                    self.window.set_sync_error(deployed);
                }
            }
        } else if !self.live_deployment
            && let Some(pair) = self.sync_paths.borrow_mut().as_mut()
        {
            // Keep the isolated native fixture's metadata current without ever
            // discovering a real secondary profile or changing deployment state.
            for paths in [&mut pair.primary, &mut pair.secondary] {
                let path = paths.instructions.with_file_name("AGENTS.override.md");
                paths.instructions_override = path.exists().then_some(path);
            }
        }
        self.window.set_sync_busy(busy);
        self.window.set_dual_busy(busy || deployment_busy);
        let pair = self.sync_paths.borrow();
        let dirty = pair
            .as_ref()
            .is_some_and(|pair| self.config_sync_dirty(pair));
        let primary_config = self.drafts.borrow().primary.path.clone();
        let primary_instructions = primary_config.with_file_name("AGENTS.md");
        for (index, kind) in [FileKind::Config, FileKind::Instructions]
            .into_iter()
            .enumerate()
        {
            let primary = pair
                .as_ref()
                .map(|pair| sync_path(pair, &kind, false))
                .unwrap_or(if kind == FileKind::Config {
                    &primary_config
                } else {
                    &primary_instructions
                });
            let secondary = pair.as_ref().map(|pair| sync_path(pair, &kind, true));
            let primary_exists = regular_file(primary);
            let secondary_exists = secondary.is_some_and(regular_file);
            let mut note = String::new();
            if kind == FileKind::Config && dirty {
                note.push_str("Codex 或 Dodex 状态栏有未应用的更改，请先在「Codex CLI」页签应用后再同步配置。Dodex 已停用时需先重新启用以处理其草稿。");
            }
            if kind == FileKind::Instructions
                && let Some(pair) = pair.as_ref()
            {
                for (name, paths) in [("Codex", &pair.primary), ("Dodex", &pair.secondary)] {
                    if let Some(path) = &paths.instructions_override {
                        if !note.is_empty() {
                            note.push('\n');
                        }
                        note.push_str(&format!("{name} 的 {} 可能优先于同目录 AGENTS.md；本操作不会修改该 override 文件。", path.display()));
                    }
                }
            }
            if !primary_exists || !secondary_exists {
                if !note.is_empty() {
                    note.push('\n');
                }
                note.push_str("源文件不存在时对应方向不可用；目标文件不存在时可创建。");
            }
            let feedback = self.sync_feedback.borrow()[index].clone();
            let available =
                pair.is_some() && !busy && !deployment_busy && !(kind == FileKind::Config && dirty);
            let row = ui::ProfileSyncFile {
                key: if kind == FileKind::Config {
                    "config"
                } else {
                    "instructions"
                }
                .into(),
                title: if kind == FileKind::Config {
                    "config.toml"
                } else {
                    "AGENTS.md"
                }
                .into(),
                primary_path: primary.to_string_lossy().as_ref().into(),
                secondary_path: secondary
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                primary_exists,
                secondary_exists,
                to_secondary: available && primary_exists,
                to_primary: available && secondary_exists,
                note: note.into(),
                message: feedback.message.into(),
                error: feedback.error,
                backup_path: feedback
                    .backup
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
            };
            if self.sync_rows.row_count() <= index {
                self.sync_rows.push(row);
            } else if self.sync_rows.row_data(index).as_ref() != Some(&row) {
                self.sync_rows.set_row_data(index, row);
            }
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    fn open_sync_file(&self, key: &str, secondary: bool, edit: bool) {
        let Some(kind) = sync_kind(key) else {
            return;
        };
        self.refresh_profile_sync();
        if edit && (self.sync_operation.borrow().is_some() || self.window.get_dual_busy()) {
            return;
        }
        let path = if let Some(pair) = self.sync_paths.borrow().as_ref() {
            sync_path(pair, &kind, secondary).to_owned()
        } else if !secondary {
            let primary = self.drafts.borrow().primary.path.clone();
            if kind == FileKind::Config {
                primary
            } else {
                primary.with_file_name("AGENTS.md")
            }
        } else {
            return;
        };
        let result = open_profile_path(&path, edit);
        if result.is_err() {
            self.sync_feedback.borrow_mut()[sync_index(&kind)] = SyncFeedback {
                message: "无法打开文件或所在目录。可复制上方路径后手动打开。".into(),
                error: true,
                backup: None,
            };
            self.refresh_profile_sync();
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    fn start_deployment(&self, enabled: Option<bool>) {
        if !self.live_deployment {
            return;
        }
        // Mark the operation pending before spawning so even two clicks in the
        // same event-loop turn cannot start two workers.
        if self.deployment_operation.borrow().is_some()
            || self.sync_operation.borrow().is_some()
            || deployment::status().busy
        {
            return;
        }
        *self.deployment_error.borrow_mut() = None;
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.deployment_operation.borrow_mut() = Some(receiver);
        let worker = std::thread::Builder::new()
            .name("codex-deployment".into())
            .spawn(move || {
                let result = match enabled {
                    Some(enabled) => deployment::set_enabled(enabled),
                    None => deployment::deploy(),
                };
                let _ = sender.send(result);
            });
        if worker.is_err() {
            self.deployment_operation.borrow_mut().take();
            *self.deployment_error.borrow_mut() = Some("无法开始部署操作，请重试。".into());
        }
        self.refresh_deployment();
    }

    #[cfg(any(target_os = "macos", windows))]
    fn refresh_deployment(&self) {
        self.refresh_profile_sync();
        if !self.live_deployment {
            return;
        }
        let completed = self
            .deployment_operation
            .borrow()
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("部署操作已中断，请重试。".into()))
                }
            });
        if let Some(result) = completed {
            self.deployment_operation.borrow_mut().take();
            *self.deployment_error.borrow_mut() = result.err();
        }
        let status = deployment::status();
        let busy = status.busy
            || self.deployment_operation.borrow().is_some()
            || self.sync_operation.borrow().is_some();
        let active = deployment::active_instance();
        let (available, selection_changed) = {
            let mut drafts = self.drafts.borrow_mut();
            let previous_path = drafts.active().path.clone();
            drafts.set_secondary(
                active
                    .filter(|_| status.enabled)
                    .map(|instance| instance.codex_home.join("config.toml")),
            );
            (
                drafts.secondary_available,
                previous_path != drafts.active().path,
            )
        };
        if self.window.get_instance_options().row_count() != if available { 2 } else { 1 } {
            let labels = if available {
                vec!["Codex".into(), "Dodex".into()]
            } else {
                vec!["Codex".into()]
            };
            self.window
                .set_instance_options(ModelRc::new(VecModel::from(labels)));
        }
        self.window
            .set_selected_instance(i32::from(self.drafts.borrow().secondary_selected));
        self.window.set_dual_enabled(available);
        self.window.set_dual_deployed(status.deployed);
        self.window.set_dual_busy(busy);
        self.window.set_dual_phase(
            match status.phase.as_str() {
                "copying" => "正在复制官方运行程序…",
                "configuring" => "正在创建独立环境…",
                "verifying" => "正在验证隔离与签名…",
                "shell" => "正在设置 dodex 命令…",
                "finishing" => "正在完成部署…",
                _ => "正在检查双开环境…",
            }
            .into(),
        );
        let error = self.deployment_error.borrow();
        self.window
            .set_dual_error(error.is_some() || status.phase == "failed");
        self.window
            .set_dual_message(error.as_deref().unwrap_or(&status.message).into());
        if selection_changed {
            let result = {
                let mut drafts = self.drafts.borrow_mut();
                let active = drafts.active_mut();
                if active.draft.is_none() {
                    active.load()
                } else {
                    Ok(())
                }
            };
            self.refresh();
            match result {
                Ok(()) => self.note("The available instance changed. Check the selected configuration before applying; existing drafts are retained.", false),
                Err(error) => self.note(&format!("Could not read configuration: {}", configuration_error(&error)), true),
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn show_deployment_error(&self, error: String) {
        *self.deployment_error.borrow_mut() = Some(error);
        self.window.set_settings_page(3);
        self.refresh_deployment();
    }

    #[cfg(any(target_os = "macos", windows))]
    fn select_instance(&self, label: &str) {
        let secondary = match label {
            "Codex" => false,
            "Dodex" => true,
            _ => return,
        };
        let result = {
            let mut drafts = self.drafts.borrow_mut();
            if !drafts.select(secondary) {
                return;
            }
            let active = drafts.active_mut();
            if active.draft.is_none() {
                active.load()
            } else {
                Ok(())
            }
        };
        self.window.set_selected_instance(i32::from(secondary));
        self.refresh();
        match result {
            Ok(()) => self.note(
                "Each instance keeps its own draft. Apply saves only the selected instance.",
                false,
            ),
            Err(error) => self.note(
                &format!(
                    "Could not read {label} configuration: {}",
                    configuration_error(&error)
                ),
                true,
            ),
        }
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        if !self.window.window().is_visible() {
            match self.drafts.borrow_mut().active_mut().load() {
                Ok(()) => {
                    self.note("Restart Codex CLI after applying. Project or profile overrides may take priority.", false);
                }
                Err(error) => {
                    self.note(
                        &format!(
                            "Could not read configuration: {}",
                            configuration_error(&error)
                        ),
                        true,
                    );
                }
            }
            self.refresh();
        }
        self.window.show()
    }

    fn note(&self, message: &str, error: bool) {
        self.window.set_message(message.into());
        self.window.set_error(error);
    }

    fn refresh(&self) {
        #[cfg(any(target_os = "macos", windows))]
        self.update_profile_sync();
        let drafts = self.drafts.borrow();
        let state = drafts.active();
        self.window
            .set_config_path(state.path.to_string_lossy().as_ref().into());
        self.window.set_ready(state.draft.is_some());
        let Some(draft) = state.draft.as_ref() else {
            self.left_rows.set_vec(vec![]);
            self.right_rows.set_vec(vec![]);
            self.window.set_dirty(false);
            self.window.set_preview("".into());
            self.window.set_selected_count(0);
            return;
        };
        let items = draft.items();
        self.window.set_preview(config::preview(&items).into());
        self.window.set_selected_count(items.len() as i32);
        self.window.set_dirty(draft.dirty());
        self.window.set_custom(draft.saved.items.is_some());
        let rows = draft.rows();
        // Read across each row, keeping the first/default components together.
        update_rows(&self.left_rows, rows.iter().step_by(2).cloned().collect());
        update_rows(
            &self.right_rows,
            rows.iter().skip(1).step_by(2).cloned().collect(),
        );
    }

    fn save(&self, restore: bool) {
        #[cfg(any(target_os = "macos", windows))]
        if self.sync_operation.borrow().is_some() {
            return;
        }
        if self.drafts.borrow().active().draft.is_none() {
            return;
        }
        #[cfg(any(target_os = "macos", windows))]
        if self.live_deployment && self.drafts.borrow().secondary_selected {
            let active = deployment::active_instance();
            if active.is_none_or(|instance| {
                instance.codex_home.join("config.toml") != self.drafts.borrow().active().path
            }) {
                self.note("Dodex is no longer available. Re-enable and validate its deployment before saving.", true);
                return;
            }
        }
        let result = self.drafts.borrow_mut().active_mut().save(restore);
        match result {
            Ok(()) => {
                self.note(
                    if restore {
                        "Codex defaults restored. Restart Codex CLI to load them."
                    } else {
                        "Status bar saved. Restart Codex CLI to load it; use codex resume to continue a session."
                    },
                    false,
                );
                self.refresh();
            }
            Err(error) => self.note(
                &format!("Could not save: {}", configuration_error(&error)),
                true,
            ),
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn sync_kind(key: &str) -> Option<FileKind> {
    match key {
        "config" => Some(FileKind::Config),
        "instructions" => Some(FileKind::Instructions),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", windows))]
fn sync_index(kind: &FileKind) -> usize {
    usize::from(*kind == FileKind::Instructions)
}

#[cfg(any(target_os = "macos", windows))]
fn sync_path<'a>(pair: &'a ProfilePair, kind: &FileKind, secondary: bool) -> &'a std::path::Path {
    let paths = if secondary {
        &pair.secondary
    } else {
        &pair.primary
    };
    match kind {
        FileKind::Config => &paths.config,
        FileKind::Instructions => &paths.instructions,
    }
}

#[cfg(any(target_os = "macos", windows))]
fn regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

#[cfg(any(target_os = "macos", windows))]
fn profile_sync_error(error: std::io::Error) -> String {
    // Filesystem/TOML diagnostics are intentionally not shown: a parser may
    // include a source excerpt containing a token or provider credential.
    match error.kind() {
        std::io::ErrorKind::NotFound => "同步失败：源文件或部署目录不存在，请刷新后重试。",
        std::io::ErrorKind::PermissionDenied => "同步失败：没有读取源文件或写入目标目录的权限。",
        std::io::ErrorKind::InvalidData => "同步失败：配置格式或隔离设置无效，请检查文件后重试。",
        _ => "同步失败：文件无法安全读取、备份或覆盖，请检查路径与权限后重试。",
    }
    .into()
}

#[cfg(any(target_os = "macos", windows))]
fn open_profile_path(path: &std::path::Path, edit: bool) -> std::io::Result<()> {
    if edit && !regular_file(path) {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let mut destination = path;
    while !destination.exists() {
        destination = destination.parent().ok_or(std::io::ErrorKind::NotFound)?;
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        if edit {
            command.arg("-t");
        } else if destination.is_file() {
            command.arg("-R");
        }
        command.arg(destination);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command =
            std::process::Command::new(if edit { "notepad.exe" } else { "explorer.exe" });
        if !edit && destination.is_file() {
            command.arg(format!("/select,{}", destination.display()));
        } else {
            command.arg(destination);
        }
        command
    };
    command.spawn().map(|_| ())
}

fn configuration_error(error: &std::io::Error) -> String {
    // TOML parse errors include source excerpts, which can contain credentials
    // in unrelated settings. Never render those excerpts in the macOS UI.
    if error.kind() == std::io::ErrorKind::InvalidData {
        return "The selected config.toml is invalid. Check its syntax and status-line values, then reopen Settings.".into();
    }
    error.to_string()
}

fn update_rows(model: &VecModel<ui::StatusComponent>, rows: Vec<ui::StatusComponent>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    // Keep delegates alive: replacing the entire model on each checkbox click
    // loses keyboard focus and interrupts subsequent Space/Tab interaction.
    for (index, row) in rows.into_iter().enumerate() {
        if model.row_data(index).is_none_or(|previous| previous != row) {
            model.set_row_data(index, row);
        }
    }
}

/// Standalone mode owns only this window. No tray, pipe, hook, or monitor starts.
pub fn run() -> std::io::Result<()> {
    #[cfg(windows)]
    let path = crate::windows_deployment::primary_home()?.join("config.toml");
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    let path = agent_companion_core::install::codex_home()?.join("config.toml");
    #[cfg(target_os = "macos")]
    let path = crate::macos::primary_home()?.join("config.toml");
    #[cfg(target_os = "macos")]
    crate::macos::prepare_editor().map_err(std::io::Error::other)?;
    let editor = Editor::new(path).map_err(std::io::Error::other)?;
    editor.window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    editor.show().map_err(std::io::Error::other)?;
    slint::run_event_loop().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_off_and_back_restores_order_and_clears_dirty_state() {
        let mut draft = Draft::new(StatusLine {
            items: Some(vec![
                "git-branch".into(),
                "future-item".into(),
                "model".into(),
            ]),
        });
        let initial = draft.items();
        draft.toggle("git-branch", false);
        assert!(draft.dirty());
        draft.toggle("git-branch", true);
        assert_eq!(draft.items(), initial);
        assert!(!draft.dirty());
        draft.toggle("current-dir", true);
        assert_eq!(&draft.items()[..3], initial.as_slice());
        assert!(
            draft
                .rows()
                .iter()
                .any(|row| row.id == "future-item" && row.selected)
        );
    }

    #[test]
    fn all_components_can_be_hidden_without_mutating_saved_settings() {
        let saved = StatusLine { items: None };
        let mut draft = Draft::new(saved.clone());
        for id in config::DEFAULT_ITEMS {
            draft.toggle(id, false);
        }
        assert!(draft.items().is_empty());
        assert!(draft.dirty());
        assert_eq!(draft.saved, saved);
        draft.toggle("not-a-component", true);
        assert!(draft.items().is_empty());
    }

    #[test]
    fn instance_drafts_and_saves_never_cross_paths() {
        let home = tempfile::tempdir().unwrap();
        let primary = home.path().join("codex/config.toml");
        let secondary = home.path().join("dodex/config.toml");
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::create_dir_all(secondary.parent().unwrap()).unwrap();
        std::fs::write(
            &primary,
            "model = 'primary-model'\n[tui]\nstatus_line = ['model']\n",
        )
        .unwrap();
        std::fs::write(
            &secondary,
            "model = 'secondary-model'\n[tui]\nstatus_line = ['current-dir']\n",
        )
        .unwrap();
        let primary_original = std::fs::read(&primary).unwrap();
        let secondary_original = std::fs::read(&secondary).unwrap();
        let mut drafts = InstanceDrafts::new(primary.clone());
        drafts.active_mut().load().unwrap();
        drafts
            .active_mut()
            .draft
            .as_mut()
            .unwrap()
            .toggle("git-branch", true);
        drafts.set_secondary(Some(secondary.clone()));
        assert!(drafts.select(true));
        drafts.active_mut().load().unwrap();
        drafts
            .active_mut()
            .draft
            .as_mut()
            .unwrap()
            .toggle("weekly-limit", true);
        assert!(drafts.select(false));
        assert_eq!(
            drafts.active().draft.as_ref().unwrap().items(),
            ["model", "git-branch"]
        );
        assert!(drafts.active().draft.as_ref().unwrap().dirty());
        assert_eq!(std::fs::read(&primary).unwrap(), primary_original);
        assert_eq!(std::fs::read(&secondary).unwrap(), secondary_original);
        drafts.active_mut().save(false).unwrap();
        assert_eq!(std::fs::read(&secondary).unwrap(), secondary_original);
        assert!(drafts.select(true));
        assert_eq!(
            drafts.active().draft.as_ref().unwrap().items(),
            ["current-dir", "weekly-limit"]
        );
        let primary_saved = std::fs::read(&primary).unwrap();
        drafts.active_mut().save(false).unwrap();
        assert_eq!(std::fs::read(&primary).unwrap(), primary_saved);
        assert!(
            std::fs::read_to_string(&secondary)
                .unwrap()
                .contains("secondary-model")
        );
        assert!(
            !std::fs::read_to_string(&secondary)
                .unwrap()
                .contains("primary-model")
        );
        drafts.active_mut().save(true).unwrap();
        assert_eq!(config::read(&secondary).unwrap().items, None);
        assert_eq!(std::fs::read(&primary).unwrap(), primary_saved);
    }

    #[test]
    fn disabling_secondary_preserves_both_drafts_and_blocks_selection() {
        let home = tempfile::tempdir().unwrap();
        let primary = home.path().join("codex.toml");
        let secondary = home.path().join("dodex.toml");
        let mut drafts = InstanceDrafts::new(primary.clone());
        assert!(!drafts.select(true));
        drafts.active_mut().load().unwrap();
        drafts
            .active_mut()
            .draft
            .as_mut()
            .unwrap()
            .toggle("model", true);
        drafts.set_secondary(Some(secondary.clone()));
        assert!(drafts.select(true));
        drafts.active_mut().load().unwrap();
        drafts
            .active_mut()
            .draft
            .as_mut()
            .unwrap()
            .toggle("git-branch", true);
        drafts.set_secondary(None);
        assert!(!drafts.secondary_selected);
        assert!(!drafts.select(true));
        assert!(
            drafts
                .active()
                .draft
                .as_ref()
                .unwrap()
                .items()
                .contains(&"model".into())
        );
        drafts.set_secondary(Some(secondary.clone()));
        assert!(drafts.select(true));
        assert!(
            drafts
                .active()
                .draft
                .as_ref()
                .unwrap()
                .items()
                .contains(&"git-branch".into())
        );
        assert!(!primary.exists());
        assert!(!secondary.exists());
        drafts.set_secondary(Some(primary));
        assert!(
            !drafts.select(true),
            "Primary config cannot be registered as Dodex"
        );
    }

    #[test]
    fn malformed_secondary_does_not_discard_primary_draft() {
        let home = tempfile::tempdir().unwrap();
        let primary = home.path().join("codex.toml");
        let secondary = home.path().join("dodex.toml");
        std::fs::write(&secondary, "[malformed").unwrap();
        let mut drafts = InstanceDrafts::new(primary);
        drafts.active_mut().load().unwrap();
        drafts
            .active_mut()
            .draft
            .as_mut()
            .unwrap()
            .toggle("weekly-limit", true);
        drafts.set_secondary(Some(secondary));
        drafts.select(true);
        assert!(drafts.active_mut().load().is_err());
        assert!(drafts.active().draft.is_none());
        drafts.select(false);
        assert!(drafts.active().draft.as_ref().unwrap().dirty());
        assert!(
            drafts
                .active()
                .draft
                .as_ref()
                .unwrap()
                .items()
                .contains(&"weekly-limit".into())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn configuration_errors_never_render_source_excerpts() {
        let error = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "TOML parse error: synthetic-secret-value",
        );
        let message = configuration_error(&error);
        assert!(message.contains("config.toml is invalid"));
        assert!(!message.contains("synthetic-secret-value"));
    }
}
