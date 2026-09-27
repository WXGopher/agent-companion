//! Cross-platform account reader. UI events enter the core scheduler; worker
//! threads execute one isolated, read-only app-server request at a time.
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub use agent_companion_core::usage_service::{InstanceSnapshot, Source, UsageSettings};
use agent_companion_core::usage_service::{QueryKind, Request, Scheduler, settings_path};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::oneshot,
};

const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

struct Pending {
    request: Request,
    cancel: oneshot::Sender<()>,
    worker: std::thread::JoinHandle<()>,
}

struct Completion {
    request: Request,
    result: Result<Value, String>,
    completed_at: u64,
    elapsed_ms: u64,
}

pub struct UsageService {
    scheduler: Scheduler,
    jobs: BTreeMap<u64, Pending>,
    retired: Vec<std::thread::JoinHandle<()>>,
    sender: mpsc::Sender<Completion>,
    receiver: mpsc::Receiver<Completion>,
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

    pub fn poll(&mut self, _now: u64) -> bool {
        let mut changed = false;
        while let Ok(completion) = self.receiver.try_recv() {
            if let Some(pending) = self.jobs.remove(&completion.request.id) {
                self.retired.push(pending.worker);
            }
            changed |= self.scheduler.complete(
                &completion.request,
                completion.result,
                completion.completed_at,
                completion.elapsed_ms,
            );
        }
        self.reap_finished();
        changed
    }

    pub fn panel_open(&mut self, now: u64) {
        self.poll(now);
        let requests = self.scheduler.panel_open(now);
        self.dispatch(requests, now);
    }

    pub fn refresh(&mut self, id: &str, now: u64) {
        self.poll(now);
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
            .iter()
            .filter_map(|(id, job)| (!self.scheduler.is_current(&job.request)).then_some(*id))
            .collect();
        for id in obsolete {
            if let Some(job) = self.jobs.remove(&id) {
                let _ = job.cancel.send(());
                self.retired.push(job.worker);
            }
        }
    }

    fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                let _ = self.retired.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }

    fn dispatch(&mut self, requests: Vec<Request>, now: u64) {
        for request in requests {
            let (cancel, cancelled) = oneshot::channel();
            let pending = request.clone();
            let sender = self.sender.clone();
            let timeout = self.read_timeout;
            let spawned = std::thread::Builder::new()
                .name(format!(
                    "usage-{}-{:?}",
                    request.source.instance_id, request.kind
                ))
                .spawn(move || {
                    let started = Instant::now();
                    let result = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime.block_on(read(
                            &request.source,
                            request.kind,
                            cancelled,
                            timeout,
                        )),
                        Err(_) => Some(Err(
                            "Could not start the usage reader. Please refresh.".into()
                        )),
                    };
                    if let Some(result) = result {
                        let _ = sender.send(Completion {
                            request,
                            result,
                            completed_at: agent_companion_core::now_unix_secs(),
                            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        });
                    }
                });
            match spawned {
                Ok(worker) => {
                    self.jobs.insert(
                        pending.id,
                        Pending {
                            request: pending,
                            cancel,
                            worker,
                        },
                    );
                }
                Err(_) => {
                    self.scheduler.complete(
                        &pending,
                        Err("Could not start the usage reader. Please refresh.".into()),
                        now,
                        0,
                    );
                }
            }
        }
    }

    pub fn stop(&mut self) {
        self.scheduler.clear();
        self.cancel_obsolete();
        // Source changes remain nonblocking. App shutdown waits for its owned
        // readers to kill and reap their children before the process can exit.
        for worker in self.retired.drain(..) {
            let _ = worker.join();
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

fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    let value = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = std::env::var_os("HOME");
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn primary_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        crate::codex::executable(None).ok()
    }
    #[cfg(not(windows))]
    {
        let mut candidates: Vec<_> = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .filter(|path| path.is_absolute())
            .map(|path| path.join("codex"))
            .collect();
        if let Some(home) = user_home() {
            candidates.push(home.join(".local/bin/codex"));
        }
        candidates.extend([
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
        ]);
        if let Some(path) = candidates.into_iter().find(|path| is_executable(path)) {
            return Some(path);
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = user_home() {
            return crate::macos_primary_app::discover(
                Path::new("/Applications"),
                &home.join("Applications"),
            )
            .and_then(|app| app.executable);
        }
        None
    }
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
            let name = name.to_string_lossy().to_ascii_uppercase();
            #[cfg(windows)]
            {
                [
                    "SYSTEMROOT",
                    "WINDIR",
                    "SYSTEMDRIVE",
                    "USERPROFILE",
                    "HOMEDRIVE",
                    "HOMEPATH",
                    "LOCALAPPDATA",
                    "APPDATA",
                    "TEMP",
                    "TMP",
                    "PATH",
                    "COMSPEC",
                    "PATHEXT",
                    "USERNAME",
                    "USERDOMAIN",
                ]
                .contains(&name.as_str())
            }
            #[cfg(not(windows))]
            {
                !["CODEX_", "OPENAI_", "CHATGPT_", "ELECTRON_", "DYLD_", "LD_"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                    && !["NODE_OPTIONS", "NODE_PATH"].contains(&name.as_str())
            }
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
        .envs(isolated_environment(source, std::env::vars_os()));
    command.args([
        "app-server",
        "--listen",
        "stdio://",
        "-c",
        "analytics.enabled=false",
    ]);
    if source.instance_id != "codex" {
        command.args(["-c", "cli_auth_credentials_store=\"file\""]);
    }
    command.arg("-c").arg(format!(
        "sqlite_home={}",
        serde_json::to_string(&source.database_path.to_string_lossy()).unwrap()
    ));
    // An absent home must not fall back to the GUI's inherited working
    // directory, where a project-local configuration could change accounts.
    command.current_dir(user_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "User home directory is unavailable.",
        )
    })?);
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
    #[cfg(unix)]
    group: i32,
    #[cfg(windows)]
    _job: crate::codex::job::Job,
}

impl Child {
    fn spawn(source: &Source, executable: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        let job = crate::codex::job::Job::new()?;
        let process = tokio::process::Command::from(command(source, executable)?)
            .kill_on_drop(true)
            .spawn()?;
        let pid = process
            .id()
            .ok_or_else(|| io::Error::other("Usage reader PID unavailable."))?;
        #[cfg(windows)]
        job.assign(pid)?;
        Ok(Self {
            process,
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

async fn read(
    source: &Source,
    kind: QueryKind,
    mut cancelled: oneshot::Receiver<()>,
    timeout: Duration,
) -> Option<Result<Value, String>> {
    let started = Instant::now();
    if !matches!(
        cancelled.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ) {
        return None;
    }
    let Some(executable) = source
        .executable_path
        .as_ref()
        .filter(|path| is_executable(path))
    else {
        return Some(Err(if source.instance_id == "codex" {
            "Install Codex CLI or Codex.app, then sign in with your ChatGPT subscription and refresh."
        } else { "The Dodex runtime could not be located. Validate its deployment in Settings." }.into()));
    };
    let mut child = match Child::spawn(source, executable) {
        Ok(child) => child,
        Err(_) => {
            return Some(Err(
                "Could not start Codex to read usage. Check the runtime and refresh.".into(),
            ));
        }
    };
    let mut rpc = Rpc::new(
        child.process.stdout.take().unwrap(),
        child.process.stdin.take().unwrap(),
        timeout.saturating_sub(started.elapsed()),
    );
    let result = tokio::select! {
        biased;
        _ = &mut cancelled => None,
        result = exchange(&mut rpc, kind) => Some(result.unwrap_or_else(|error| Err(if error.kind() == io::ErrorKind::TimedOut {
            "Codex did not respond within 20 seconds. Check your connection and refresh."
        } else {
            "Could not read subscription usage. Check Codex CLI, your subscription login and connection, then refresh."
        }.into()))),
    };
    drop(rpc);
    child.close().await;
    result
}

struct Rpc<R, W> {
    input: W,
    output: BufReader<R>,
    received: usize,
    replies: HashMap<u64, Value>,
    deadline: tokio::time::Instant,
}

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
            if let Some(id) = reply["id"].as_u64().filter(|id| (1..=2).contains(id)) {
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
    let method = match kind {
        QueryKind::Limits => "account/rateLimits/read",
        QueryKind::History => "account/usage/read",
    };
    rpc.send(json!({"id":2,"method":method})).await?;
    Ok(decode(&rpc.receive(2).await?, kind))
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
