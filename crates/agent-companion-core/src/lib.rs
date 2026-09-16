//! Shared types and logic for Agent Companion.
//!
//! The hook binary and the desktop app both depend on this crate so that the
//! wire format, on-disk state, and installation layout stay in sync.
//!
//! `config-edit` enables TOML persistence without the background service.
//! `server` adds the Windows backend dependencies; the hook uses neither feature.

pub mod codex;
pub mod compat;
pub mod dashboard;
pub mod install;
pub mod pipe;
pub mod protocol;
pub mod questions;
#[cfg(all(windows, feature = "server"))]
pub mod server;
pub mod state;
pub mod transcript;
pub mod usage;

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, or `0` if the clock is before it.
///
/// Every module that needs wall-clock time takes it as an argument instead of
/// reading it, so the reducers and parsers below stay deterministic under test.
/// This is the one place that actually looks at the clock.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
