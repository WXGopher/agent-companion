//! One owned actor per credential store. Requests share a native process but
//! have separate IDs, deadlines and publications. No existing daemon is used.
use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

use agent_companion_core::usage_service::{AccountIdentity, QueryKind, Request, Source};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    sync::mpsc as async_mpsc,
};

use super::{Child, MAX_RESPONSE_BYTES, decode, identity, is_executable};

pub(super) enum Event {
    Identity {
        worker_id: u64,
        source: Source,
        identity: Result<AccountIdentity, String>,
    },
    Completed {
        worker_id: u64,
        request: Request,
        result: Result<Value, String>,
        completed_at: u64,
        elapsed_ms: u64,
    },
}

impl Event {
    pub(super) fn home(&self) -> &Path {
        match self {
            Self::Identity { source, .. } => &source.codex_home,
            Self::Completed { request, .. } => &request.source.codex_home,
        }
    }
    pub(super) fn worker_id(&self) -> u64 {
        match self {
            Self::Identity { worker_id, .. } | Self::Completed { worker_id, .. } => *worker_id,
        }
    }
}

enum Message {
    Query(Box<Request>),
    Cancel(u64),
    Stop,
    Probe,
}

pub(super) struct Worker {
    pub(super) id: u64,
    source: Source,
    sender: async_mpsc::UnboundedSender<Message>,
    thread: std::thread::JoinHandle<()>,
}

impl Worker {
    pub(super) fn spawn(
        id: u64,
        source: Source,
        sender: mpsc::Sender<Event>,
        timeout: Duration,
    ) -> io::Result<Self> {
        let (tx, rx) = async_mpsc::unbounded_channel();
        let actor_source = source.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let thread = std::thread::Builder::new()
            .name("companion-usage".into())
            .spawn(move || {
                runtime.block_on(run(id, actor_source, sender, rx, timeout));
            })?;
        Ok(Self {
            id,
            source,
            sender: tx,
            thread,
        })
    }
    pub(super) fn accepts(&self, source: &Source) -> bool {
        self.source.codex_home == source.codex_home
            && self.source.executable_path == source.executable_path
            && self.source.database_path == source.database_path
            && (self.source.instance_id == "codex") == (source.instance_id == "codex")
    }
    pub(super) fn query(&self, request: Request) -> Result<(), ()> {
        self.sender
            .send(Message::Query(Box::new(request)))
            .map_err(|_| ())
    }
    pub(super) fn cancel(&self, id: u64) {
        let _ = self.sender.send(Message::Cancel(id));
    }
    pub(super) fn stop(&self) {
        let _ = self.sender.send(Message::Stop);
    }
    pub(super) fn probe(&self) {
        let _ = self.sender.send(Message::Probe);
    }
    pub(super) fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
    pub(super) fn join(self) {
        let _ = self.thread.join();
    }
}

struct Pending {
    request: Request,
    started: Instant,
    deadline: tokio::time::Instant,
    sent: bool,
}

struct Native {
    child: Child,
    input: tokio::process::ChildStdin,
    output: Lines<BufReader<tokio::io::Take<tokio::process::ChildStdout>>>,
    initialized: bool,
}

impl Native {
    async fn spawn(source: &Source, deadline: tokio::time::Instant) -> Result<Self, String> {
        use tokio::io::AsyncReadExt;
        let executable = source
            .executable_path
            .as_ref()
            .filter(|path| is_executable(path))
            .ok_or_else(|| {
                if source.instance_id == "codex" {
                    "Install Codex CLI or Codex.app, sign in, then refresh."
                } else {
                    "The Dodex runtime could not be located. Validate its deployment in Settings."
                }
                .to_owned()
            })?;
        let mut child = Child::spawn(source, executable).map_err(|_| failure(false))?;
        let input = child.process.stdin.take().unwrap();
        let output = BufReader::new(
            child
                .process
                .stdout
                .take()
                .unwrap()
                .take(MAX_RESPONSE_BYTES as u64 + 1),
        )
        .lines();
        let mut native = Self {
            child,
            input,
            output,
            initialized: false,
        };
        if native.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"agent_companion_usage","version":"2"},"capabilities":{"experimentalApi":true}}}), deadline).await.is_err() {
            native.child.close().await;
            return Err(failure(false));
        }
        Ok(native)
    }
    async fn send(&mut self, message: Value, deadline: tokio::time::Instant) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        tokio::time::timeout_at(deadline, self.input.write_all(&bytes)).await??;
        Ok(())
    }
}

fn failure(timeout: bool) -> String {
    if timeout { "Codex did not respond within 20 seconds. Check your connection and refresh." }
    else { "Could not read subscription usage. Check Codex CLI, your subscription login and connection, then refresh." }.into()
}

fn complete(
    sender: &mpsc::Sender<Event>,
    worker_id: u64,
    pending: Pending,
    result: Result<Value, String>,
) {
    let _ = sender.send(Event::Completed {
        worker_id,
        request: pending.request,
        result,
        completed_at: agent_companion_core::now_unix_secs(),
        elapsed_ms: pending.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    });
}

async fn close(native: &mut Option<Native>) {
    if let Some(mut native) = native.take() {
        native.child.close().await;
    }
}

async fn run(
    worker_id: u64,
    source: Source,
    sender: mpsc::Sender<Event>,
    mut messages: async_mpsc::UnboundedReceiver<Message>,
    timeout: Duration,
) {
    let mut native: Option<Native> = None;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut observed: Option<Result<AccountIdentity, String>> = None;
    let mut clients = BTreeMap::from([(source.instance_id.clone(), source.clone())]);
    let mut probe = tokio::time::interval(Duration::from_secs(1));
    probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut received = 0;
    loop {
        let deadline = pending
            .values()
            .map(|p| p.deadline)
            .min()
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400));
        enum Input {
            Message(Option<Message>),
            Reply(io::Result<Option<String>>),
            Deadline,
            Probe,
        }
        let input = tokio::select! {
            biased;
            message = messages.recv() => Input::Message(message),
            _ = tokio::time::sleep_until(deadline), if !pending.is_empty() => Input::Deadline,
            reply = async { native.as_mut().unwrap().output.next_line().await }, if native.is_some() => Input::Reply(reply),
            _ = probe.tick() => Input::Probe,
        };
        match input {
            Input::Message(Some(Message::Stop) | None) => break,
            Input::Message(Some(Message::Cancel(id))) => {
                pending.remove(&id);
            }
            Input::Message(Some(Message::Query(request))) => {
                let request = *request;
                clients.insert(request.source.instance_id.clone(), request.source.clone());
                let identity = identity::read(&request.source);
                if request.identity.as_ref() != identity.as_ref().ok() || identity.is_err() {
                    close(&mut native).await;
                    pending.clear();
                    for source in clients.values() {
                        let _ = sender.send(Event::Identity {
                            worker_id,
                            source: source.clone(),
                            identity: identity.clone(),
                        });
                    }
                    observed = Some(identity);
                    continue;
                }
                if observed.as_ref().is_some_and(|old| old != &identity) {
                    close(&mut native).await;
                    pending.clear();
                }
                observed = Some(identity);
                pending.insert(
                    request.id,
                    Pending {
                        request,
                        started: Instant::now(),
                        deadline: tokio::time::Instant::now() + timeout,
                        sent: false,
                    },
                );
            }
            Input::Probe | Input::Message(Some(Message::Probe)) => {
                let force = matches!(input, Input::Message(Some(Message::Probe)));
                let identity = identity::read(&source);
                if observed.as_ref() != Some(&identity) {
                    observed = Some(identity.clone());
                    close(&mut native).await;
                    pending.clear();
                } else if !force {
                    continue;
                }
                for source in clients.values() {
                    let _ = sender.send(Event::Identity {
                        worker_id,
                        source: source.clone(),
                        identity: identity.clone(),
                    });
                }
            }
            Input::Deadline => {
                let now = tokio::time::Instant::now();
                let due: Vec<_> = pending
                    .iter()
                    .filter_map(|(id, p)| (p.deadline <= now).then_some(*id))
                    .collect();
                for id in due {
                    if let Some(p) = pending.remove(&id) {
                        publish(&sender, worker_id, p, Err(failure(true)));
                    }
                }
            }
            Input::Reply(reply) => {
                let reply = reply.and_then(|line| {
                    let line = line.ok_or_else(|| io::Error::other("reader closed"))?;
                    received += line.len() + 1;
                    if received > MAX_RESPONSE_BYTES {
                        return Err(io::Error::other("response too large"));
                    }
                    if line.trim().is_empty() {
                        return Ok(Value::Null);
                    }
                    serde_json::from_str::<Value>(&line).map_err(io::Error::other)
                });
                match reply {
                    Ok(reply) if reply["id"] == 1 => {
                        if reply.get("error").is_some() || reply.get("result").is_none() {
                            for (_, p) in std::mem::take(&mut pending) {
                                publish(
                                    &sender,
                                    worker_id,
                                    p,
                                    Err("Update Codex CLI to read subscription usage.".into()),
                                );
                            }
                            close(&mut native).await;
                        } else if let Some(native) = &mut native {
                            if native
                                .send(json!({"method":"initialized"}), deadline)
                                .await
                                .is_ok() && native.send(json!({"id":2,"method":"config/read","params":{"includeLayers":false}}), deadline).await.is_ok()
                            {
                            } else {
                                for (_, p) in std::mem::take(&mut pending) {
                                    publish(&sender, worker_id, p, Err(failure(false)));
                                }
                            }
                        }
                    }
                    Ok(reply) if reply["id"] == 2 => {
                        if reply.get("error").is_none()
                            && identity::configuration_matches(&source, &reply["result"]["config"])
                        {
                            if let Some(native) = &mut native {
                                native.initialized = true;
                            }
                        } else {
                            pending.clear();
                            close(&mut native).await;
                            for source in clients.values() {
                                let _ = sender.send(Event::Identity { worker_id, source: source.clone(), identity: Err("The native authentication configuration could not be verified. Check the instance settings and refresh.".into()) });
                            }
                        }
                    }
                    Ok(reply) => {
                        if let Some(id) = reply["id"].as_u64().and_then(|id| id.checked_sub(2))
                            && let Some(p) = pending.remove(&id)
                        {
                            let result = decode(&reply, p.request.kind);
                            publish(&sender, worker_id, p, result);
                        }
                    }
                    Err(_) => {
                        for (_, p) in std::mem::take(&mut pending) {
                            publish(&sender, worker_id, p, Err(failure(false)));
                        }
                        close(&mut native).await;
                    }
                }
            }
        }
        if pending.is_empty() {
            close(&mut native).await;
            continue;
        }
        let deadline = pending.values().map(|p| p.deadline).min().unwrap();
        if native.is_none() {
            match Native::spawn(&source, deadline).await {
                Ok(child) => {
                    native = Some(child);
                    received = 0;
                }
                Err(error) => {
                    for (_, p) in std::mem::take(&mut pending) {
                        publish(&sender, worker_id, p, Err(error.clone()));
                    }
                    continue;
                }
            }
        }
        if native.as_ref().is_some_and(|native| native.initialized)
            && pending.values().any(|p| !p.sent)
        {
            // Initialization/config validation can take time. Verify again at
            // the actual business-RPC boundary, not only when it was queued.
            let identity = identity::read(&source);
            if pending
                .values()
                .any(|p| p.request.identity.as_ref() != identity.as_ref().ok())
            {
                observed = Some(identity.clone());
                close(&mut native).await;
                pending.clear();
                for source in clients.values() {
                    let _ = sender.send(Event::Identity {
                        worker_id,
                        source: source.clone(),
                        identity: identity.clone(),
                    });
                }
                continue;
            }
        }
        if let Some(connection) = native.as_mut().filter(|native| native.initialized) {
            let mut failed = false;
            for p in pending.values_mut().filter(|p| !p.sent) {
                let method = match p.request.kind {
                    QueryKind::Limits => "account/rateLimits/read",
                    QueryKind::History => "account/usage/read",
                };
                if connection
                    .send(json!({"id":p.request.id + 2,"method":method}), p.deadline)
                    .await
                    .is_err()
                {
                    failed = true;
                    break;
                }
                p.sent = true;
            }
            if failed {
                for (_, p) in std::mem::take(&mut pending) {
                    publish(&sender, worker_id, p, Err(failure(false)));
                }
                close(&mut native).await;
            }
        }
    }
    close(&mut native).await;
}

fn publish(
    sender: &mpsc::Sender<Event>,
    worker_id: u64,
    pending: Pending,
    result: Result<Value, String>,
) {
    // This runs off the UI thread even for keyring-backed identities.
    let identity = identity::read(&pending.request.source);
    if identity.as_ref().ok() != pending.request.identity.as_ref() {
        let _ = sender.send(Event::Identity {
            worker_id,
            source: pending.request.source,
            identity,
        });
        return;
    }
    let result = match result {
        Ok(value) if !identity::response_matches(&value, identity.as_ref().unwrap()) => Err("Codex returned usage for a different workspace. Refresh after checking the active account.".into()),
        other => other,
    };
    complete(sender, worker_id, pending, result);
}
