//! On-demand, read-only Codex account usage. Account data stays in memory.
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use super::ui;
use crate::usage_cache::{left_tier, reset_label, window_label};

const CACHE_TTL: Duration = Duration::from_secs(300);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub lifetime_tokens: Option<i64>,
    pub peak_daily_tokens: Option<i64>,
    pub longest_running_turn_sec: Option<i64>,
    pub current_streak_days: Option<i64>,
    pub longest_streak_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub start_date: String,
    pub tokens: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub summary: Summary,
    pub daily_usage_buckets: Option<Vec<Day>>,
}

impl Tokens {
    pub fn recent_days(&self) -> Vec<&Day> {
        let mut days: Vec<_> = self
            .daily_usage_buckets
            .iter()
            .flatten()
            .filter(|day| day.tokens >= 0)
            .collect();
        days.sort_by(|left, right| right.start_date.cmp(&left.start_date));
        days.truncate(7);
        days
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    used_percent: f64,
    window_duration_mins: Option<u64>,
    resets_at: Option<u64>,
}

impl Window {
    fn remaining(&self, now: u64) -> Option<i64> {
        if self.resets_at.is_some_and(|at| at <= now) || !self.used_percent.is_finite() {
            return None;
        }
        Some(100 - self.used_percent.clamp(0.0, 100.0).round() as i64)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<Window>,
    secondary: Option<Window>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    rate_limits: Bucket,
    rate_limits_by_limit_id: Option<BTreeMap<String, Bucket>>,
}

impl Limits {
    pub fn weekly(&self, now: u64) -> Option<(i64, Option<u64>)> {
        let bucket = self
            .rate_limits_by_limit_id
            .as_ref()
            .and_then(|b| b.get("codex"))
            .or_else(|| {
                self.rate_limits
                    .limit_id
                    .as_deref()
                    .is_none_or(|id| id == "codex")
                    .then_some(&self.rate_limits)
            })?;
        let window = [&bucket.secondary, &bucket.primary]
            .into_iter()
            .flatten()
            .find(|w| w.window_duration_mins == Some(10_080))
            .or_else(|| {
                bucket
                    .secondary
                    .as_ref()
                    .filter(|w| w.window_duration_mins.is_none())
            })?;
        Some((window.remaining(now)?, window.resets_at))
    }

    pub fn rows(&self, now: u64, offset: i64, good: i64, warn: i64) -> Vec<ui::UsageRow> {
        let mut buckets: Vec<_> = self
            .rate_limits_by_limit_id
            .as_ref()
            .map(|buckets| {
                buckets
                    .iter()
                    .map(|(id, bucket)| (id.as_str(), bucket))
                    .collect()
            })
            .unwrap_or_default();
        if buckets.is_empty() {
            buckets.push((
                self.rate_limits.limit_id.as_deref().unwrap_or("codex"),
                &self.rate_limits,
            ));
        }
        buckets.sort_by_key(|(id, _)| (*id != "codex", *id));
        let mut rows = Vec::new();
        for (id, bucket) in buckets {
            let mut title = bucket
                .limit_name
                .as_deref()
                .unwrap_or(if id == "codex" { "Codex" } else { id })
                .to_owned();
            if let Some(plan) = &bucket.plan_type
                && plan != "unknown"
            {
                title.push_str(&format!(" · {}", plan.replace('_', " ")));
            }
            rows.push(ui::UsageRow {
                heading: true,
                agent: "codex".into(),
                label: title.into(),
                ..Default::default()
            });
            for (window, fallback) in [
                (&bucket.primary, "Primary"),
                (&bucket.secondary, "Secondary"),
            ] {
                let Some(window) = window else { continue };
                let left = window.remaining(now);
                rows.push(ui::UsageRow {
                    label: if window.window_duration_mins.unwrap_or(0) == 0 {
                        fallback.to_owned()
                    } else {
                        window_label(window.window_duration_mins)
                    }
                    .into(),
                    value: left
                        .map(|left| format!("{left}%"))
                        .unwrap_or_else(|| "—".into())
                        .into(),
                    tier: left
                        .map(|left| left_tier(left, good, warn))
                        .unwrap_or("")
                        .into(),
                    fill: left.unwrap_or(0) as f32 / 100.0,
                    resets: if left.is_none() {
                        "Reset passed · refresh for a new reading".into()
                    } else {
                        reset_label(window.resets_at, now, offset)
                            .map(|label| format!("Resets {label}"))
                            .unwrap_or_default()
                            .into()
                    },
                    ..Default::default()
                });
            }
            if bucket.primary.is_none() && bucket.secondary.is_none() {
                rows.push(ui::UsageRow {
                    label: "No allowance window reported".into(),
                    ..Default::default()
                });
            }
        }
        rows
    }
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub tokens: Option<Tokens>,
    pub limits: Option<Limits>,
    pub token_error: String,
    pub limits_error: String,
    pub error: String,
    pub read_at: Option<u64>,
}

impl Snapshot {
    fn failure(message: &str) -> Self {
        Self {
            error: message.into(),
            ..Self::default()
        }
    }
}

pub fn number(value: Option<i64>) -> String {
    let Some(value) = value.filter(|value| *value >= 0) else {
        return "—".into();
    };
    for (divisor, suffix) in [
        (1_000_000_000_000.0, "T"),
        (1_000_000_000.0, "B"),
        (1_000_000.0, "M"),
        (1_000.0, "K"),
    ] {
        if value as f64 >= divisor {
            let number = format!("{:.1}", value as f64 / divisor);
            return format!("{}{suffix}", number.strip_suffix(".0").unwrap_or(&number));
        }
    }
    value.to_string()
}

pub fn duration(value: Option<i64>) -> String {
    match value.filter(|value| *value >= 0) {
        Some(seconds) if seconds >= 3600 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        Some(seconds) if seconds >= 60 => format!("{}m {}s", seconds / 60, seconds % 60),
        Some(seconds) => format!("{seconds}s"),
        None => "—".into(),
    }
}

pub fn day_count(value: Option<i64>) -> String {
    match value.filter(|value| *value >= 0) {
        Some(1) => "1 day".into(),
        Some(days) => format!("{days} days"),
        None => "—".into(),
    }
}

#[derive(Default)]
pub struct Monitor {
    pub snapshot: Snapshot,
    completed_at: Option<Instant>,
    source_home: Option<PathBuf>,
    executable: Option<PathBuf>,
    receiver: Option<mpsc::Receiver<Snapshot>>,
    cancel: Option<oneshot::Sender<()>>,
}

impl Monitor {
    pub fn weekly(&self, now: u64) -> Option<(i64, Option<u64>)> {
        self.completed_at.filter(|at| at.elapsed() < CACHE_TTL)?;
        self.snapshot.limits.as_ref()?.weekly(now)
    }

    pub fn set_executable(&mut self, executable: Option<PathBuf>) {
        if self.executable != executable {
            self.stop();
            self.snapshot = Snapshot::default();
            self.completed_at = None;
            self.executable = executable;
        }
    }
    pub fn loading(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn refresh(&mut self, home: PathBuf, force: bool) {
        if self.source_home.as_ref() != Some(&home) {
            self.stop();
            self.snapshot = Snapshot::default();
            self.completed_at = None;
            self.source_home = Some(home.clone());
        }
        if self.loading()
            || (!force && self.completed_at.is_some_and(|at| at.elapsed() < CACHE_TTL))
        {
            return;
        }
        let executable = self.executable.clone();
        let (tx, rx) = mpsc::channel();
        let (cancel, cancelled) = oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("codex-subscription-usage".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let snapshot = match result {
                    Ok(runtime) => runtime.block_on(async {
                        tokio::select! {
                            biased;
                            _ = cancelled => None,
                            result = read_with_executable(home, executable) => Some(result),
                        }
                    }),
                    Err(_) => Some(Snapshot::failure(
                        "Could not start the usage reader. Please refresh.",
                    )),
                };
                if let Some(snapshot) = snapshot {
                    let _ = tx.send(snapshot);
                }
            });
        if spawned.is_ok() {
            self.receiver = Some(rx);
            self.cancel = Some(cancel);
        } else {
            self.snapshot = Snapshot::failure("Could not start the usage reader. Please refresh.");
            self.completed_at = Some(Instant::now());
        }
    }

    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(snapshot) => snapshot,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                Snapshot::failure("The usage reader stopped. Please refresh.")
            }
        };
        // Replace on failure, too: an old login must not survive a failed read.
        self.snapshot = result;
        self.completed_at = Some(Instant::now());
        self.receiver = None;
        self.cancel = None;
        true
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.receiver = None;
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
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
            let mut bounded = (&mut self.output).take(remaining as u64);
            let count =
                tokio::time::timeout_at(self.deadline, bounded.read_until(b'\n', &mut line))
                    .await??;
            self.received += count;
            if self.received > MAX_RESPONSE_BYTES {
                return Err(io::Error::other("usage response too large"));
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "usage reader closed",
                ));
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let reply: Value = serde_json::from_slice(&line)?;
            if let Some(reply_id) = reply["id"].as_u64().filter(|id| (1..=4).contains(id)) {
                self.replies.insert(reply_id, reply);
            }
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(reply: &Value, section: &str) -> Result<T, String> {
    if reply.get("error").is_none()
        && let Some(result) = reply.get("result")
        && let Ok(value) = serde_json::from_value(result.clone())
    {
        return Ok(value);
    }
    if matches!(reply["error"]["code"].as_i64(), Some(-32601 | -32600)) {
        Err(format!(
            "Update Codex CLI to read {section}. This version does not support account usage."
        ))
    } else {
        Err(format!(
            "Could not read {section}. Check your subscription login and connection, then refresh."
        ))
    }
}

async fn exchange<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
) -> io::Result<Snapshot> {
    rpc.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"agent_companion_usage","version":"1"},"capabilities":{"experimentalApi":true}}})).await?;
    let reply = rpc.receive(1).await?;
    if reply.get("error").is_some() || reply.get("result").is_none() {
        return Ok(Snapshot::failure(
            "Update Codex CLI to read subscription usage.",
        ));
    }
    rpc.send(json!({"method":"initialized"})).await?;
    rpc.send(json!({"id":2,"method":"account/read","params":{"refreshToken":false}}))
        .await?;
    let reply = rpc.receive(2).await?;
    let account: Value = match decode(&reply, "the Codex login") {
        Ok(account) => account,
        Err(error) => return Ok(Snapshot::failure(&error)),
    };
    if account["account"]["type"].as_str() != Some("chatgpt") {
        return Ok(Snapshot::failure(
            "Sign in to Codex with your ChatGPT subscription, then refresh. Subscription usage does not use an API key.",
        ));
    }
    rpc.send(json!({"id":3,"method":"account/rateLimits/read"}))
        .await?;
    rpc.send(json!({"id":4,"method":"account/usage/read"}))
        .await?;
    let mut snapshot = Snapshot {
        read_at: Some(agent_companion_core::now_unix_secs()),
        ..Default::default()
    };
    let incomplete = "The usage read did not finish. Check your connection and refresh.";
    match rpc.receive(3).await {
        Ok(reply) => match decode(&reply, "subscription limits") {
            Ok(limits) => snapshot.limits = Some(limits),
            Err(error) => snapshot.limits_error = error,
        },
        Err(_) => snapshot.limits_error = incomplete.into(),
    }
    match rpc.receive(4).await {
        Ok(reply) => match decode(&reply, "token activity") {
            Ok(tokens) => snapshot.tokens = Some(tokens),
            Err(error) => snapshot.token_error = error,
        },
        Err(_) => snapshot.token_error = incomplete.into(),
    }
    Ok(snapshot)
}

#[cfg(test)]
async fn read(home: PathBuf) -> Snapshot {
    read_with_executable(home, None).await
}

async fn read_with_executable(home: PathBuf, executable: Option<PathBuf>) -> Snapshot {
    let Ok(executable) = crate::codex::executable(executable.as_ref()) else {
        return Snapshot::failure(
            "Install Codex CLI, then sign in with your ChatGPT subscription and refresh.",
        );
    };
    let result: io::Result<Snapshot> = async {
        let job = crate::codex::job::Job::new()?;
        let database = agent_companion_core::dashboard::database_home(&home);
        let command = crate::windows_deployment::isolated_command(&executable, &home, &database);
        let mut child = tokio::process::Command::from(command)
            .args([
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "analytics.enabled=false",
            ])
            .current_dir(
                crate::util::home_dir()
                    .ok_or_else(|| io::Error::other("home directory unavailable"))?,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .kill_on_drop(true)
            .spawn()?;
        job.assign(
            child
                .id()
                .ok_or_else(|| io::Error::other("usage reader PID unavailable"))?,
        )?;
        let mut rpc = Rpc::new(
            child.stdout.take().unwrap(),
            child.stdin.take().unwrap(),
            READ_TIMEOUT,
        );
        let result = exchange(&mut rpc).await;
        drop(rpc);
        let _ = child.kill().await;
        drop(job);
        result
    }
    .await;
    result.unwrap_or_else(|_| Snapshot::failure("Could not read subscription usage. Check Codex CLI, your subscription login and connection, then refresh."))
}

#[cfg(test)]
mod tests;
