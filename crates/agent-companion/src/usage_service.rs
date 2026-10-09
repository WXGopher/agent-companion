//! Cross-platform account reader. UI events enter the core scheduler; worker
//! threads verify account ownership and multiplex isolated app-server reads.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub use agent_companion_core::usage_service::{InstanceSnapshot, Source, UsageSettings};
use agent_companion_core::usage_service::{QueryKind, Request, Scheduler, settings_path};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(test)]
use std::{collections::HashMap, time::Instant};
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[path = "usage_service/identity.rs"]
mod identity;
#[path = "usage_service/worker.rs"]
mod worker;

use worker::{Event, Worker};

pub struct UsageService {
    scheduler: Scheduler,
    jobs: BTreeMap<u64, Request>,
    workers: BTreeMap<PathBuf, Worker>,
    retired: Vec<Worker>,
    next_worker_id: u64,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
    settings_path: Option<PathBuf>,
    settings_checked_at: Option<u64>,
    settings_stamp: Option<std::time::SystemTime>,
    read_timeout: Duration,
}

impl UsageService {
    pub fn new() -> Self {
        Self::create(settings_path().ok())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_settings_path(path: PathBuf) -> Self {
        Self::create(Some(path))
    }

    fn create(path: Option<PathBuf>) -> Self {
        let settings = path.as_deref().map(UsageSettings::load).unwrap_or_default();
        let settings_stamp = path.as_deref().and_then(settings_stamp);
        let (sender, receiver) = mpsc::channel();
        Self {
            scheduler: Scheduler::new(settings.refresh_interval_minutes),
            jobs: BTreeMap::new(),
            retired: Vec::new(),
            workers: BTreeMap::new(),
            next_worker_id: 0,
            sender,
            receiver,
            settings_path: path,
            settings_checked_at: None,
            settings_stamp,
            read_timeout: READ_TIMEOUT,
        }
    }

    pub fn sync_sources(&mut self, sources: Vec<Source>, now: u64) {
        let sources = sources.into_iter().map(resolve_source).collect();
        let requests = self.scheduler.sync_sources(sources, now);
        self.cancel_obsolete();
        self.dispatch(requests, now);
    }

    /// Timer entry point. Reading snapshots and polling completions never
    /// trigger an account request or a history cache check.
    pub fn tick(&mut self, now: u64) -> bool {
        let mut changed = self.poll(now);
        if self.settings_checked_at != Some(now) {
            self.settings_checked_at = Some(now);
            if let Some(path) = &self.settings_path {
                let settings = UsageSettings::load(path);
                let stamp = settings_stamp(path);
                if stamp != self.settings_stamp
                    || settings.refresh_interval_minutes != self.interval_minutes()
                {
                    let _ = self
                        .scheduler
                        .set_interval(settings.refresh_interval_minutes, now);
                    self.settings_stamp = stamp;
                    changed = true;
                }
            }
        }
        let requests = self.scheduler.tick(now);
        changed |= !requests.is_empty();
        self.dispatch(requests, now);
        changed
    }

    pub fn poll(&mut self, now: u64) -> bool {
        let mut changed = false;
        while let Ok(event) = self.receiver.try_recv() {
            let current = self
                .workers
                .get(event.home())
                .is_some_and(|worker| worker.id == event.worker_id());
            if !current {
                continue;
            }
            match event {
                Event::Identity {
                    source, identity, ..
                } => {
                    if !self
                        .scheduler
                        .snapshot(&source.instance_id)
                        .is_some_and(|snapshot| snapshot.source == source)
                    {
                        continue;
                    }
                    let history_pending = self.jobs.values().any(|request| {
                        request.source == source && request.kind == QueryKind::History
                    });
                    let before = self.scheduler.snapshot(&source.instance_id);
                    let mut requests =
                        self.scheduler
                            .sync_identity(&source.instance_id, identity, now);
                    self.cancel_obsolete();
                    if history_pending {
                        requests.extend(self.scheduler.load_history(&source.instance_id, now));
                    }
                    changed |= before != self.scheduler.snapshot(&source.instance_id);
                    self.dispatch(requests, now);
                }
                Event::Completed {
                    request,
                    result,
                    completed_at,
                    elapsed_ms,
                    ..
                } => {
                    self.jobs.remove(&request.id);
                    if self.scheduler.is_current(&request)
                        && let Some(expected) = &request.identity
                        && let Some(identity) =
                            identity::local_publication_check(&request.source, expected)
                        && identity.as_ref().ok() != Some(expected)
                    {
                        let requests = self.scheduler.sync_identity(
                            &request.source.instance_id,
                            identity,
                            now,
                        );
                        self.cancel_obsolete();
                        self.dispatch(requests, now);
                        changed = true;
                        continue;
                    }
                    changed |= self
                        .scheduler
                        .complete(&request, result, completed_at, elapsed_ms);
                }
            }
        }
        self.reap_finished();
        changed
    }

    pub fn panel_open(&mut self, now: u64) {
        self.poll(now);
        for worker in self.workers.values() {
            worker.probe();
        }
        let requests = self.scheduler.panel_open(now);
        self.dispatch(requests, now);
    }

    pub fn refresh(&mut self, id: &str, now: u64) {
        self.poll(now);
        if let Some(source) = self.scheduler.snapshot(id).map(|snapshot| snapshot.source)
            && let Some(worker) = self.workers.get(&source.codex_home)
        {
            worker.probe();
        }
        let requests = self.scheduler.refresh(id, now);
        self.dispatch(requests, now);
    }

    pub fn load_history(&mut self, id: &str, now: u64) {
        self.poll(now);
        let requests = self.scheduler.load_history(id, now);
        self.dispatch(requests, now);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn snapshot(&self, id: &str) -> Option<InstanceSnapshot> {
        self.scheduler.snapshot(id)
    }
    #[cfg_attr(windows, allow(dead_code))]
    pub fn snapshots(&self) -> Vec<InstanceSnapshot> {
        self.scheduler.snapshots()
    }
    pub fn interval_minutes(&self) -> u8 {
        self.scheduler.interval_minutes()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn set_interval(&mut self, minutes: u8, now: u64) -> io::Result<()> {
        let path = self.settings_path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Application configuration directory is unavailable.",
            )
        })?;
        UsageSettings {
            refresh_interval_minutes: minutes,
        }
        .save(path)?;
        self.scheduler.set_interval(minutes, now)?;
        self.settings_stamp = settings_stamp(path);
        self.settings_checked_at = Some(now);
        Ok(())
    }

    fn cancel_obsolete(&mut self) {
        let obsolete: Vec<_> = self
            .jobs
            .values()
            .filter(|request| !self.scheduler.is_current(request))
            .cloned()
            .collect();
        for request in obsolete {
            self.jobs.remove(&request.id);
            if let Some(worker) = self.workers.get(&request.source.codex_home) {
                worker.cancel(request.id);
            }
        }
        let snapshots = self.scheduler.snapshots();
        let unused: Vec<_> = self
            .workers
            .iter()
            .filter_map(|(home, worker)| {
                (!snapshots
                    .iter()
                    .any(|snapshot| worker.accepts(&snapshot.source)))
                .then_some(home.clone())
            })
            .collect();
        for home in unused {
            if let Some(worker) = self.workers.remove(&home) {
                worker.stop();
                self.retired.push(worker);
            }
        }
    }

    fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                self.retired.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }

    fn dispatch(&mut self, requests: Vec<Request>, now: u64) {
        for request in requests {
            if !self.scheduler.is_current(&request) {
                continue;
            }
            let home = request.source.codex_home.clone();
            if !self.workers.contains_key(&home) {
                self.next_worker_id += 1;
                match Worker::spawn(
                    self.next_worker_id,
                    request.source.clone(),
                    self.sender.clone(),
                    self.read_timeout,
                ) {
                    Ok(worker) => {
                        self.workers.insert(home.clone(), worker);
                    }
                    Err(_) => {
                        self.scheduler.complete(
                            &request,
                            Err("Could not start the usage reader. Please refresh.".into()),
                            now,
                            0,
                        );
                        continue;
                    }
                }
            }
            let worker = &self.workers[&home];
            if !worker.accepts(&request.source) || worker.query(request.clone()).is_err() {
                self.scheduler.complete(&request, Err("The authentication store has conflicting or unavailable usage runtimes. Validate the instance configuration and refresh.".into()), now, 0);
            } else {
                self.jobs.insert(request.id, request);
            }
        }
    }

    pub fn stop(&mut self) {
        self.scheduler.clear();
        self.cancel_obsolete();
        // Join only Companion-owned readers; never touch existing daemons.
        for worker in self.retired.drain(..) {
            worker.join();
        }
    }
}

fn settings_stamp(path: &Path) -> Option<std::time::SystemTime> {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

impl Drop for UsageService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn resolve_source(mut source: Source) -> Source {
    if source.executable_path.is_none() && source.instance_id == "codex" {
        source.executable_path = primary_executable();
    }
    if let Some(path) = &source.executable_path {
        source.executable_path = Some(path.canonicalize().unwrap_or_else(|_| path.clone()));
    }
    source.codex_home = source
        .codex_home
        .canonicalize()
        .unwrap_or(source.codex_home);
    source.database_path = source
        .database_path
        .canonicalize()
        .unwrap_or(source.database_path);
    source
}

fn primary_executable() -> Option<PathBuf> {
    crate::tui_deployment::primary_instance()?.runtime().ok()
}

#[cfg(not(windows))]
fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(all(target_os = "macos", test))]
fn primary_macos_executable(
    home: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
    _system_applications: &Path,
    _user_applications: &Path,
) -> Option<PathBuf> {
    for path in candidates.into_iter().filter(|path| is_executable(path)) {
        // Reading usage also supports existing native Homebrew/standalone
        // binaries that predate the updater's complete-package layout. Resolve
        // links without executing a shell/Node wrapper that could select another
        // account. Package migration keeps its stricter, separate validation.
        if let Ok(entry) = path.canonicalize()
            && primary_runtime_location(home, &entry)
        {
            if is_macos_native(&entry) {
                return Some(entry);
            }
            if let Some(native) = npm_native(&entry)
                && primary_runtime_location(home, &native)
            {
                return Some(native);
            }
        }
    }
    None
}

#[cfg(all(target_os = "macos", test))]
fn primary_runtime_location(home: &Path, path: &Path) -> bool {
    !path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.eq_ignore_ascii_case("Dodex.app") || name.eq_ignore_ascii_case(".Dodex")
    }) && ![
        "Library/Application Support/AgentCompanion/Tui",
        "Library/Application Support/AgentCompanion/Dodex",
        "Library/Application Support/AgentCompanion/DodexApp",
        "Library/Application Support/Codex-B",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .any(|root| {
        path.starts_with(&root) || root.canonicalize().is_ok_and(|root| path.starts_with(root))
    })
}

#[cfg(all(target_os = "macos", test))]
fn npm_native(entry: &Path) -> Option<PathBuf> {
    let package = entry.parent()?.parent()?;
    if entry != package.join("bin/codex.js") {
        return None;
    }
    let manifest = |root: &Path| -> Option<Value> {
        let path = root.join("package.json");
        if !path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 64 * 1024)
        {
            return None;
        }
        let bytes = agent_companion_core::tui_instance::read_limited(&path, 64 * 1024).ok()?;
        serde_json::from_slice(&bytes).ok()
    };
    let main = manifest(package)?;
    if main["name"] != "@openai/codex"
        || !matches!(
            main["bin"]["codex"].as_str(),
            Some("bin/codex.js" | "./bin/codex.js")
        )
    {
        return None;
    }
    let (platform, triple) = if cfg!(target_arch = "aarch64") {
        ("codex-darwin-arm64", "aarch64-apple-darwin")
    } else if cfg!(target_arch = "x86_64") {
        ("codex-darwin-x64", "x86_64-apple-darwin")
    } else {
        return None;
    };
    // Official @openai/codex bin/codex.js resolves its optional platform
    // package, then falls back to its own vendor directory. npm may nest or
    // hoist the optional package. Read only these known layouts, never run JS
    // or infer a command from arbitrary package metadata.
    for root in [
        package.join("node_modules/@openai").join(platform),
        package.parent()?.join(platform),
        package.to_path_buf(),
    ] {
        let Some(info) = manifest(&root) else {
            continue;
        };
        if info["name"] != "@openai/codex"
            && info["name"].as_str() != Some(format!("@openai/{platform}").as_str())
        {
            continue;
        }
        // 0.155/0.160 use bin/codex; older releases (e.g. 0.104) use codex/codex.
        for relative in ["bin/codex", "codex/codex"] {
            let native = root.join("vendor").join(triple).join(relative);
            if is_executable(&native) && is_macos_native(&native) {
                return native.canonicalize().ok();
            }
        }
    }
    None
}

#[cfg(all(target_os = "macos", test))]
fn is_macos_native(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xce]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn isolated_environment(
    source: &Source,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<_, _> = inherited
        .into_iter()
        .filter(|(name, _)| {
            agent_companion_core::process_environment::keep_variable(&name.to_string_lossy())
        })
        .collect();
    environment.insert(
        "CODEX_HOME".into(),
        source.codex_home.as_os_str().to_owned(),
    );
    environment.insert(
        "CODEX_SQLITE_HOME".into(),
        source.database_path.as_os_str().to_owned(),
    );
    environment
}

fn command(source: &Source, executable: &Path) -> io::Result<Command> {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .envs(isolated_environment(source, std::env::vars_os()))
        // Supported by the native app-server bootstrap: disable remote
        // control for this owned process without changing persisted settings.
        .env("CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED", "1");
    command.args([
        // This directly starts our stdio server; it never takes the separate
        // daemon/proxy subcommand path. Do not inject daemon_auto_start: older
        // bundled runtimes reject that newer feature under strict config.
        "app-server",
        "--listen",
        "stdio://",
        "--strict-config",
        "-c",
        "analytics.enabled=false",
        "-c",
        "features.remote_control=false",
    ]);
    let configuration = identity::configuration(source).map_err(io::Error::other)?;
    command.arg("-c").arg(format!(
        "cli_auth_credentials_store={}",
        serde_json::to_string(&configuration.store).unwrap()
    ));
    command.arg("-c").arg(format!(
        "chatgpt_base_url={}",
        serde_json::to_string(&configuration.service).unwrap()
    ));
    command.arg("-c").arg(format!(
        "sqlite_home={}",
        serde_json::to_string(&source.database_path.to_string_lossy()).unwrap()
    ));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Ok(command)
}

struct Child {
    process: tokio::process::Child,
    _working_directory: tempfile::TempDir,
    #[cfg(unix)]
    group: i32,
    #[cfg(windows)]
    _job: crate::codex::job::Job,
}

impl Child {
    fn spawn(source: &Source, executable: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        let job = crate::codex::job::Job::new()?;
        let working_directory = tempfile::Builder::new()
            .prefix("companion-usage-")
            .tempdir()?;
        let mut command = command(source, executable)?;
        command.current_dir(working_directory.path());
        let process = tokio::process::Command::from(command)
            .kill_on_drop(true)
            .spawn()?;
        let pid = process
            .id()
            .ok_or_else(|| io::Error::other("Usage reader PID unavailable."))?;
        #[cfg(windows)]
        job.assign(pid)?;
        Ok(Self {
            process,
            _working_directory: working_directory,
            #[cfg(unix)]
            group: pid as i32,
            #[cfg(windows)]
            _job: job,
        })
    }

    async fn close(&mut self) {
        #[cfg(unix)]
        self.kill_group();
        let _ = self.process.kill().await;
        let _ = self.process.wait().await;
        #[cfg(unix)]
        {
            self.group = 0;
        }
    }

    #[cfg(unix)]
    fn kill_group(&self) {
        // Every query starts its own process group. Never signal a preexisting
        // app-server, the editor, or another instance's process group.
        if self.group > 0 {
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        #[cfg(unix)]
        self.kill_group();
    }
}

#[cfg(test)]
struct Rpc<R, W> {
    input: W,
    output: BufReader<R>,
    received: usize,
    replies: HashMap<u64, Value>,
    deadline: tokio::time::Instant,
}

#[cfg(test)]
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Rpc<R, W> {
    fn new(output: R, input: W, timeout: Duration) -> Self {
        Self {
            input,
            output: BufReader::new(output),
            received: 0,
            replies: HashMap::new(),
            deadline: tokio::time::Instant::now() + timeout,
        }
    }

    async fn send(&mut self, message: Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        tokio::time::timeout_at(self.deadline, self.input.write_all(&bytes)).await??;
        Ok(())
    }

    async fn receive(&mut self, id: u64) -> io::Result<Value> {
        loop {
            if let Some(reply) = self.replies.remove(&id) {
                return Ok(reply);
            }
            let mut line = Vec::new();
            let remaining = MAX_RESPONSE_BYTES.saturating_sub(self.received) + 1;
            let count = tokio::time::timeout_at(
                self.deadline,
                (&mut self.output)
                    .take(remaining as u64)
                    .read_until(b'\n', &mut line),
            )
            .await??;
            self.received += count;
            if self.received > MAX_RESPONSE_BYTES {
                return Err(io::Error::other("Usage response too large."));
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Usage reader closed.",
                ));
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let reply: Value = serde_json::from_slice(&line)?;
            if let Some(id) = reply["id"].as_u64().filter(|id| (1..=3).contains(id)) {
                self.replies.insert(id, reply);
            }
        }
    }
}

fn decode(reply: &Value, kind: QueryKind) -> Result<Value, String> {
    if reply.get("error").is_none()
        && let Some(value) = reply.get("result")
    {
        return validate_result(value.clone(), kind);
    }
    if matches!(reply["error"]["code"].as_i64(), Some(-32601 | -32600)) {
        Err("Update Codex CLI to read this account usage interface.".into())
    } else {
        // Server errors may contain secrets or local paths. Use a useful,
        // bounded message without exposing backend diagnostic text.
        Err("Could not read account usage. Check your subscription login and connection, then refresh.".into())
    }
}

#[cfg(test)]
async fn exchange<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
    kind: QueryKind,
) -> io::Result<Result<Value, String>> {
    rpc.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"agent_companion_usage","version":"1"},"capabilities":{"experimentalApi":true}}})).await?;
    let initialized = rpc.receive(1).await?;
    if initialized.get("error").is_some() || initialized.get("result").is_none() {
        return Ok(Err("Update Codex CLI to read subscription usage.".into()));
    }
    rpc.send(json!({"method":"initialized"})).await?;
    rpc.send(json!({"id":2,"method":"config/read","params":{"includeLayers":false}}))
        .await?;
    let config = rpc.receive(2).await?;
    if config.get("error").is_some() || config.get("result").is_none() {
        return Ok(Err("The native configuration could not be verified.".into()));
    }
    let method = match kind {
        QueryKind::Limits => "account/rateLimits/read",
        QueryKind::History => "account/usage/read",
    };
    rpc.send(json!({"id":3,"method":method})).await?;
    Ok(decode(&rpc.receive(3).await?, kind))
}

// Validate supported payload fields before the state machine calls a response
// successful. Missing windows are a valid reading and must clear old windows.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Window {
    used_percent: f64,
    window_duration_mins: Option<u64>,
    resets_at: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Bucket {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<Window>,
    secondary: Option<Window>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Limits {
    rate_limits: Bucket,
    rate_limits_by_limit_id: Option<BTreeMap<String, Bucket>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Summary {
    lifetime_tokens: Option<i64>,
    peak_daily_tokens: Option<i64>,
    longest_running_turn_sec: Option<i64>,
    current_streak_days: Option<i64>,
    longest_streak_days: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Day {
    start_date: String,
    tokens: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Tokens {
    summary: Summary,
    daily_usage_buckets: Option<Vec<Day>>,
}

fn validate_result(mut value: Value, kind: QueryKind) -> Result<Value, String> {
    let valid = match kind {
        QueryKind::Limits => {
            // Some CLI releases represent an unavailable allowance by null.
            if value.is_object()
                && (value.get("rateLimits").is_some() || value.get("rateLimitsByLimitId").is_some())
                && value["rateLimits"].is_null()
            {
                value["rateLimits"] = json!({});
            }
            serde_json::from_value::<Limits>(value.clone()).is_ok()
        }
        QueryKind::History => serde_json::from_value::<Tokens>(value.clone()).is_ok(),
    };
    if valid {
        Ok(value)
    } else {
        Err("Codex returned an invalid usage response. Update Codex CLI and refresh.".into())
    }
}

#[cfg(test)]
#[path = "usage_service/tests.rs"]
mod tests;
