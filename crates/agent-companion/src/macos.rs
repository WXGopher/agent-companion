//! Native notch shell; the independent Slint editor uses its own process.
use std::ffi::{CStr, CString, c_char};
use std::fs::OpenOptions;
use std::io;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use agent_companion_core::dashboard::{Dashboard, Snapshot};

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();

unsafe extern "C" {
    fn agent_companion_run_notch() -> i32;
    fn agent_companion_prepare_editor() -> bool;
    fn agent_companion_start_editor_preferences();
    fn agent_companion_dock_visible() -> bool;
    fn agent_companion_set_dock_visible(visible: bool) -> bool;
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
    let home = agent_companion_core::install::codex_home()?;
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
        Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
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
            let mut dashboard = Dashboard::new(home);
            loop {
                let snapshot = dashboard.poll(agent_companion_core::now_unix_secs());
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
