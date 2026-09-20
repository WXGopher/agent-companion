//! Status-bar draft state and the native editor. Opening/toggling never writes;
//! only Apply and Restore Codex defaults touch Codex's user configuration.

#[cfg(target_os = "macos")]
use crate::macos_deployment as deployment;
#[cfg(windows)]
use crate::windows_deployment as deployment;

use std::{cell::RefCell, collections::HashSet, path::PathBuf, rc::Rc};

use agent_companion_core::install::codex_tui::{self as config, StatusLine};
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
    #[cfg(target_os = "macos")]
    display_settings: RefCell<Option<crate::macos::DisplaySettings>>,
    #[cfg(any(target_os = "macos", windows))]
    pub(crate) preference_timer: slint::Timer,
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
        self.drafts.borrow_mut().set_secondary(Some(path));
        self.window
            .set_instance_options(ModelRc::new(VecModel::from(vec![
                "Codex".into(),
                "Dodex".into(),
            ])));
        self.window.set_dual_enabled(true);
    }

    fn new_inner(path: PathBuf, _live_deployment: bool) -> Result<Rc<Self>, slint::PlatformError> {
        let editor = Rc::new(Self {
            window: ui::CodexTuiWindow::new()?,
            drafts: RefCell::new(InstanceDrafts::new(path)),
            left_rows: Rc::new(VecModel::default()),
            right_rows: Rc::new(VecModel::default()),
            #[cfg(target_os = "macos")]
            display_settings: RefCell::new(None),
            #[cfg(any(target_os = "macos", windows))]
            preference_timer: slint::Timer::default(),
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
            editor
                .window
                .set_show_dock_icon(crate::macos::dock_visible());
            editor
                .window
                .set_show_menu_bar(crate::macos::menu_bar_visible());
            editor.window.set_show_notch(crate::macos::notch_visible());
            let weak = Rc::downgrade(&editor);
            editor.window.on_toggle_menu_bar(move |visible| {
                if let Some(editor) = weak.upgrade() {
                    let saved = crate::macos::set_menu_bar_visible(visible);
                    editor
                        .window
                        .set_show_menu_bar(crate::macos::menu_bar_visible());
                    editor.window.set_menu_bar_error(!saved);
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_toggle_notch(move |visible| {
                if let Some(editor) = weak.upgrade() {
                    let saved = crate::macos::set_notch_visible(visible);
                    editor.window.set_show_notch(crate::macos::notch_visible());
                    editor.window.set_notch_error(!saved);
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.window.on_toggle_dock_icon(move |visible| {
                if let Some(editor) = weak.upgrade() {
                    let saved = crate::macos::set_dock_visible(visible);
                    editor
                        .window
                        .set_show_dock_icon(crate::macos::dock_visible());
                    editor.window.set_dock_error(!saved);
                }
            });
            editor.refresh_display_settings();
            let weak = Rc::downgrade(&editor);
            editor.window.on_select_display(move |label| {
                if let Some(editor) = weak.upgrade() {
                    // Resolve the displayed option to its stable identity,
                    // rather than persisting a transient list/screen index.
                    let identifier =
                        editor
                            .display_settings
                            .borrow()
                            .as_ref()
                            .and_then(|settings| {
                                settings
                                    .options
                                    .iter()
                                    .find(|option| option.label == label.as_str())
                                    .map(|option| option.id.clone())
                            });
                    let saved = identifier.is_some_and(|id| crate::macos::select_display(&id));
                    editor.window.set_display_error(!saved);
                    editor.refresh_display_settings();
                }
            });
            let weak = Rc::downgrade(&editor);
            editor.preference_timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(300),
                move || {
                    if let Some(editor) = weak.upgrade() {
                        editor.refresh_deployment();
                        editor.refresh_display_settings();
                        editor
                            .window
                            .set_show_dock_icon(crate::macos::dock_visible());
                        editor
                            .window
                            .set_show_menu_bar(crate::macos::menu_bar_visible());
                        editor.window.set_show_notch(crate::macos::notch_visible());
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
                    if editor.deployment_operation.borrow().is_some() {
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
            editor.preference_timer.start(
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
    fn start_deployment(&self, enabled: Option<bool>) {
        if !self.live_deployment {
            return;
        }
        // Mark the operation pending before spawning so even two clicks in the
        // same event-loop turn cannot start two workers.
        if self.deployment_operation.borrow().is_some() || deployment::status().busy {
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
        let busy = status.busy || self.deployment_operation.borrow().is_some();
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

    #[cfg(target_os = "macos")]
    fn refresh_display_settings(&self) {
        let Some(settings) = crate::macos::display_settings() else {
            self.window.set_display_error(true);
            return;
        };
        let mut previous = self.display_settings.borrow_mut();
        if previous
            .as_ref()
            .is_none_or(|old| old.options != settings.options)
        {
            let labels = settings
                .options
                .iter()
                .map(|option| option.label.as_str().into())
                .collect::<Vec<_>>();
            self.window
                .set_display_options(ModelRc::new(VecModel::from(labels)));
        }
        let selected = settings
            .options
            .iter()
            .position(|option| option.id == settings.selected_id)
            .unwrap_or(0);
        // Also restore the control after a failed save, even when the stored
        // snapshot did not change. Updating properties never invokes selection.
        self.window.set_selected_display(selected as i32);
        self.window
            .set_display_status(settings.status.as_str().into());
        *previous = Some(settings);
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

fn configuration_error(error: &std::io::Error) -> String {
    // TOML parse errors include source excerpts, which can contain credentials
    // in unrelated settings. Never render those excerpts in the macOS UI.
    #[cfg(target_os = "macos")]
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
    #[cfg(target_os = "macos")]
    slint::Timer::single_shot(
        std::time::Duration::ZERO,
        crate::macos::start_editor_preferences,
    );
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
