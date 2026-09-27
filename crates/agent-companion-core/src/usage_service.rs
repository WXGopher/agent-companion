//! Account usage scheduling shared by the desktop surfaces. Time and query
//! results enter explicitly, so reading a snapshot never starts work.
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_INTERVAL_MINUTES: u8 = 5;
pub const HISTORY_CACHE_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub instance_id: String,
    pub codex_home: PathBuf,
    pub executable_path: Option<PathBuf>,
    pub database_path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuerySnapshot {
    /// Last successful response, including a response containing no windows.
    pub value: Option<Value>,
    pub last_success_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub loading: bool,
    /// The latest failure remains visible while another read is in progress.
    pub error: Option<String>,
    pub next_query_at: Option<u64>,
    pub elapsed_ms: Option<u64>,
}

impl QuerySnapshot {
    /// Asterisks describe a failed query, never elapsed time or a passed reset.
    pub fn failed_reading(&self) -> bool {
        self.value.is_some() && self.error.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSnapshot {
    pub source: Source,
    pub limits: QuerySnapshot,
    pub history: QuerySnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryKind {
    Limits,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub source: Source,
    pub kind: QueryKind,
}

struct InstanceState {
    snapshot: InstanceSnapshot,
    limits_request: Option<u64>,
    history_request: Option<u64>,
}

/// Pure state machine. The application executes returned requests and cancels
/// any requests for which [`Self::is_current`] is no longer true.
pub struct Scheduler {
    interval_minutes: u8,
    next_id: u64,
    instances: BTreeMap<String, InstanceState>,
}

impl Scheduler {
    pub fn new(interval_minutes: u8) -> Self {
        Self {
            interval_minutes: valid_interval(interval_minutes).unwrap_or(DEFAULT_INTERVAL_MINUTES),
            next_id: 0,
            instances: BTreeMap::new(),
        }
    }

    /// Startup and newly enabled/replaced sources start one allowance query.
    pub fn sync_sources(&mut self, sources: Vec<Source>, now: u64) -> Vec<Request> {
        let sources: BTreeMap<_, _> = sources
            .into_iter()
            .map(|source| (source.instance_id.clone(), source))
            .collect();
        self.instances
            .retain(|id, state| sources.get(id) == Some(&state.snapshot.source));
        let mut added = Vec::new();
        for (id, source) in sources {
            if !self.instances.contains_key(&id) {
                self.instances.insert(
                    id.clone(),
                    InstanceState {
                        snapshot: InstanceSnapshot {
                            source,
                            limits: QuerySnapshot::default(),
                            history: QuerySnapshot::default(),
                        },
                        limits_request: None,
                        history_request: None,
                    },
                );
                added.push(id);
            }
        }
        added
            .into_iter()
            .filter_map(|id| self.start(&id, QueryKind::Limits, now))
            .collect()
    }

    /// No catch-up queue: a resumed machine gets at most one query per source.
    pub fn tick(&mut self, now: u64) -> Vec<Request> {
        let due: Vec<_> = self
            .instances
            .iter()
            .filter_map(|(id, state)| {
                state
                    .snapshot
                    .limits
                    .next_query_at
                    .filter(|at| *at <= now)
                    .map(|_| id.clone())
            })
            .collect();
        due.into_iter()
            .filter_map(|id| self.start(&id, QueryKind::Limits, now))
            .collect()
    }

    pub fn panel_open(&mut self, now: u64) -> Vec<Request> {
        let ids: Vec<_> = self.instances.keys().cloned().collect();
        ids.into_iter()
            .filter_map(|id| self.start(&id, QueryKind::Limits, now))
            .collect()
    }

    pub fn refresh(&mut self, id: &str, now: u64) -> Vec<Request> {
        self.start(id, QueryKind::Limits, now).into_iter().collect()
    }

    /// Called only for Usage-page entry or an instance change on that page.
    /// Failed and successful history reads share the same completion-based TTL.
    pub fn load_history(&mut self, id: &str, now: u64) -> Vec<Request> {
        if self.instances.get(id).is_some_and(|state| {
            state
                .snapshot
                .history
                .next_query_at
                .is_some_and(|at| now < at)
        }) {
            return Vec::new();
        }
        self.start(id, QueryKind::History, now)
            .into_iter()
            .collect()
    }

    fn start(&mut self, id: &str, kind: QueryKind, _now: u64) -> Option<Request> {
        let state = self.instances.get_mut(id)?;
        let (snapshot, pending) = match kind {
            QueryKind::Limits => (&mut state.snapshot.limits, &mut state.limits_request),
            QueryKind::History => (&mut state.snapshot.history, &mut state.history_request),
        };
        if pending.is_some() {
            return None;
        }
        self.next_id += 1;
        *pending = Some(self.next_id);
        snapshot.loading = true;
        // Preserve both the value and any previous error until completion.
        Some(Request {
            id: self.next_id,
            source: state.snapshot.source.clone(),
            kind,
        })
    }

    pub fn is_current(&self, request: &Request) -> bool {
        self.instances
            .get(&request.source.instance_id)
            .is_some_and(|state| {
                state.snapshot.source == request.source
                    && match request.kind {
                        QueryKind::Limits => state.limits_request == Some(request.id),
                        QueryKind::History => state.history_request == Some(request.id),
                    }
            })
    }

    pub fn complete(
        &mut self,
        request: &Request,
        result: Result<Value, String>,
        now: u64,
        elapsed_ms: u64,
    ) -> bool {
        if !self.is_current(request) {
            return false;
        }
        let state = self.instances.get_mut(&request.source.instance_id).unwrap();
        let (snapshot, pending, interval) = match request.kind {
            QueryKind::Limits => (
                &mut state.snapshot.limits,
                &mut state.limits_request,
                u64::from(self.interval_minutes) * 60,
            ),
            QueryKind::History => (
                &mut state.snapshot.history,
                &mut state.history_request,
                HISTORY_CACHE_SECS,
            ),
        };
        *pending = None;
        snapshot.loading = false;
        snapshot.completed_at = Some(now);
        snapshot.elapsed_ms = Some(elapsed_ms);
        snapshot.next_query_at = Some(now.saturating_add(interval));
        match result {
            Ok(value) => {
                snapshot.value = Some(value);
                snapshot.last_success_at = Some(now);
                snapshot.error = None;
            }
            Err(error) => snapshot.error = Some(error),
        }
        true
    }

    pub fn interval_minutes(&self) -> u8 {
        self.interval_minutes
    }

    /// Rebase every allowance timer without querying or invalidating values.
    pub fn set_interval(&mut self, minutes: u8, now: u64) -> io::Result<()> {
        let minutes = valid_interval(minutes)?;
        self.interval_minutes = minutes;
        for state in self.instances.values_mut() {
            state.snapshot.limits.next_query_at = Some(now.saturating_add(u64::from(minutes) * 60));
        }
        Ok(())
    }

    pub fn snapshot(&self, id: &str) -> Option<InstanceSnapshot> {
        self.instances.get(id).map(|state| state.snapshot.clone())
    }

    pub fn snapshots(&self) -> Vec<InstanceSnapshot> {
        self.instances
            .values()
            .map(|state| state.snapshot.clone())
            .collect()
    }

    pub fn clear(&mut self) {
        self.instances.clear();
    }
}

fn valid_interval(minutes: u8) -> io::Result<u8> {
    if (1..=60).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage refresh interval must be an integer from 1 to 60 minutes.",
        ))
    }
}

/// Shared UI validation; accept decimal integer digits only, not a rounded
/// floating-point value, exponent, sign, or unit suffix.
pub fn parse_interval(input: &str) -> io::Result<u8> {
    if input.is_empty() || !input.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage refresh interval must be an integer from 1 to 60 minutes.",
        ));
    }
    let minutes = input.parse::<u8>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage refresh interval must be an integer from 1 to 60 minutes.",
        )
    })?;
    valid_interval(minutes)
}

/// This file contains preferences only; account readings are never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageSettings {
    pub refresh_interval_minutes: u8,
}

impl Default for UsageSettings {
    fn default() -> Self {
        Self {
            refresh_interval_minutes: DEFAULT_INTERVAL_MINUTES,
        }
    }
}

impl UsageSettings {
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|settings| valid_interval(settings.refresh_interval_minutes).is_ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        valid_interval(self.refresh_interval_minutes)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let temporary = parent.join(format!(
            ".usage-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

pub fn settings_path() -> io::Result<PathBuf> {
    if let Some(path) =
        crate::compat::var_os("AGENT_COMPANION_CONFIG_DIR").filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(path).join("usage.json"));
    }
    #[cfg(windows)]
    let directory = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("AgentCompanion"));
    #[cfg(target_os = "macos")]
    let directory = std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("Library/Application Support/AgentCompanion"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".config"))
        })
        .map(|path| path.join("AgentCompanion"));
    directory
        .map(|directory| directory.join("usage.json"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Application configuration directory is unavailable.",
            )
        })
}

#[cfg(test)]
mod tests;
