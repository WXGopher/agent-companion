//! Read-only release awareness. Opening the panel may start a daily check;
//! reading the snapshot never schedules work or touches the network.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use semver::Version;
use serde::{Deserialize, Serialize};

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 16 * 1024;
const MAX_TAG_BYTES: usize = 128;
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/WXGopher/agent-companion/releases/latest";
const RELEASE_URL_PREFIX: &str = "https://github.com/WXGopher/agent-companion/releases/tag/";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSnapshot {
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Cache {
    last_attempt_at: Option<u64>,
    // Persist only a validated tag. The release URL always uses our fixed repo.
    latest_tag: Option<String>,
}

struct State {
    cache: Cache,
    in_flight: bool,
}

type Fetch = dyn Fn() -> io::Result<Vec<u8>> + Send + Sync;

struct Inner {
    state: Mutex<State>,
    path: Option<PathBuf>,
    current_version: Version,
    fetch: Box<Fetch>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn save(&self, cache: &Cache) {
        if let Some(path) = &self.path {
            // A read-only/unavailable config directory must not disrupt the UI.
            let _ = save_cache(path, cache);
        }
    }
}

pub struct UpdateService {
    inner: Arc<Inner>,
}

impl UpdateService {
    pub fn new() -> Self {
        let path = agent_companion_core::usage_service::settings_path()
            .ok()
            .map(|path| path.with_file_name("updates.json"));
        Self::create(path, env!("CARGO_PKG_VERSION"), Box::new(fetch_latest))
    }

    fn create(path: Option<PathBuf>, current_version: &str, fetch: Box<Fetch>) -> Self {
        let cache = path.as_deref().map(load_cache).unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    cache,
                    in_flight: false,
                }),
                path,
                current_version: Version::parse(current_version)
                    .expect("Cargo package versions are semantic versions"),
                fetch,
            }),
        }
    }

    /// `now` is Unix seconds. Failed attempts share the same daily throttle as
    /// successes. No network or disk IO runs on this caller's thread.
    pub fn panel_open(&self, now: u64) {
        let attempt = {
            let mut state = self.inner.state();
            if state.in_flight
                || state
                    .cache
                    .last_attempt_at
                    .is_some_and(|last| now.saturating_sub(last) < CHECK_INTERVAL_SECS)
            {
                return;
            }
            state.in_flight = true;
            state.cache.last_attempt_at = Some(now);
            state.cache.clone()
        };

        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new()
            .name("release-check".into())
            .spawn(move || {
                // Save before fetching so quitting during a failed/slow check
                // cannot cause another request at the next startup.
                inner.save(&attempt);
                let result = (inner.fetch)().and_then(|body| parse_release(&body));
                let completed = {
                    let mut state = inner.state();
                    if let Ok(tag) = result {
                        state.cache.latest_tag = Some(tag);
                    }
                    state.cache.clone()
                };
                // Keep the in-flight gate until persistence completes, without
                // holding the state mutex over either filesystem operation.
                inner.save(&completed);
                inner.state().in_flight = false;
            });
        if spawned.is_err() {
            self.inner.state().in_flight = false;
        }
    }

    /// A cached release stops being advertised after the app is upgraded to it.
    /// Background completions become visible here without reopening the panel.
    pub fn snapshot(&self) -> UpdateSnapshot {
        let tag = self.inner.state().cache.latest_tag.clone();
        let Some((tag, version)) = tag
            .and_then(|tag| stable_version(&tag).map(|version| (tag, version)))
            .filter(|(_, version)| version.cmp_precedence(&self.inner.current_version).is_gt())
        else {
            return UpdateSnapshot::default();
        };
        UpdateSnapshot {
            latest_version: Some(version.to_string()),
            release_url: Some(format!("{RELEASE_URL_PREFIX}{tag}")),
        }
    }
}

fn request_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .https_only(true)
        .max_redirects(0)
        .user_agent(concat!("agent-companion/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn fetch_latest() -> io::Result<Vec<u8>> {
    let mut response = request_config()
        .new_agent()
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(io::Error::other)?;
    // Disabled redirects and unexpected success statuses are not release data.
    if response.status() != 200 {
        return Err(io::Error::other("Unexpected release response status"));
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(io::Error::other)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn parse_release(body: &[u8]) -> io::Result<String> {
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(io::Error::other("Release response is too large"));
    }
    let release: Release = serde_json::from_slice(body)?;
    if release.draft || release.prerelease || stable_version(&release.tag_name).is_none() {
        return Err(io::Error::other("Release is not a stable semantic version"));
    }
    // GitHub's html_url is intentionally ignored: even cached and remote data
    // can only produce a release URL under this repository's canonical path.
    Ok(release.tag_name)
}

fn stable_version(tag: &str) -> Option<Version> {
    if tag.len() > MAX_TAG_BYTES {
        return None;
    }
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    version.pre.is_empty().then_some(version)
}

fn load_cache(path: &Path) -> Cache {
    let read = || -> io::Result<Cache> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(MAX_CACHE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err(io::Error::other("Release cache is too large"));
        }
        let mut cache: Cache = serde_json::from_slice(&bytes)?;
        if cache
            .latest_tag
            .as_deref()
            .is_some_and(|tag| stable_version(tag).is_none())
        {
            cache.latest_tag = None;
        }
        Ok(cache)
    };
    read().unwrap_or_default()
}

fn save_cache(path: &Path, cache: &Cache) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".updates-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(cache)?)?;
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

#[cfg(test)]
#[path = "update_service/tests.rs"]
mod tests;
