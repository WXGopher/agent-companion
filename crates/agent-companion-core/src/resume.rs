//! Resume a native thread using its original state and an explicitly selected
//! file-backed account. No transcript is copied, summarized, or synchronized.
//!
//! The home-overlay design and explicit-ID recovery were inspired by
//! xhyqaq/codex-auto. See THIRD_PARTY_NOTICES.md and its retained MIT license.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod config;
mod discovery;
mod runtime;

pub use discovery::discover_sessions;
pub use runtime::{
    PreparedResume, cleanup_stale, inspect, prepare, prepare_from_inspection, verify_runtime,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub label: String,
    pub home: PathBuf,
    /// The native source runtime, never a shell wrapper that resets CODEX_HOME.
    pub executable: PathBuf,
    pub database_home: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: PathBuf,
    pub rollout_path: PathBuf,
    /// Storage provenance. This never changes when the quota account changes.
    pub environment_id: String,
    pub modified_secs: u64,
    /// Old metadata proves storage, not the original account or environment.
    pub creation_source: Option<String>,
    pub busy: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Provenance {
    /// Redacted, display-ready summary cells.
    pub rows: Vec<(String, String)>,
    /// Paths and explicitly allowlisted values only; never raw configuration.
    pub details: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResumeInspection {
    pub provenance: Provenance,
    pub blockers: Vec<String>,
    pub account_label: String,
    /// Non-secret identity selected in the UI, bound again during preparation.
    pub account_identity: Option<AccountIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountIdentity {
    pub workspace_id: String,
    pub user_id: String,
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn display_path(path: &Path) -> String {
    safe_text(&path.to_string_lossy())
}

/// Native records/configuration may contain terminal escape sequences.
fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(500)
        .collect()
}
