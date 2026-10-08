//! Explicit, independent TUI updates. Opening settings only observes versions.
use crate::tui_deployment;
use agent_companion_core::tui_instance::{Channel, InstanceConfig, InstanceLock, Layout};
use std::{
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Action {
    UpdateCodex,
    UpdateDodex,
}
impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "update-codex" => Some(Self::UpdateCodex),
            "update-dodex" => Some(Self::UpdateDodex),
            _ => None,
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::UpdateCodex => "codex",
            Self::UpdateDodex => "dodex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub current: String,
    pub target: String,
    pub message: String,
    pub error: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub rows: Vec<Row>,
    pub busy: bool,
    pub message: String,
    pub error: bool,
    pub initializing: bool,
    pub notice: String,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            rows: ["Codex TUI", "Dodex TUI"]
                .into_iter()
                .map(|name| Row {
                    name: name.into(),
                    current: "尚未读取".into(),
                    target: "各自安装渠道".into(),
                    message: String::new(),
                    error: false,
                })
                .collect(),
            busy: false,
            message: "两套 TUI 独立更新，允许版本不同。".into(),
            error: false,
            initializing: false,
            notice: String::new(),
        }
    }
}

#[derive(Default)]
struct Requests {
    pending: Option<Action>,
    repeated: bool,
}
static REQUEST: OnceLock<Mutex<Requests>> = OnceLock::new();
pub fn request(action: Action) {
    let mut requests = REQUEST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if requests.pending.is_some() {
        requests.repeated = true;
    } else {
        requests.pending = Some(action);
    }
}

pub(crate) fn run_action(action: Action) -> Result<Snapshot, String> {
    let layout = Layout::current().map_err(|e| e.to_string())?;
    let _lock = InstanceLock::acquire(&layout, action.id()).map_err(|e| e.to_string())?;
    let instance = selected(action).ok_or("所选 TUI 未安装，请先安装或修复。")?;
    update_instance(&instance)?;
    let mut snapshot = Snapshot::default();
    observe(&mut snapshot);
    snapshot.message = format!("{} TUI 更新完成；另一实例保持原版本。", instance.label);
    Ok(snapshot)
}
fn selected(action: Action) -> Option<InstanceConfig> {
    match action {
        Action::UpdateCodex => tui_deployment::primary_instance(),
        Action::UpdateDodex => tui_deployment::saved_instance(),
    }
}

fn update_instance(instance: &InstanceConfig) -> Result<(), String> {
    let mut command = if instance.channel == Channel::Homebrew {
        let mut command = Command::new(
            instance
                .updater
                .as_ref()
                .ok_or("Homebrew 更新程序不可用。")?,
        );
        instance.environment(&mut command);
        command.args(["upgrade", "--cask", "codex"]);
        command
    } else if instance.channel == Channel::Npm {
        let mut command = Command::new(instance.updater.as_ref().ok_or("npm 更新程序不可用。")?);
        instance.environment(&mut command);
        command.args(["install", "--global", "@openai/codex@latest"]);
        command
    } else if instance.channel == Channel::Standalone {
        instance
            .command(&["update".into()])
            .map_err(|e| e.to_string())?
    } else {
        return Err("请通过 Codex 原安装渠道更新；Companion 不会自动迁移安装方式。".into());
    };
    command.env("CODEX_NON_INTERACTIVE", "1");
    tui_deployment::run_bounded(&mut command, Duration::from_secs(900))
        .map_err(|e| e.to_string())?;
    if instance.id == "dodex" {
        tui_deployment::verify_package(instance).map_err(|e| e.to_string())?;
    }
    version(instance)?;
    Ok(())
}

fn version(instance: &InstanceConfig) -> Result<String, String> {
    let mut command = instance
        .command(&["--version".into()])
        .map_err(|e| e.to_string())?;
    let bytes = tui_deployment::run_bounded(&mut command, Duration::from_secs(20))
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let version = text
        .trim()
        .strip_prefix("codex-cli ")
        .ok_or("TUI 版本输出无效。")?;
    semver::Version::parse(version).map_err(|_| "TUI 版本无效。")?;
    Ok(version.into())
}
fn observe(snapshot: &mut Snapshot) {
    for (index, action) in [Action::UpdateCodex, Action::UpdateDodex]
        .into_iter()
        .enumerate()
    {
        let row = &mut snapshot.rows[index];
        row.error = false;
        match selected(action) {
            Some(instance) => {
                row.target = match instance.channel {
                    Channel::Homebrew => "Homebrew",
                    Channel::Npm => "npm",
                    Channel::Standalone => "官方 standalone",
                    Channel::Native => "原安装渠道",
                }
                .into();
                match version(&instance) {
                    Ok(value) => {
                        row.current = value;
                        row.message.clear();
                    }
                    Err(error) => {
                        row.current = "不可用".into();
                        row.error = true;
                        row.message = error;
                    }
                }
            }
            None => {
                row.current = "未安装".into();
                row.message.clear();
            }
        }
    }
}

#[derive(Clone)]
pub struct Service {
    state: Arc<Mutex<Snapshot>>,
}
impl Service {
    pub fn new() -> Self {
        let service = Self {
            state: Arc::new(Mutex::new(Snapshot::default())),
        };
        service.refresh();
        service
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn start(&self, action: Action) {
        self.start_job(Some(action));
    }
    pub fn refresh(&self) {
        self.start_job(None);
    }
    pub fn poll_requests(&self) -> bool {
        let action = {
            let mut requests = REQUEST
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.initializing {
                return false;
            }
            let pending = requests.pending.take();
            if requests.repeated || (state.busy && pending.is_some()) {
                state.notice = "已有更新操作正在进行；重复请求已忽略。".into();
                requests.repeated = false;
            }
            if state.busy { None } else { pending }
        };
        if let Some(action) = action {
            self.start(action);
            true
        } else {
            false
        }
    }
    fn start_job(&self, action: Option<Action>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.busy {
                return;
            }
            state.busy = true;
            state.initializing = action.is_none();
            state.error = false;
            state.message = action.map_or(
                "正在读取本机 TUI 版本（不联网）…".into(),
                |action| format!("正在更新 {} TUI…", action.id()),
            );
        }
        let service = self.clone();
        let result = std::thread::Builder::new()
            .name("tui-update".into())
            .spawn(move || {
                // Native version probes can take seconds. Keep them outside
                // the UI mutex so rendering and duplicate requests stay live.
                let mut snapshot = match action.map(run_action).transpose() {
                    Ok(Some(snapshot)) => snapshot,
                    Ok(None) => {
                        let mut snapshot = Snapshot::default();
                        observe(&mut snapshot);
                        snapshot
                    }
                    Err(error) => {
                        let mut snapshot = Snapshot::default();
                        observe(&mut snapshot);
                        snapshot.error = true;
                        snapshot.message = error;
                        snapshot
                    }
                };
                let mut state = service.state.lock().unwrap_or_else(|e| e.into_inner());
                snapshot.notice = state.notice.clone();
                *state = snapshot;
            });
        if result.is_err() {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.busy = false;
            state.initializing = false;
            state.error = true;
            state.message = "无法开始 TUI 更新，请重试。".into();
        }
    }
}
