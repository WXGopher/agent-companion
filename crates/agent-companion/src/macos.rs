//! Native menu bar shell; the independent Slint editor uses its own process.
use std::ffi::{CStr, CString, c_char};
use std::fs::OpenOptions;
use std::io;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use crate::macos_deployment::InstanceConfig;
use agent_companion_core::dashboard::{Dashboard, Instance, Snapshot};

static USAGE: OnceLock<Mutex<crate::usage_service::UsageService>> = OnceLock::new();
static UPDATE: OnceLock<crate::update_service::UpdateService> = OnceLock::new();

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();

/// Borrowed, NUL-terminated build version. The native UI must not free it.
#[unsafe(no_mangle)]
extern "C" fn agent_companion_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

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
    fn agent_companion_run_menu_bar() -> i32;
    fn agent_companion_reopen_menu();
    fn agent_companion_listen_software_updates();
}

pub fn listen_software_updates() {
    // The native listener is registered on the editor's main thread and emits
    // a PID-scoped readiness notification before the menu forwards a request.
    unsafe { agent_companion_listen_software_updates() }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn agent_companion_request_software_update(pointer: *const c_char) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: Swift borrows a NUL-terminated action string for this call.
    if let Ok(value) = unsafe { CStr::from_ptr(pointer) }.to_str()
        && let Some(action) = crate::software_updates::Action::parse(value)
    {
        crate::software_updates::request(action);
    }
}

/// Configure Slint before it creates its AppKit event loop, including when
/// launched as a bare CLI binary without an Info.plist.
pub fn prepare_editor() -> Result<(), slint::PlatformError> {
    use slint::winit_030::winit::{
        event_loop::EventLoop,
        platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
    };
    let mut builder = EventLoop::with_user_event();
    builder.with_activation_policy(ActivationPolicy::Accessory);
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_event_loop_builder(builder)
        // Slint defaults to a transparent native window. Painting the content
        // alone leaves AppKit's title bar clear; settings need an opaque frame.
        .with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false))
        .select()
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

fn update_service() -> &'static crate::update_service::UpdateService {
    UPDATE.get_or_init(crate::update_service::UpdateService::new)
}

/// Called only after an intentional closed-to-open native popup transition.
/// The service starts any eligible request in the background.
#[unsafe(no_mangle)]
extern "C" fn agent_companion_update_panel_open() {
    update_service().panel_open(agent_companion_core::now_unix_secs());
}

/// Read-only polling; ownership matches the shared JSON release function.
#[unsafe(no_mangle)]
extern "C" fn agent_companion_update_snapshot_json() -> *mut c_char {
    CString::new(serde_json::to_vec(&update_service().snapshot()).unwrap_or_default())
        .unwrap_or_default()
        .into_raw()
}

fn usage_service() -> &'static Mutex<crate::usage_service::UsageService> {
    USAGE.get_or_init(|| Mutex::new(crate::usage_service::UsageService::new()))
}

fn usage_json(service: &crate::usage_service::UsageService) -> *mut c_char {
    let value = serde_json::json!({
        "intervalMinutes": service.interval_minutes(),
        "instances": service.snapshots(),
    });
    CString::new(value.to_string())
        .unwrap_or_default()
        .into_raw()
}

/// Snapshot reads never schedule requests or decide whether a cache is fresh.
#[unsafe(no_mangle)]
extern "C" fn agent_companion_usage_snapshot_json() -> *mut c_char {
    let service = usage_service()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    usage_json(&service)
}

#[derive(serde::Deserialize)]
#[serde(tag = "event")]
enum UsageEvent {
    #[serde(rename = "panelOpen")]
    PanelOpen,
    #[serde(rename = "refresh")]
    Refresh {
        #[serde(rename = "instanceId")]
        instance_id: String,
    },
    #[serde(rename = "history")]
    History {
        #[serde(rename = "instanceId")]
        instance_id: String,
    },
}

fn apply_usage_event(
    service: &mut crate::usage_service::UsageService,
    event: UsageEvent,
    now: u64,
) {
    match event {
        UsageEvent::PanelOpen => service.panel_open(now),
        UsageEvent::Refresh { instance_id } => service.refresh(&instance_id, now),
        UsageEvent::History { instance_id } => service.load_history(&instance_id, now),
    }
}

/// Explicit UI boundaries only: opening a panel, quota refresh, or history entry.
#[unsafe(no_mangle)]
unsafe extern "C" fn agent_companion_usage_event(pointer: *const c_char) -> *mut c_char {
    let mut service = usage_service()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !pointer.is_null() {
        // SAFETY: Swift borrows a NUL-terminated UTF-8 string for this call.
        let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
        if let Ok(event) = serde_json::from_slice::<UsageEvent>(bytes) {
            apply_usage_event(&mut service, event, agent_companion_core::now_unix_secs());
        }
    }
    usage_json(&service)
}

pub fn run_menu_bar() -> io::Result<()> {
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
        // Keep the historical filename to exclude older running versions too.
        .open(state_dir.join("notch.lock"))?;
    match instance.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            // SAFETY: notification-only main-thread bridge to the existing app.
            unsafe { agent_companion_reopen_menu() };
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
            // Publish routing before scanning session history so account queries
            // can start independently, including on a large first task scan.
            *state.lock().unwrap_or_else(|error| error.into_inner()) =
                dashboard.routing_snapshot(crate::macos_deployment::active_instance().as_ref());
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
    let (stop_usage, usage_stopped) = mpsc::channel();
    let usage_worker = std::thread::Builder::new()
        .name("account-usage-monitor".into())
        .spawn(move || {
            loop {
                let sources = {
                    let snapshot = state.lock().unwrap_or_else(|error| error.into_inner());
                    snapshot
                        .instances
                        .iter()
                        .map(|snapshot| {
                            let instance = &snapshot.instance;
                            crate::usage_service::Source {
                                instance_id: instance.instance_id.clone(),
                                codex_home: instance.codex_home.clone().into(),
                                executable_path: instance.executable_path.clone().map(Into::into),
                                database_path: instance
                                    .database_path
                                    .clone()
                                    .unwrap_or_else(|| instance.codex_home.clone())
                                    .into(),
                            }
                        })
                        .collect()
                };
                {
                    let now = agent_companion_core::now_unix_secs();
                    let mut usage = usage_service()
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    usage.sync_sources(sources, now);
                    usage.tick(now);
                }
                if usage_stopped.recv_timeout(Duration::from_millis(100))
                    != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
        })?;
    // SAFETY: main calls this on the process main thread; AppKit owns the UI loop.
    let result = unsafe { agent_companion_run_menu_bar() };
    let _ = stop.send(());
    let _ = stop_usage.send(());
    let _ = usage_worker.join();
    let _ = worker.join();
    usage_service()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .stop();
    drop(instance);
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other("could not start the macOS menu bar app"))
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
        let system_applications = std::path::Path::new("/Applications");
        let user_applications = home.parent().unwrap_or(home).join("Applications");
        Self::primary_descriptor_in(home, system_applications, &user_applications)
    }

    fn primary_descriptor_in(
        home: &std::path::Path,
        system_applications: &std::path::Path,
        user_applications: &std::path::Path,
    ) -> (Instance, std::path::PathBuf) {
        let database = agent_companion_core::dashboard::database_home(home);
        let found = crate::macos_primary_app::discover(system_applications, user_applications);
        let (app, executable) = found
            .map(|found| (Some(found.app), found.executable))
            .unwrap_or_default();
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

    fn routing_snapshot(&self, enabled: Option<&InstanceConfig>) -> Snapshot {
        let pending = |instance: Instance| {
            Snapshot {
                codex_home: instance.codex_home.clone(),
                loading: true,
                ..Snapshot::default()
            }
            .with_instance(instance)
        };
        let mut instances = vec![pending(self.primary_instance.clone())];
        if let Some(config) = enabled {
            instances.push(pending(Self::secondary_descriptor(config)));
        }
        Snapshot::merge(instances)
    }

    fn secondary_descriptor(config: &InstanceConfig) -> Instance {
        Instance {
            instance_id: config.id.clone(),
            label: config.label.clone(),
            codex_home: config.codex_home.to_string_lossy().into_owned(),
            app_path: Some(config.runtime_app.to_string_lossy().into_owned()),
            executable_path: Some(config.cli_path.to_string_lossy().into_owned()),
            database_path: Some(config.database_dir.to_string_lossy().into_owned()),
        }
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
                .poll_tasks(now)
                .with_instance(self.primary_instance.clone()),
        ];
        if let Some((config, dashboard)) = &mut self.secondary {
            snapshots.push(
                dashboard
                    .poll_tasks(now)
                    .with_instance(Self::secondary_descriptor(config)),
            );
        }
        Snapshot::merge(snapshots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ffi_accepts_only_explicit_query_events() {
        for event in [
            r#"{"event":"panelOpen"}"#,
            r#"{"event":"refresh","instanceId":"dodex"}"#,
            r#"{"event":"history","instanceId":"codex"}"#,
        ] {
            assert!(serde_json::from_str::<UsageEvent>(event).is_ok());
        }
        for event in [
            "render",
            "taskActivity",
            "hover",
            "close",
            "reset",
            "switchInstance",
        ] {
            assert!(
                serde_json::from_value::<UsageEvent>(serde_json::json!({"event":event})).is_err()
            );
        }
    }

    #[test]
    fn usage_ffi_snapshot_serializes_without_starting_a_query() {
        let root = tempfile::tempdir().unwrap();
        let service =
            crate::usage_service::UsageService::with_settings_path(root.path().join("usage.json"));
        let pointer = usage_json(&service);
        let value: serde_json::Value =
            unsafe { serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()).unwrap() };
        unsafe { agent_companion_release_json(pointer) };
        assert_eq!(value["intervalMinutes"], 5);
        assert_eq!(value["instances"], serde_json::json!([]));
        assert!(!root.path().join("usage.json").exists());
    }

    #[test]
    fn renamed_primary_descriptor_routes_app_and_cli_to_the_same_primary_bundle() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let system = root.join("Applications");
        let user = root.join("user/Applications");
        let home = root.join("user/.codex");
        let app = system.join("ChatGPT.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), b"<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>com.openai.codex</string></dict></plist>").unwrap();
        for relative in ["Contents/MacOS/ChatGPT", "Contents/Resources/codex"] {
            let file = app.join(relative);
            std::fs::write(&file, "synthetic executable, never run").unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (instance, database) = InstanceDashboards::primary_descriptor_in(&home, &system, &user);
        assert_eq!(instance.instance_id, "codex");
        assert_eq!(instance.codex_home, home.to_string_lossy());
        assert_eq!(instance.app_path.as_deref(), app.to_str());
        assert_eq!(
            instance.executable_path.as_deref(),
            app.join("Contents/Resources/codex").to_str()
        );
        assert_eq!(instance.database_path.as_deref(), database.to_str());
    }

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
        let routing = monitor.routing_snapshot(Some(&config));
        assert!(routing.loading && routing.tasks.is_empty());
        assert_eq!(routing.instances.len(), 2);
        assert_eq!(
            routing.instances[1].instance.executable_path.as_deref(),
            config.cli_path.to_str()
        );
        assert!(
            monitor.secondary.is_none(),
            "routing does not require a task scan"
        );
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
