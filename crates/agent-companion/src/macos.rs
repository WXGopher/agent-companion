//! Native notch shell; the independent Slint editor uses its own process.
use std::ffi::{CString, c_char};
use std::fs::OpenOptions;
use std::io;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use agent_companion_core::dashboard::{Dashboard, Snapshot};

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();

unsafe extern "C" {
    fn agent_companion_run_notch() -> i32;
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
