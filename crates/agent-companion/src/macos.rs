//! Native notch shell; the independent Slint editor uses its own process.
use std::ffi::{CStr, CString, c_char};
use std::fs::OpenOptions;
use std::io;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use crate::macos_deployment::InstanceConfig;
use agent_companion_core::dashboard::{Dashboard, Instance, Snapshot};

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();

/// A launcher must not redirect the primary monitor through its inherited
/// CODEX_HOME (for example when Companion is opened from Dodex's terminal).
pub fn primary_home() -> io::Result<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".codex"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine the home directory",
            )
        })
}

unsafe extern "C" {
    fn agent_companion_run_notch() -> i32;
    fn agent_companion_prepare_editor() -> bool;
    fn agent_companion_start_editor_preferences();
    fn agent_companion_dock_visible() -> bool;
    fn agent_companion_set_dock_visible(visible: bool) -> bool;
    fn agent_companion_menu_bar_visible() -> bool;
    fn agent_companion_notch_visible() -> bool;
    fn agent_companion_set_menu_bar_visible(visible: bool) -> bool;
    fn agent_companion_set_notch_visible(visible: bool) -> bool;
    fn agent_companion_reopen_settings();
    fn agent_companion_display_settings_json() -> *mut c_char;
    fn agent_companion_free_native_string(pointer: *mut c_char);
    fn agent_companion_select_display(identifier: *const c_char) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct DisplayOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettings {
    pub options: Vec<DisplayOption>,
    pub selected_id: String,
    pub status: String,
}

pub fn display_settings() -> Option<DisplaySettings> {
    // SAFETY: the UI thread receives a Swift-owned, NUL-terminated allocation
    // and always frees it through the matching native allocator.
    unsafe {
        let pointer = agent_companion_display_settings_json();
        if pointer.is_null() {
            return None;
        }
        let settings = serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()).ok();
        agent_companion_free_native_string(pointer);
        settings
    }
}

pub fn select_display(identifier: &str) -> bool {
    let Ok(identifier) = CString::new(identifier) else {
        return false;
    };
    // SAFETY: UI-thread call; Swift borrows the string only for this call.
    unsafe { agent_companion_select_display(identifier.as_ptr()) }
}

/// Configure Slint before it creates its AppKit event loop, including when
/// launched as a bare CLI binary without an Info.plist.
pub fn prepare_editor() -> Result<(), slint::PlatformError> {
    use slint::winit_030::winit::{
        event_loop::EventLoop,
        platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
    };
    // SAFETY: standalone editor startup runs on the process main thread.
    let visible = unsafe { agent_companion_prepare_editor() };
    let mut builder = EventLoop::with_user_event();
    builder.with_activation_policy(if visible {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    });
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_event_loop_builder(builder)
        // Slint defaults to a transparent native window. Painting the content
        // alone leaves AppKit's title bar clear; settings need an opaque frame.
        .with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false))
        .select()
}

pub fn dock_visible() -> bool {
    // SAFETY: called by the editor on its main UI thread.
    unsafe { agent_companion_dock_visible() }
}

pub fn menu_bar_visible() -> bool {
    // SAFETY: main-thread preferences bridge, no borrowed storage.
    unsafe { agent_companion_menu_bar_visible() }
}
pub fn notch_visible() -> bool {
    // SAFETY: main-thread preferences bridge, no borrowed storage.
    unsafe { agent_companion_notch_visible() }
}
pub fn set_menu_bar_visible(visible: bool) -> bool {
    // SAFETY: called by the editor on its main UI thread.
    unsafe { agent_companion_set_menu_bar_visible(visible) }
}
pub fn set_notch_visible(visible: bool) -> bool {
    // SAFETY: called by the editor on its main UI thread.
    unsafe { agent_companion_set_notch_visible(visible) }
}

pub fn start_editor_preferences() {
    // SAFETY: invoked by a Slint timer after its main-thread event loop starts.
    unsafe { agent_companion_start_editor_preferences() }
}

pub fn set_dock_visible(visible: bool) -> bool {
    // SAFETY: called by the editor on its main UI thread.
    unsafe { agent_companion_set_dock_visible(visible) }
}

/// Swift owns the returned allocation until it calls the paired release.
#[unsafe(no_mangle)]
extern "C" fn agent_companion_snapshot_json() -> *mut c_char {
    let state = SNAPSHOT.get_or_init(|| Mutex::new(Snapshot::default()));
    let state = state.lock().unwrap_or_else(|error| error.into_inner());
    CString::new(serde_json::to_vec(&*state).unwrap_or_default())
        .unwrap_or_default()
        .into_raw()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn agent_companion_release_json(pointer: *mut c_char) {
    if !pointer.is_null() {
        // SAFETY: the native bridge returns each allocation exactly once.
        drop(unsafe { CString::from_raw(pointer) });
    }
}

pub fn run() -> io::Result<()> {
    let home = primary_home()?;
    let user_home = std::env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine the home directory",
        )
    })?;
    let state_dir =
        std::path::PathBuf::from(user_home).join("Library/Application Support/AgentCompanion");
    std::fs::create_dir_all(&state_dir)?;
    let instance = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state_dir.join("notch.lock"))?;
    match instance.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            // SAFETY: notification-only main-thread bridge to the existing app.
            unsafe { agent_companion_reopen_settings() };
            return Ok(());
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    let state = SNAPSHOT.get_or_init(|| {
        Mutex::new(Snapshot {
            codex_home: home.to_string_lossy().into_owned(),
            loading: true,
            ..Snapshot::default()
        })
    });
    let (stop, stopped) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("codex-monitor".into())
        .spawn(move || {
            let mut dashboard = InstanceDashboards::new(home);
            loop {
                let snapshot = dashboard.poll(
                    crate::macos_deployment::active_instance(),
                    agent_companion_core::now_unix_secs(),
                );
                *state.lock().unwrap_or_else(|error| error.into_inner()) = snapshot;
                if stopped.recv_timeout(Duration::from_secs(2))
                    != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
        })?;
    // SAFETY: main calls this on the process main thread; AppKit owns the UI loop.
    let result = unsafe { agent_companion_run_notch() };
    let _ = stop.send(());
    let _ = worker.join();
    drop(instance);
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other("could not start the macOS notch"))
    }
}

struct InstanceDashboards {
    primary_home: std::path::PathBuf,
    primary: Dashboard,
    primary_instance: Instance,
    secondary: Option<(InstanceConfig, Dashboard)>,
}

impl InstanceDashboards {
    fn new(home: std::path::PathBuf) -> Self {
        let (instance, database) = Self::primary_descriptor(&home);
        Self {
            primary_home: home.clone(),
            primary_instance: instance,
            primary: Dashboard::with_database_home(home, database),
            secondary: None,
        }
    }

    fn primary_descriptor(home: &std::path::Path) -> (Instance, std::path::PathBuf) {
        let database = agent_companion_core::dashboard::database_home(home);
        let app = [
            std::path::PathBuf::from("/Applications/Codex.app"),
            home.parent().unwrap_or(home).join("Applications/Codex.app"),
        ]
        .into_iter()
        .find(|app| app.join("Contents/MacOS/ChatGPT").is_file());
        let executable = app
            .as_ref()
            .map(|app| app.join("Contents/Resources/codex"))
            .filter(|path| path.is_file());
        (
            Instance {
                instance_id: "codex".into(),
                label: "Codex".into(),
                codex_home: home.to_string_lossy().into_owned(),
                app_path: app.map(|path| path.to_string_lossy().into_owned()),
                executable_path: executable.map(|path| path.to_string_lossy().into_owned()),
                database_path: Some(database.to_string_lossy().into_owned()),
            },
            database,
        )
    }

    fn poll(&mut self, enabled: Option<InstanceConfig>, now: u64) -> Snapshot {
        // Installing Codex or changing its database location should recover
        // while Companion stays open, including after a failed deployment retry.
        let (primary, database) = Self::primary_descriptor(&self.primary_home);
        if primary.database_path != self.primary_instance.database_path {
            self.primary = Dashboard::with_database_home(self.primary_home.clone(), database);
        }
        self.primary_instance = primary;
        if self.secondary.as_ref().map(|(config, _)| config) != enabled.as_ref() {
            self.secondary = enabled.map(|config| {
                let dashboard = Dashboard::with_database_home(
                    config.codex_home.clone(),
                    config.database_dir.clone(),
                );
                (config, dashboard)
            });
        }
        let mut snapshots = vec![
            self.primary
                .poll(now)
                .with_instance(self.primary_instance.clone()),
        ];
        if let Some((config, dashboard)) = &mut self.secondary {
            snapshots.push(dashboard.poll(now).with_instance(Instance {
                instance_id: config.id.clone(),
                label: config.label.clone(),
                codex_home: config.codex_home.to_string_lossy().into_owned(),
                app_path: Some(config.runtime_app.to_string_lossy().into_owned()),
                executable_path: Some(config.cli_path.to_string_lossy().into_owned()),
                database_path: Some(config.database_dir.to_string_lossy().into_owned()),
            }));
        }
        Snapshot::merge(snapshots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_database_changes_refresh_metadata_and_reader_without_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut monitor = InstanceDashboards::new(root.path().to_owned());
        let first = monitor.poll(None, 1000);
        let database = root.path().join("custom-database");
        std::fs::write(
            root.path().join("config.toml"),
            format!("sqlite_home = {:?}\n", database.to_string_lossy()),
        )
        .unwrap();
        let next = monitor.poll(None, 1001);
        assert_ne!(
            first.instances[0].instance.database_path,
            next.instances[0].instance.database_path
        );
        assert_eq!(
            next.instances[0].instance.database_path.as_deref(),
            database.to_str()
        );
    }

    #[test]
    fn second_monitor_exists_only_while_explicitly_enabled() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("main");
        let second = root.path().join("second");
        let config = InstanceConfig {
            id: "dodex".into(),
            label: "Dodex".into(),
            codex_home: second.clone(),
            desktop_user_data: second.join("desktop"),
            database_dir: second.join("sqlite"),
            runtime_app: second.join("Runtime.app"),
            launcher_app: second.join("Dodex.app"),
            cli_path: second.join("Runtime.app/Contents/Resources/codex"),
        };
        let mut monitor = InstanceDashboards::new(home);
        assert_eq!(monitor.poll(None, 1000).instances.len(), 1);
        assert!(monitor.secondary.is_none() && !second.exists());
        let enabled = monitor.poll(Some(config), 1001);
        assert_eq!(enabled.instances.len(), 2);
        assert_eq!(enabled.instances[1].instance.instance_id, "dodex");
        assert!(monitor.secondary.is_some());
        assert_eq!(monitor.poll(None, 1002).instances.len(), 1);
        assert!(
            monitor.secondary.is_none(),
            "disable must drop second-instance caches"
        );
        assert!(
            !second.exists(),
            "monitoring must never create deployment files"
        );
    }
}
