//! Explicit, user initiated Codex/Dodex maintenance. Observing local versions
//! never contacts a server. App versions and terminal versions are independent.
use std::{
    cmp::Ordering,
    sync::{Arc, Mutex, OnceLock},
};

#[path = "software_updates/macos.rs"]
mod macos;
#[cfg(test)]
#[path = "software_updates/tests.rs"]
mod tests;

/// Validated read-only runtime metadata for navigation and quota. Saved legacy
/// deployment manifests still describe their original signed runtime.
pub(crate) fn managed_cli() -> Option<std::path::PathBuf> {
    macos::System::new().ok()?.managed_cli().ok().flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Action {
    Align,
    UpdateAll,
}

impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "align" => Some(Self::Align),
            "update-all" => Some(Self::UpdateAll),
            _ => None,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    CodexTui,
    DodexTui,
    CodexApp,
    DodexApp,
}
impl Target {
    const ALL: [Self; 4] = [
        Self::CodexTui,
        Self::DodexTui,
        Self::CodexApp,
        Self::DodexApp,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|target| *target == self).unwrap()
    }
    fn name(self) -> &'static str {
        match self {
            Self::CodexTui => "Codex TUI",
            Self::DodexTui => "Dodex TUI",
            Self::CodexApp => "Codex App",
            Self::DodexApp => "Dodex App",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Version {
    version: String,
    build: Option<String>,
}
impl Version {
    fn display(&self) -> String {
        match &self.build {
            Some(build) => format!("{} ({build})", self.version),
            None => self.version.clone(),
        }
    }
    fn compare(&self, other: &Self) -> Result<Ordering, String> {
        match (&self.build, &other.build) {
            (Some(left), Some(right)) => Ok(numeric_version(left)?.cmp(&numeric_version(right)?)),
            (None, None) => {
                let parse = |value: &str| {
                    semver::Version::parse(value)
                        .map_err(|_| "无法比较 TUI 版本；未替换程序。".to_owned())
                };
                Ok(parse(&self.version)?.cmp(&parse(&other.version)?))
            }
            _ => Err("版本类型不匹配；未替换程序。".into()),
        }
    }
}
fn numeric_version(value: &str) -> Result<Vec<u64>, String> {
    let mut result = value
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .map_err(|_| "无法比较 App 构建版本；未替换程序。".to_owned())?;
    while result.last() == Some(&0) {
        result.pop();
    }
    Ok(result)
}

#[derive(Clone, Debug)]
struct Release {
    version: Version,
    url: String,
    length: u64,
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
            rows: Target::ALL
                .into_iter()
                .map(|target| Row {
                    name: target.name().into(),
                    current: "尚未读取".into(),
                    target: "尚未检查".into(),
                    message: String::new(),
                    error: false,
                })
                .collect(),
            busy: false,
            message: "对齐使用本机 Codex；全部更新会检查官方稳定版。仅点击操作时联网。".into(),
            error: false,
            initializing: false,
            notice: String::new(),
        }
    }
}

trait Operations {
    fn installed(&self, target: Target) -> Result<Option<Version>, String>;
    fn latest(&self, app: bool) -> Result<Release, String>;
    fn preflight(&self, cli: bool, app: bool) -> Result<(), String>;
    fn app_needs_sync(&self) -> bool {
        false
    }
    /// Checks all affected desktop and terminal processes before any changes.
    fn require_stopped(&self) -> Result<(), String>;
    fn update_primary(&self, app: bool, release: &Release) -> Result<(), String>;
    fn align_secondary(&self, app: bool, expected: &Version) -> Result<(), String>;
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
        service.start_job(None);
        service
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn start(&self, action: Action) {
        self.start_job(Some(action));
    }
    /// Keep the initial menu request until local inspection has completed;
    /// requests during an actual update are rejected, never silently replayed.
    pub fn poll_requests(&self) -> bool {
        let action = {
            let mut requests = REQUEST
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            queued_action(&mut requests, &mut state)
        };
        if let Some(action) = action {
            self.start(action);
            return true;
        }
        false
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
            state.message = if action.is_some() {
                "正在检查版本与运行中的 App / TUI…"
            } else {
                "正在读取本机版本（不联网）…"
            }
            .into();
        }
        let service = self.clone();
        let result = std::thread::Builder::new()
            .name("codex-software-maintenance".into())
            .spawn(move || {
                let result = (|| {
                    let ops = macos::System::new()?;
                    // One editor must not replace software while another one is
                    // updating. The lock is process scoped and released on a crash.
                    let _lock = action.map(|_| ops.lock()).transpose()?;
                    execute(&ops, action, |change| {
                        let mut state = service.state.lock().unwrap_or_else(|e| e.into_inner());
                        change(&mut state);
                    })
                })();
                let mut state = service.state.lock().unwrap_or_else(|e| e.into_inner());
                state.busy = false;
                state.initializing = false;
                if let Err(error) = result {
                    state.error = true;
                    state.message = error;
                }
            });
        if result.is_err() {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.busy = false;
            state.initializing = false;
            state.error = true;
            state.message = "无法开始更新任务，请重试。".into();
        }
    }
}

fn queued_action(requests: &mut Requests, state: &mut Snapshot) -> Option<Action> {
    if state.initializing {
        return None;
    }
    if !state.busy && requests.pending.is_some() && !requests.repeated {
        state.notice.clear();
    }
    let action = requests.pending.take();
    if requests.repeated || (state.busy && action.is_some()) {
        state.notice = "已有操作正在等待或进行中；重复请求已忽略。请等待结果后再操作。".into();
        requests.repeated = false;
    }
    if state.busy { None } else { action }
}

type Change<'a> = Box<dyn FnOnce(&mut Snapshot) + 'a>;
fn execute(
    ops: &dyn Operations,
    action: Option<Action>,
    mut report: impl FnMut(Change<'_>),
) -> Result<(), String> {
    let mut installed: [Option<Version>; 4] = Default::default();
    let mut inspection_error = None;
    for target in Target::ALL {
        let observed = ops.installed(target);
        let (current, error) = match &observed {
            Ok(Some(version)) => (version.display(), None),
            Ok(None) => ("未安装 / 未接入".into(), None),
            Err(error) => {
                inspection_error = Some(error.clone());
                ("无法读取".into(), Some(error.clone()))
            }
        };
        installed[target.index()] = observed.ok().flatten();
        report(Box::new(move |state| {
            let row = &mut state.rows[target.index()];
            row.current = current;
            row.target = "尚未检查".into();
            row.error = error.is_some();
            row.message = error.unwrap_or_default();
        }));
    }
    let Some(action) = action else {
        report(Box::new(|state| {
            state.message = "已读取本机版本；点击「对齐」或「全部更新」继续。".into()
        }));
        return Ok(());
    };
    if let Some(error) = inspection_error {
        return Err(format!("无法确认现有安装：{error}"));
    }
    let mut releases: [Option<Release>; 2] = [None, None];
    if action == Action::UpdateAll {
        report(Box::new(|state| {
            state.message = "正在检查官方 TUI 与 App 稳定版…".into()
        }));
        // Fetch both successfully before making either local installation change.
        releases[0] = Some(ops.latest(false)?);
        releases[1] = Some(ops.latest(true)?);
    }
    // Plan the complete operation first. A secondary instance newer than its
    // proposed source must never be silently downgraded.
    let mut plan = Vec::new();
    let mut skipped = Vec::new();
    for app in [false, true] {
        let (primary, secondary) = if app {
            (Target::CodexApp, Target::DodexApp)
        } else {
            (Target::CodexTui, Target::DodexTui)
        };
        let current = installed[primary.index()]
            .as_ref()
            .ok_or_else(|| format!("未找到 {}；请先安装官方程序，再重试。", primary.name()))?;
        let desired = match &releases[usize::from(app)] {
            Some(release) if current.compare(&release.version)? == Ordering::Greater => {
                skipped.push(format!("{} 高于官方稳定版，已保留", primary.name()));
                current.clone()
            }
            Some(release) => release.version.clone(),
            None => current.clone(),
        };
        for target in [primary, secondary] {
            let displayed = desired.display();
            let label = if action == Action::Align {
                "目标：本机 Codex"
            } else {
                "目标：官方稳定版（保留更高版本）"
            };
            report(Box::new(move |state| {
                state.rows[target.index()].target = displayed;
                state.rows[target.index()].message = label.into();
            }));
        }
        let newer_secondary = installed[secondary.index()]
            .as_ref()
            .map(|value| value.compare(&desired))
            .transpose()?
            == Some(Ordering::Greater);
        if newer_secondary {
            skipped.push(format!(
                "{} 较新，未降级；请使用「全部更新到最新」或检查官方渠道",
                secondary.name()
            ));
            report(Box::new(move |state| {
                state.rows[secondary.index()].message =
                    "Dodex 比目标更新，已保留；可尝试全部更新。".into();
            }));
        }
        let update_primary = current.compare(&desired)? == Ordering::Less;
        let update_secondary = !newer_secondary
            && (installed[secondary.index()].as_ref() != Some(&desired)
                || app && ops.app_needs_sync());
        plan.push((app, desired, update_primary, update_secondary));
    }
    if plan
        .iter()
        .any(|(_, _, primary, secondary)| *primary || *secondary)
    {
        // The new terminal adapter sends workspaces through the public App.
        // A newer Dodex App is retained, so its old bootstrap cannot be upgraded
        // by copying the older primary. Fail before changing either component.
        if plan.iter().any(|(app, _, _, secondary)| !app && *secondary)
            && ops.app_needs_sync()
            && !plan.iter().any(|(app, _, _, secondary)| *app && *secondary)
        {
            return Err("Dodex App 较新，但启动器需要更新；为保留工作区打开行为，未修改任何程序。请使用「全部更新到最新」后重试。".into());
        }
        ops.preflight(
            plan.iter()
                .any(|(app, _, primary, secondary)| !*app && (*primary || *secondary)),
            plan.iter()
                .any(|(app, _, primary, secondary)| *app && (*primary || *secondary)),
        )?;
        ops.require_stopped()?;
    }
    let result = (|| {
        // Publish the App bootstrap before migrating the terminal entry so its
        // `dodex app <project>` route is available even after a later failure.
        for (app, desired, update_primary, update_secondary) in plan.iter().rev() {
            let (primary, secondary) = if *app {
                (Target::CodexApp, Target::DodexApp)
            } else {
                (Target::CodexTui, Target::DodexTui)
            };
            if *update_primary {
                report(Box::new(move |state| {
                    state.message =
                        format!("正在更新 {}；请保持相关 App / TUI 关闭…", primary.name())
                }));
                ops.require_stopped()?;
                ops.update_primary(*app, releases[usize::from(*app)].as_ref().unwrap())?;
                verify_result(ops, primary, desired)?;
            }
            if *update_secondary {
                report(Box::new(move |state| {
                    state.message = format!(
                        "正在对齐 {}；账号、会话与配置目录保持原样…",
                        secondary.name()
                    )
                }));
                ops.require_stopped()?;
                ops.align_secondary(*app, desired)?;
                verify_result(ops, secondary, desired)?;
            }
        }
        Ok::<(), String>(())
    })();
    // Always re-observe after a partial failure; never claim both sides match
    // from the original plan or a package manager's exit code alone.
    for target in Target::ALL {
        let observed = ops.installed(target);
        report(Box::new(move |state| {
            let row = &mut state.rows[target.index()];
            match observed {
                Ok(Some(value)) => {
                    row.current = value.display();
                    if row.current == row.target {
                        row.message = "已达到本次目标版本".into();
                    }
                }
                Ok(None) => row.current = "未安装 / 未接入".into(),
                Err(error) => {
                    row.current = "无法读取".into();
                    row.message = error;
                    row.error = true;
                }
            }
        }));
    }
    result.map_err(|error| {
        format!("操作未全部完成：{error} 已完成的更新会保留；关闭相关程序后可重试。")
    })?;
    report(Box::new(move |state| {
        state.message = if skipped.is_empty() {
            if action == Action::Align {
                "Codex / Dodex 的 TUI 与 App 已分别对齐。"
            } else {
                "已检查官方稳定版，并分别对齐 Codex / Dodex 的 TUI 与 App。"
            }
            .into()
        } else {
            skipped.join("；")
        };
    }));
    Ok(())
}
fn verify_result(ops: &dyn Operations, target: Target, desired: &Version) -> Result<(), String> {
    if ops.installed(target)?.as_ref() != Some(desired) {
        return Err(format!(
            "{} 安装后的版本与目标不符，未标记为完成。",
            target.name()
        ));
    }
    Ok(())
}
