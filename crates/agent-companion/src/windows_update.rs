//! Windows package updates. Network and staging work never runs on the UI thread.
//!
//! The copied helper owns an exact handle to the old process before the UI may
//! quit. It changes only the release package entries in the current directory;
//! CLI aliases, TUI installations and user configuration are separate owners.
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ExitCode},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

mod native;
mod package;
mod transaction;

const REPOSITORY: &str = "WXGopher/agent-companion";
const RELEASE_PREFIX: &str = "https://github.com/WXGopher/agent-companion/releases/tag/";
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_CHECKSUMS: u64 = 64 * 1024;
const MAX_ARCHIVE: u64 = 384 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Idle,
    Downloading,
    Verifying,
    Preparing,
    Restarting,
    Failed,
}

impl Stage {
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Downloading | Self::Verifying | Self::Preparing | Self::Restarting
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub stage: Stage,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub message: String,
}

struct State {
    snapshot: Snapshot,
    helper: Option<Child>,
    restart_ready: bool,
}

struct Inner {
    state: Mutex<State>,
    result_path: Option<PathBuf>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn progress(&self, stage: Stage, message: &str, downloaded: u64, total: Option<u64>) {
        self.state().snapshot = Snapshot {
            stage,
            downloaded,
            total,
            message: message.into(),
        };
    }

    fn fail(&self, message: String) {
        let mut state = self.state();
        state.snapshot.stage = Stage::Failed;
        state.snapshot.message = message;
        state.restart_ready = false;
    }
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

impl Manager {
    pub fn new() -> Self {
        let result_path = agent_companion_core::usage_service::settings_path()
            .ok()
            .map(|path| path.with_file_name("windows-update-result.json"));
        let snapshot = result_path
            .as_deref()
            .and_then(transaction::read_result)
            .map(|message| Snapshot {
                stage: Stage::Failed,
                message,
                ..Snapshot::default()
            })
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    snapshot,
                    helper: None,
                    restart_ready: false,
                }),
                result_path,
            }),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.inner.state();
        if state.snapshot.stage == Stage::Restarting
            && state
                .helper
                .as_mut()
                .is_some_and(|helper| !matches!(helper.try_wait(), Ok(None)))
        {
            state.snapshot.stage = Stage::Failed;
            state.snapshot.message = self
                .inner
                .result_path
                .as_deref()
                .and_then(transaction::read_result)
                .unwrap_or_else(|| {
                    "The update helper stopped before the app exited. Please try again.".into()
                });
            state.restart_ready = false;
            state.helper = None;
        }
        state.snapshot.clone()
    }

    pub fn start(&self, update: &crate::update_service::UpdateSnapshot) -> Result<(), String> {
        let selected = Selection::from_snapshot(
            update,
            env!("AGENT_COMPANION_SEMVER_VERSION"),
            !cfg!(companion_local_build),
            cfg!(target_arch = "x86_64"),
        )?;
        {
            let mut state = self.inner.state();
            if state.snapshot.stage.is_busy() {
                return Err("An update is already in progress.".into());
            }
            state.snapshot = Snapshot {
                stage: Stage::Downloading,
                message: "Downloading the official Windows package…".into(),
                ..Snapshot::default()
            };
            state.restart_ready = false;
            state.helper = None;
        }
        let inner = Arc::clone(&self.inner);
        if let Err(error) = std::thread::Builder::new()
            .name("windows-update".into())
            .spawn(move || match prepare(&selected, &inner) {
                Ok(helper) => {
                    let mut state = inner.state();
                    state.helper = Some(helper);
                    state.snapshot.stage = Stage::Restarting;
                    state.snapshot.message =
                        "Restarting Agent Companion to finish the update…".into();
                    state.restart_ready = true;
                }
                Err(error) => inner.fail(format!("Update failed: {error}")),
            })
        {
            let message = format!("Cannot start the update worker: {error}");
            self.inner.fail(message.clone());
            return Err(message);
        }
        Ok(())
    }

    /// Consume readiness once, only while the initialized helper is alive.
    pub fn take_restart_ready(&self) -> bool {
        let mut state = self.inner.state();
        if !state.restart_ready {
            return false;
        }
        let alive = state
            .helper
            .as_mut()
            .is_some_and(|helper| matches!(helper.try_wait(), Ok(None)));
        state.restart_ready = false;
        if !alive {
            state.snapshot.stage = Stage::Failed;
            state.snapshot.message =
                "The update helper stopped before restart. Please try again.".into();
        }
        alive
    }
}

/// Dispatch this before console attachment, Clap and single-instance handling.
pub fn try_run_helper() -> Option<ExitCode> {
    transaction::try_run_helper()
}

/// Called after the restarted GUI has bound its own pipe and entered its event loop.
pub fn acknowledge_startup() {
    transaction::acknowledge_startup();
}

/// Updater children must fail if another app owns the pipe instead of taking it over.
pub fn is_update_restart() -> bool {
    transaction::is_update_restart()
}

#[derive(Clone, Debug)]
struct Selection {
    tag: String,
    version: String,
    archive: String,
}

impl Selection {
    fn from_snapshot(
        update: &crate::update_service::UpdateSnapshot,
        current: &str,
        enabled: bool,
        supported_arch: bool,
    ) -> Result<Self, String> {
        if !enabled {
            return Err("Local builds cannot install release updates.".into());
        }
        if !supported_arch {
            return Err("Automatic updates support Windows x86_64 only.".into());
        }
        let version = update
            .latest_version
            .as_deref()
            .and_then(stable_version)
            .ok_or("No supported update is selected.")?;
        let current = Version::parse(current).map_err(|_| "The current version is invalid.")?;
        if !version.cmp_precedence(&current).is_gt() {
            return Err("The selected release is not newer than this installation.".into());
        }
        let tag = update
            .release_url
            .as_deref()
            .and_then(|url| url.strip_prefix(RELEASE_PREFIX))
            .filter(|tag| stable_version(tag).as_ref() == Some(&version))
            .ok_or("The selected release is not from the official repository.")?;
        Ok(Self {
            tag: tag.into(),
            version: version.to_string(),
            archive: format!("agent-companion-v{version}-windows-x86_64.zip"),
        })
    }
}

fn stable_version(tag: &str) -> Option<Version> {
    if tag.len() > 128
        || !tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return None;
    }
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    version.pre.is_empty().then_some(version)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Deserialize)]
struct Asset {
    id: u64,
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

fn select_assets(body: &[u8], selected: &Selection) -> io::Result<(Asset, Asset)> {
    if body.len() as u64 > MAX_METADATA {
        return Err(io::Error::other("Release metadata is too large"));
    }
    let release: Release = serde_json::from_slice(body)?;
    if release.draft || release.prerelease || release.tag_name != selected.tag {
        return Err(io::Error::other(
            "The selected official stable release is unavailable",
        ));
    }
    let find = |name: &str, maximum: u64| -> io::Result<Asset> {
        let mut assets = release.assets.iter().filter(|asset| asset.name == name);
        let asset = assets
            .next()
            .ok_or_else(|| io::Error::other(format!("Release is missing {name}")))?;
        let expected = format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
            selected.tag
        );
        if assets.next().is_some()
            || asset.id == 0
            || asset.size == 0
            || asset.size > maximum
            || asset.browser_download_url != expected
        {
            return Err(io::Error::other(
                "Release asset identity or size is invalid",
            ));
        }
        if asset.digest.as_deref().is_some_and(|digest| {
            parse_digest(digest.strip_prefix("sha256:").unwrap_or("")).is_none()
        }) {
            return Err(io::Error::other(
                "Release asset has an unsupported checksum",
            ));
        }
        Ok(asset.clone())
    };
    let archive = find(&selected.archive, MAX_ARCHIVE)?;
    let sums = find("SHA256SUMS.txt", MAX_CHECKSUMS)?;
    if archive.id == sums.id {
        return Err(io::Error::other("Release asset IDs are duplicated"));
    }
    Ok((archive, sums))
}

fn prepare(selected: &Selection, inner: &Inner) -> io::Result<Child> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let directory = transaction::validate_install(&executable)?;
    let _directory_pin = transaction::pin_directory(&directory)?;
    let temporary = tempfile::Builder::new()
        .prefix(".agent-companion-update-")
        .tempdir_in(&directory)?;
    let metadata = download_bytes(
        &format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/tags/{}",
            selected.tag
        ),
        MAX_METADATA,
        false,
    )?;
    let (asset, sums) = select_assets(&metadata, selected)?;
    let checksums = download_bytes(&sums.browser_download_url, MAX_CHECKSUMS, true)?;
    if checksums.len() as u64 != sums.size {
        return Err(io::Error::other("Checksum asset size changed"));
    }
    verify_asset_digest(&checksums, &sums)?;
    let expected = checksum_for(&checksums, &selected.archive)?;
    if let Some(digest) = &asset.digest
        && digest
            .strip_prefix("sha256:")
            .and_then(parse_digest)
            .as_deref()
            != Some(&expected)
    {
        return Err(io::Error::other(
            "GitHub and SHA256SUMS disagree about the package checksum",
        ));
    }
    let archive_path = temporary.path().join("download.zip");
    let mut archive = File::create(&archive_path)?;
    download(
        &asset.browser_download_url,
        MAX_ARCHIVE,
        true,
        |reader, length| {
            if length.is_some_and(|length| length != asset.size) {
                return Err(io::Error::other(
                    "Package response size does not match the release",
                ));
            }
            copy_bounded(reader, &mut archive, MAX_ARCHIVE, |downloaded| {
                inner.progress(
                    Stage::Downloading,
                    "Downloading the official Windows package…",
                    downloaded,
                    Some(asset.size),
                );
            })
            .and_then(|count| {
                if count == asset.size {
                    Ok(())
                } else {
                    Err(io::Error::other("Package download was incomplete"))
                }
            })
        },
    )?;
    archive.sync_all()?;
    drop(archive);
    inner.progress(
        Stage::Verifying,
        "Verifying the downloaded package…",
        asset.size,
        Some(asset.size),
    );
    if hash_file(&archive_path)? != expected {
        return Err(io::Error::other("Package SHA256 verification failed"));
    }
    let payload = temporary.path().join("payload");
    fs::create_dir(&payload)?;
    let entries = package::extract(&archive_path, &payload)?;
    inner.progress(
        Stage::Preparing,
        "Preparing a safe restart…",
        asset.size,
        Some(asset.size),
    );
    transaction::prepare_helper(
        temporary,
        executable,
        selected,
        entries,
        inner.result_path.clone(),
    )
}

fn parse_digest(value: &str) -> Option<String> {
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn verify_asset_digest(bytes: &[u8], asset: &Asset) -> io::Result<()> {
    if asset.digest.as_deref().is_some_and(|expected| {
        expected
            .strip_prefix("sha256:")
            .and_then(parse_digest)
            .as_deref()
            != Some(&format!("{:x}", Sha256::digest(bytes)))
    }) {
        return Err(io::Error::other(
            "Checksum asset integrity verification failed",
        ));
    }
    Ok(())
}

fn checksum_for(bytes: &[u8], filename: &str) -> io::Result<String> {
    let text = std::str::from_utf8(bytes)
        .map_err(io::Error::other)?
        .trim_start_matches('\u{feff}');
    let mut found = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let hash = parts
            .next()
            .and_then(parse_digest)
            .ok_or_else(|| io::Error::other("Invalid SHA256SUMS entry"))?;
        let name = parts
            .next()
            .ok_or_else(|| io::Error::other("Invalid SHA256SUMS filename"))?
            .trim_start_matches('*');
        if parts.next().is_some() {
            return Err(io::Error::other("Invalid SHA256SUMS entry"));
        }
        if name == filename && found.replace(hash).is_some() {
            return Err(io::Error::other("Duplicate package checksum"));
        }
    }
    found.ok_or_else(|| io::Error::other("SHA256SUMS is missing the selected package"))
}

fn request_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .https_only(true)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .user_agent(concat!("agent-companion/", env!("AGENT_COMPANION_VERSION")))
        .build()
}

fn safe_asset_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "github-releases.githubusercontent.com"
            )
        )
}

fn download<T>(
    url: &str,
    limit: u64,
    redirects: bool,
    consume: impl FnOnce(&mut dyn Read, Option<u64>) -> io::Result<T>,
) -> io::Result<T> {
    let initial = Url::parse(url).map_err(io::Error::other)?;
    let mut current = initial.clone();
    let agent = request_config().new_agent();
    for hop in 0..=5 {
        let mut response = agent
            .get(current.as_str())
            .header(
                "Accept",
                if redirects {
                    "application/octet-stream"
                } else {
                    "application/vnd.github+json"
                },
            )
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(io::Error::other)?;
        match response.status().as_u16() {
            200 => {
                let length = response.body().content_length();
                if length.is_some_and(|length| length > limit) {
                    return Err(io::Error::other("Download exceeds the size limit"));
                }
                return consume(&mut response.body_mut().as_reader(), length);
            }
            301 | 302 | 303 | 307 | 308 if redirects && hop < 5 => {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|header| header.to_str().ok())
                    .ok_or_else(|| io::Error::other("Asset redirect has no location"))?;
                if location.len() > 8192 {
                    return Err(io::Error::other("Asset redirect is too long"));
                }
                let next = current.join(location).map_err(io::Error::other)?;
                if !safe_asset_url(&next)
                    || (next.host_str() == Some("github.com") && next != initial)
                {
                    return Err(io::Error::other(
                        "Asset redirect left the official GitHub download hosts",
                    ));
                }
                current = next;
            }
            status => {
                return Err(io::Error::other(format!(
                    "GitHub download returned HTTP {status}"
                )));
            }
        }
    }
    Err(io::Error::other("Too many asset redirects"))
}

fn download_bytes(url: &str, maximum: u64, redirects: bool) -> io::Result<Vec<u8>> {
    download(url, maximum, redirects, |reader, _| {
        let mut bytes = Vec::new();
        copy_bounded(reader, &mut bytes, maximum, |_| {})?;
        Ok(bytes)
    })
}

fn copy_bounded(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    maximum: u64,
    mut progress: impl FnMut(u64),
) -> io::Result<u64> {
    let mut total = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        total = total
            .checked_add(count as u64)
            .filter(|total| *total <= maximum)
            .ok_or_else(|| io::Error::other("Download exceeds the size limit"))?;
        writer.write_all(&buffer[..count])?;
        progress(total);
    }
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    hash_reader(&mut file)
}

fn hash_reader(reader: &mut dyn Read) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(format!("{:x}", hash.finalize()));
        }
        hash.update(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests;
