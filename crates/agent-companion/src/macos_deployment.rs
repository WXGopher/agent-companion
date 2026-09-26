//! Opt-in, local-only Dodex deployment. This module never starts either application,
//! reads credential files, or discovers a second profile before an explicit opt-in.
use agent_companion_core::install::profile_sync::{
    self, Direction, FileKind, IsolationPaths, ProfilePair, SyncOutcome,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 1;
const MARKER: &str = "companion-deployment-complete.json";
const MANIFEST: &str = "companion-deployment.json";
const INSTRUCTIONS: &str = "在应用程序中打开 Dodex，首次使用请登录。";
const OFFICIAL_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.openai.codex\" and certificate leaf[subject.OU] = \"2DC432GLL2\"";
// Compatibility with the audited, original Codex-B manager/compiled Dodex launcher.
// Hashes describe program code only. Unknown revisions fail closed, without running
// the manager or touching its state. No local manager or profile is distributed.
const LEGACY_LAUNCHER_SHA256: &str =
    "ca77766af2e90d176fdcae99ae198cb151883305f67ef2802f0a453f7e1e4000";
const LEGACY_PLIST_SHA256: &str =
    "6040958602f1574a394117d4c75d0243a3aaaddfc843c3a5404ba694d6342312";
const LEGACY_MANAGER_SHA256: &str =
    "777d6146f9f101c14e9a96afebff824767c5d820c6e3f05a91af23bc792565ae";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceConfig {
    pub id: String,
    pub label: String,
    pub codex_home: PathBuf,
    pub desktop_user_data: PathBuf,
    pub database_dir: PathBuf,
    pub runtime_app: PathBuf,
    pub launcher_app: PathBuf,
    pub cli_path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeploymentStatus {
    pub enabled: bool,
    pub deployed: bool,
    pub busy: bool,
    pub phase: String,
    pub message: String,
}
impl Default for DeploymentStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            deployed: false,
            busy: false,
            phase: "not_deployed".into(),
            message: "创建独立环境，首次使用需要自行登录。".into(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    schema: u32,
    enabled: bool,
    instance: InstanceConfig,
}
#[derive(Clone, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    instance: InstanceConfig,
}
type PreferenceStamp = Option<(u64, u64, u128)>;
type CompletedDeployment = (InstanceConfig, PreferenceStamp);

#[derive(Default)]
struct State {
    status: DeploymentStatus,
    instance: Option<InstanceConfig>,
    initialized: bool,
    preference_stamp: PreferenceStamp,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static SYNC_INSTANCE: OnceLock<Mutex<Option<(PreferenceStamp, InstanceConfig)>>> = OnceLock::new();

#[derive(Clone)]
struct Layout {
    user_home: PathBuf,
    support: PathBuf,
    applications: PathBuf,
    system_applications: PathBuf,
}
impl Layout {
    fn current() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("无法定位用户目录。")?;
        if !home.is_absolute() {
            return Err("用户目录必须是绝对路径。".into());
        }
        Ok(Self::for_home(home))
    }
    fn for_home(user_home: PathBuf) -> Self {
        Self {
            support: user_home.join("Library/Application Support/AgentCompanion"),
            applications: user_home.join("Applications"),
            system_applications: PathBuf::from("/Applications"),
            user_home,
        }
    }
    fn root(&self) -> PathBuf {
        self.support.join("Dodex")
    }
    fn settings(&self) -> PathBuf {
        self.support.join("dual-instance.json")
    }
    fn instance(&self) -> InstanceConfig {
        let root = self.root();
        let runtime = root.join("Runtime.app");
        InstanceConfig {
            id: "dodex".into(),
            label: "Dodex".into(),
            codex_home: root.join("codex-home"),
            desktop_user_data: root.join("desktop-data"),
            database_dir: root.join("codex-home/sqlite"),
            cli_path: runtime.join("Contents/Resources/codex"),
            runtime_app: runtime,
            launcher_app: self.applications.join("Dodex.app"),
        }
    }
}

fn shared() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}
fn preference_stamp(layout: &Layout) -> PreferenceStamp {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(layout.settings()).ok()?;
    Some((
        metadata.ino(),
        metadata.len(),
        metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    ))
}
fn read_saved_state(layout: &Layout) -> State {
    let mut state = State {
        initialized: true,
        preference_stamp: preference_stamp(layout),
        ..State::default()
    };
    // Only Companion's own preference is read before an explicit opt-in.
    if state.preference_stamp.is_none() {
        return state;
    }
    match read_json::<Record>(&layout.settings()) {
        Ok(record) if record.schema == SCHEMA => {
            state.status.deployed = true;
            state.instance = Some(record.instance);
            if record.enabled {
                state.status.busy = true;
                state.status.phase = "checking".into();
                state.status.message = "正在验证已启用的 Dodex 环境…".into();
            } else {
                state.status.phase = "disabled".into();
                state.status.message = "双实例支持已停用，Dodex 环境仍保留。".into();
            }
        }
        _ => {
            state.status.phase = "failed".into();
            state.status.message = "双开设置无效；未启用第二实例。".into();
        }
    }
    state
}
fn refresh_from_disk() {
    let Ok(layout) = Layout::current() else {
        return;
    };
    let stamp = preference_stamp(&layout);
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    if state.status.busy || (state.initialized && state.preference_stamp == stamp) {
        return;
    }
    *state = read_saved_state(&layout);
    if !state.status.busy {
        return;
    }
    let instance = state.instance.clone().expect("enabled record has instance");
    // Signatures are validated once per saved preference generation, on a worker.
    // This also synchronizes the settings subprocess with the resident monitor.
    std::thread::spawn(move || {
        let result = validate_existing(&layout, &SystemOps, &instance);
        let current_stamp = preference_stamp(&layout);
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        if !validation_is_current(&mut state, stamp, current_stamp) {
            return;
        }
        state.status.busy = false;
        match result {
            Ok(()) => {
                state.status.enabled = true;
                state.status.phase = "ready".into();
                state.status.message = INSTRUCTIONS.into();
            }
            Err(error) => {
                state.status.enabled = false;
                state.status.phase = "failed".into();
                state.status.message = error;
            }
        }
    });
}
fn validation_is_current(
    state: &mut State,
    requested: PreferenceStamp,
    current: PreferenceStamp,
) -> bool {
    if state.preference_stamp != requested || current != requested {
        state.status.enabled = false;
        state.status.busy = false;
        state.initialized = false;
        return false;
    }
    true
}
pub fn status() -> DeploymentStatus {
    refresh_from_disk();
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status
        .clone()
}
pub fn active_instance() -> Option<InstanceConfig> {
    refresh_from_disk();
    let state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state
        .status
        .enabled
        .then(|| state.instance.clone())
        .flatten()
}

/// Read-only discovery also permits a saved, disabled deployment. Cache the
/// manifest/path checks; runtime signatures are checked by the sync worker.
pub fn profile_sync_paths() -> Result<ProfilePair, String> {
    let layout = Layout::current()?;
    let stamp = preference_stamp(&layout);
    if stamp.is_none() {
        return Err("请先部署 Dodex，再同步文件。".into());
    }
    let mut cached = SYNC_INSTANCE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let instance = {
        let state = shared().lock().unwrap_or_else(|e| e.into_inner());
        if state.status.busy {
            return Err("双开操作正在进行，请稍后重试。".into());
        }
        if state.status.enabled && state.preference_stamp == stamp {
            state.instance.clone()
        } else {
            cached
                .as_ref()
                .filter(|(saved, _)| *saved == stamp)
                .map(|(_, instance)| instance.clone())
        }
    };
    let instance = match instance {
        Some(instance) => instance,
        None => sync_operation(&layout, || saved_sync_instance(&layout, &SystemOps, false))?,
    };
    *cached = Some((stamp, instance.clone()));
    sync_pair(&layout.user_home.join(".codex"), &instance)
}

/// Blocking explicit action. The same operation state and cross-process lock
/// serialize sync with deploy and enable/disable; the preference is not changed.
pub fn sync_profile_file(kind: FileKind, direction: Direction) -> Result<SyncOutcome, String> {
    let layout = Layout::current()?;
    sync_operation(&layout, || {
        sync_profile_file_under_lock(
            &layout,
            &SystemOps,
            &layout.user_home.join(".codex"),
            kind,
            direction,
        )
    })
}

fn sync_operation<T>(
    layout: &Layout,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        if state.status.busy {
            return Err("双开操作正在进行，请稍后重试。".into());
        }
        state.status.busy = true;
    }
    let result = (|| {
        no_symlinks(&layout.support)?;
        let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
        action()
    })();
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status
        .busy = false;
    result
}

fn saved_sync_instance(
    layout: &Layout,
    ops: &dyn Operations,
    runtime: bool,
) -> Result<InstanceConfig, String> {
    let record: Record =
        read_json(&layout.settings()).map_err(|_| "请先部署 Dodex，再同步文件。")?;
    if record.schema != SCHEMA {
        return Err("Dodex 部署记录版本不兼容。".into());
    }
    validate_existing_for_sync(layout, ops, &record.instance, runtime)?;
    Ok(record.instance)
}

fn sync_pair(primary: &Path, instance: &InstanceConfig) -> Result<ProfilePair, String> {
    Ok(ProfilePair {
        primary: profile_sync::profile_paths(primary)
            .map_err(|_| "Codex 文件路径不可用或包含重定向。")?,
        secondary: profile_sync::profile_paths(&instance.codex_home)
            .map_err(|_| "Dodex 文件路径不可用或包含重定向。")?,
        secondary_isolation: IsolationPaths {
            sqlite_home: instance.database_dir.clone(),
            log_dir: instance.desktop_user_data.join("logs"),
        },
    })
}

fn sync_profile_file_under_lock(
    layout: &Layout,
    ops: &dyn Operations,
    primary: &Path,
    kind: FileKind,
    direction: Direction,
) -> Result<SyncOutcome, String> {
    let instance = saved_sync_instance(layout, ops, true)?;
    let pair = sync_pair(primary, &instance)?;
    profile_sync::sync_file(&pair, kind, direction).map_err(|error| error.to_string())
}
fn begin() -> Result<(), String> {
    refresh_from_disk();
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    if state.status.busy {
        return Err("正在处理双开环境，请等待完成。".into());
    }
    state.status.busy = true;
    state.status.phase = "checking".into();
    state.status.message = "正在检查应用与隔离目录…".into();
    Ok(())
}
fn progress(phase: &str, message: &str) {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.status.phase = phase.into();
    state.status.message = message.into();
}
fn finish(result: Result<CompletedDeployment, String>) -> Result<DeploymentStatus, String> {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.status.busy = false;
    match result {
        Ok((instance, stamp)) => {
            state.preference_stamp = stamp;
            state.initialized = true;
            if Layout::current()
                .ok()
                .and_then(|layout| preference_stamp(&layout))
                != stamp
            {
                state.status.enabled = false;
                state.initialized = false;
                state.status.phase = "checking".into();
                state.status.message = "正在同步双开设置…".into();
                return Ok(state.status.clone());
            }
            state.instance = Some(instance);
            state.status.deployed = true;
            state.status.enabled = true;
            state.status.phase = "ready".into();
            state.status.message = INSTRUCTIONS.into();
            Ok(state.status.clone())
        }
        Err(error) => {
            state.status.enabled = false;
            state.status.phase = "failed".into();
            state.status.message = error.clone();
            Err(error)
        }
    }
}
/// Blocking operation. Call on a worker thread; `status` remains pollable.
pub fn deploy() -> Result<DeploymentStatus, String> {
    begin()?;
    let result = (|| {
        let layout = Layout::current()?;
        private_directory(&layout.support)?;
        let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
        let instance = deploy_under_lock(&layout, &SystemOps, progress)?;
        save_record(&layout, true, &instance)?;
        Ok((instance, preference_stamp(&layout)))
    })();
    finish(result)
}
/// Disabling preserves files and running processes. Enabling validates before use.
pub fn set_enabled(enabled: bool) -> Result<DeploymentStatus, String> {
    if enabled {
        begin()?;
        let result = (|| {
            let layout = Layout::current()?;
            let instance = shared()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .instance
                .clone()
                .ok_or("请先部署双开环境。")?;
            private_directory(&layout.support)?;
            let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
            validate_existing(&layout, &SystemOps, &instance)?;
            save_record(&layout, true, &instance)?;
            Ok((instance, preference_stamp(&layout)))
        })();
        return finish(result);
    }
    refresh_from_disk();
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    if state.status.busy {
        return Err("正在处理双开环境，请等待完成。".into());
    }
    if let Some(instance) = &state.instance {
        let layout = Layout::current()?;
        private_directory(&layout.support)?;
        let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
        save_record(&layout, false, instance)?;
        state.preference_stamp = preference_stamp(&layout);
        state.initialized = true;
    }
    state.status.enabled = false;
    state.status.phase = if state.status.deployed {
        "disabled"
    } else {
        "not_deployed"
    }
    .into();
    state.status.message = if state.status.deployed {
        "双实例支持已停用，Dodex 环境仍保留。"
    } else {
        "创建独立环境，首次使用需要自行登录。"
    }
    .into();
    Ok(state.status.clone())
}
fn save_record(layout: &Layout, enabled: bool, instance: &InstanceConfig) -> Result<(), String> {
    private_directory(&layout.support)?;
    write_atomic(
        &layout.settings(),
        &serde_json::to_vec(&Record {
            schema: SCHEMA,
            enabled,
            instance: instance.clone(),
        })
        .map_err(|_| "无法保存双开设置。")?,
    )
}

trait Operations {
    fn verify_runtime(&self, app: &Path) -> Result<(), String>;
    fn copy_runtime(&self, source: &Path, destination: &Path) -> Result<(), String>;
    fn legacy_fingerprints_match(&self, launcher: &Path, manager: &Path) -> bool;
}
struct SystemOps;
impl Operations for SystemOps {
    fn verify_runtime(&self, app: &Path) -> Result<(), String> {
        no_symlinks(app)?;
        for relative in [
            "Contents/Info.plist",
            "Contents/MacOS/ChatGPT",
            "Contents/Resources/codex",
            "Contents/Resources/app.asar",
        ] {
            regular_file(&app.join(relative))?;
        }
        let status = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "-R", OFFICIAL_REQUIREMENT])
            .arg(app)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "无法执行应用签名校验。")?;
        if !status.success() {
            return Err("Codex 应用签名无效或不是 OpenAI 官方签名；未修改现有环境。".into());
        }
        if !runtime_supports_isolation(&app.join("Contents/Resources/app.asar"))? {
            return Err("此版本 Codex 缺少独立运行环境支持，请先更新官方应用。".into());
        }
        Ok(())
    }
    fn copy_runtime(&self, source: &Path, destination: &Path) -> Result<(), String> {
        let status = Command::new("/usr/bin/ditto")
            .arg(source)
            .arg(destination)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "无法复制 Codex 运行程序。")?;
        if !status.success() {
            return Err("复制 Codex 运行程序失败，请检查可用空间和目录权限。".into());
        }
        Ok(())
    }
    fn legacy_fingerprints_match(&self, launcher: &Path, manager: &Path) -> bool {
        file_sha256(launcher).as_deref() == Some(LEGACY_LAUNCHER_SHA256)
            && file_sha256(manager).as_deref() == Some(LEGACY_MANAGER_SHA256)
            && launcher
                .parent()
                .and_then(Path::parent)
                .and_then(|contents| file_sha256(&contents.join("Info.plist")))
                .as_deref()
                == Some(LEGACY_PLIST_SHA256)
    }
}
const RUNTIME_FEATURES: [&[u8]; 3] = [
    b"CODEX_ELECTRON_USER_DATA_PATH",
    b"CODEX_CLI_PATH",
    b"CODEX_APP_SERVER_FORCE_CLI",
];

fn runtime_supports_isolation(path: &Path) -> Result<bool, String> {
    regular_file(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(no_follow_flag())
        .open(path)
        .map_err(|_| "无法读取 Codex 运行程序资源。")?;
    read_runtime_features(file).map_err(|_| "无法校验 Codex 运行程序资源。".into())
}

fn read_runtime_features(mut reader: impl Read) -> std::io::Result<bool> {
    // Signed app archives grow independently of small configuration files.
    // Scan with fixed memory and retain enough overlap for split markers.
    let overlap = RUNTIME_FEATURES
        .iter()
        .map(|feature| feature.len())
        .max()
        .unwrap()
        - 1;
    let mut buffer = vec![0; 65536 + overlap];
    let mut retained = 0;
    let mut found = [false; RUNTIME_FEATURES.len()];
    loop {
        let count = match reader.read(&mut buffer[retained..]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(false);
        }
        let length = retained + count;
        for (index, feature) in RUNTIME_FEATURES.iter().enumerate() {
            if !found[index] {
                found[index] = buffer[..length]
                    .windows(feature.len())
                    .any(|slice| slice == *feature);
            }
        }
        if found.iter().all(|present| *present) {
            return Ok(true);
        }
        retained = overlap.min(length);
        buffer.copy_within(length - retained..length, 0);
    }
}

fn file_sha256(path: &Path) -> Option<String> {
    regular_file(path).ok()?;
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

#[cfg(test)]
fn deploy_with(
    layout: &Layout,
    ops: &dyn Operations,
    notify: impl FnMut(&str, &str),
) -> Result<InstanceConfig, String> {
    // Directory creation happens only after the user selects Deploy.
    private_directory(&layout.support)?;
    let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
    deploy_under_lock(layout, ops, notify)
}
fn deploy_under_lock(
    layout: &Layout,
    ops: &dyn Operations,
    mut notify: impl FnMut(&str, &str),
) -> Result<InstanceConfig, String> {
    let instance = layout.instance();
    let legacy_launcher = layout.system_applications.join("Dodex.app");
    if exists(&instance.launcher_app) || exists(&layout.root()) {
        if exists(&legacy_launcher) {
            return Err("发现两个 Dodex 环境，无法确定接入目标；未修改任何环境。".into());
        }
        recover_or_validate_managed(layout, ops, &instance)?;
        return Ok(instance);
    }
    if exists(&legacy_launcher) {
        notify("verifying", "正在校验已有 Dodex；保留原有程序、配置和数据…");
        return validate_legacy(layout, ops);
    }
    let sources = [
        layout.system_applications.join("Codex.app"),
        layout.applications.join("Codex.app"),
    ];
    let source = sources
        .iter()
        .find(|path| exists(path))
        .ok_or("未找到官方 Codex 应用，请先将 Codex 安装到应用程序目录。")?;
    ops.verify_runtime(source)?;
    private_directory(&layout.applications)?;
    let suffix = transaction_id();
    let stage = layout.support.join(format!(".dodex-stage-{suffix}"));
    let launcher_stage = layout
        .applications
        .join(format!(".Dodex-stage-{suffix}.app"));
    let mut cleanup = OwnedArtifacts::default();
    create_private(&stage)?;
    cleanup.paths.push(stage.clone());
    create_private(&launcher_stage)?;
    cleanup.paths.push(launcher_stage.clone());
    notify("copying", "正在复制官方运行程序并保留原始签名…");
    ops.copy_runtime(source, &stage.join("Runtime.app"))?;
    notify("configuring", "正在创建独立配置、数据库与桌面缓存…");
    for relative in [
        "codex-home",
        "codex-home/sqlite",
        "desktop-data",
        "desktop-data/Cache",
        "desktop-data/logs",
    ] {
        private_directory(&stage.join(relative))?;
    }
    write_new(
        &stage.join("codex-home/config.toml"),
        config_text(&instance).as_bytes(),
        0o600,
    )?;
    write_new(
        &stage.join(MANIFEST),
        &serde_json::to_vec(&Manifest {
            schema: SCHEMA,
            instance: instance.clone(),
        })
        .map_err(|_| "无法创建部署清单。")?,
        0o600,
    )?;
    build_launcher(layout, &instance, &launcher_stage)?;
    notify("verifying", "正在验证签名与隔离配置…");
    ops.verify_runtime(&stage.join("Runtime.app"))?;
    validate_config(&stage.join("codex-home/config.toml"), &instance)?;
    validate_launcher(layout, &instance, &launcher_stage)?;
    notify("finishing", "正在完成部署…");
    rename_exclusive(&stage, &layout.root())?;
    cleanup.paths.retain(|path| path != &stage);
    cleanup.paths.push(layout.root());
    rename_exclusive(&launcher_stage, &instance.launcher_app)?;
    cleanup.paths.retain(|path| path != &launcher_stage);
    cleanup.paths.push(instance.launcher_app.clone());
    validate_instance_paths(layout, &instance)?;
    // The marker is last. No second-instance support is enabled before this point.
    write_new_atomic(
        &layout.root().join(MARKER),
        &serde_json::to_vec(&Manifest {
            schema: SCHEMA,
            instance: instance.clone(),
        })
        .map_err(|_| "无法完成部署标记。")?,
        0o600,
    )?;
    cleanup.paths.clear();
    Ok(instance)
}
fn recover_or_validate_managed(
    layout: &Layout,
    ops: &dyn Operations,
    instance: &InstanceConfig,
) -> Result<(), String> {
    if exists(&layout.root().join(MARKER)) {
        return validate_managed(layout, ops, instance);
    }
    // A manifest proves this is our interrupted publication. Foreign directories
    // without that exact record are never repaired, replaced, or removed.
    let manifest: Manifest = read_json(&layout.root().join(MANIFEST))
        .map_err(|_| "Dodex 目录与已有文件冲突；未覆盖任何文件。")?;
    if manifest.schema != SCHEMA || manifest.instance != *instance {
        return Err("Dodex 中断部署清单不兼容；未覆盖任何文件。".into());
    }
    validate_instance_paths(layout, instance)?;
    validate_config(&instance.codex_home.join("config.toml"), instance)?;
    ops.verify_runtime(&instance.runtime_app)?;
    let mut cleanup = OwnedArtifacts::default();
    if exists(&instance.launcher_app) {
        validate_launcher(layout, instance, &instance.launcher_app)?;
    } else {
        private_directory(&layout.applications)?;
        let stage = layout
            .applications
            .join(format!(".Dodex-stage-{}.app", transaction_id()));
        create_private(&stage)?;
        cleanup.paths.push(stage.clone());
        build_launcher(layout, instance, &stage)?;
        validate_launcher(layout, instance, &stage)?;
        rename_exclusive(&stage, &instance.launcher_app)?;
        cleanup.paths.retain(|path| path != &stage);
        cleanup.paths.push(instance.launcher_app.clone());
    }
    write_new_atomic(
        &layout.root().join(MARKER),
        &serde_json::to_vec(&Manifest {
            schema: SCHEMA,
            instance: instance.clone(),
        })
        .map_err(|_| "无法完成部署标记。")?,
        0o600,
    )?;
    cleanup.paths.clear();
    Ok(())
}
fn validate_existing(
    layout: &Layout,
    ops: &dyn Operations,
    instance: &InstanceConfig,
) -> Result<(), String> {
    if instance == &layout.instance() {
        validate_managed(layout, ops, instance)
    } else if &validate_legacy(layout, ops)? == instance {
        Ok(())
    } else {
        Err("已有双开环境的路径发生变化；未启用第二实例。".into())
    }
}
fn validate_existing_for_sync(
    layout: &Layout,
    ops: &dyn Operations,
    instance: &InstanceConfig,
    runtime: bool,
) -> Result<(), String> {
    if instance == &layout.instance() {
        validate_managed_with_config(layout, ops, instance, false, runtime)
    } else if &validate_legacy_with_config(layout, ops, false, runtime)? == instance {
        Ok(())
    } else {
        Err("已有双开环境的路径发生变化；未同步文件。".into())
    }
}
fn validate_managed(
    layout: &Layout,
    ops: &dyn Operations,
    instance: &InstanceConfig,
) -> Result<(), String> {
    validate_managed_with_config(layout, ops, instance, true, true)
}
fn validate_managed_with_config(
    layout: &Layout,
    ops: &dyn Operations,
    instance: &InstanceConfig,
    require_config: bool,
    runtime: bool,
) -> Result<(), String> {
    no_symlinks(&layout.root())?;
    for name in [MANIFEST, MARKER] {
        let manifest: Manifest = read_json(&layout.root().join(name))
            .map_err(|_| "Dodex 部署不完整或与现有目录冲突；未覆盖任何文件。")?;
        if manifest.schema != SCHEMA || &manifest.instance != instance {
            return Err("Dodex 部署清单不兼容；未覆盖任何文件。".into());
        }
    }
    validate_instance_paths(layout, instance)?;
    let config = instance.codex_home.join("config.toml");
    if require_config || exists(&config) {
        validate_config(&config, instance)?;
    }
    validate_launcher(layout, instance, &instance.launcher_app)?;
    if runtime {
        ops.verify_runtime(&instance.runtime_app)?;
    }
    Ok(())
}
fn validate_legacy(layout: &Layout, ops: &dyn Operations) -> Result<InstanceConfig, String> {
    validate_legacy_with_config(layout, ops, true, true)
}
fn validate_legacy_with_config(
    layout: &Layout,
    ops: &dyn Operations,
    require_config: bool,
    check_runtime: bool,
) -> Result<InstanceConfig, String> {
    let runtime = layout.system_applications.join("Codex B Runtime.app");
    let home = layout.user_home.join(".codex-second");
    let data = layout.user_home.join("Library/Application Support/Codex-B");
    let launcher = layout.system_applications.join("Dodex.app");
    let manager = data.join("tools/codex_b_manager.py");
    let executable = legacy_launcher_bundle(layout)?.join("Contents/MacOS/Dodex");
    regular_file(&manager)?;
    if !ops.legacy_fingerprints_match(&executable, &manager) {
        return Err(
            "已有 Dodex 的启动配置版本不兼容，无法安全接入；未修改程序、配置或数据。".into(),
        );
    }
    // The allowlisted manager is fixed-path. Also bind its actual user root and
    // launcher path so a copied launcher cannot accidentally open another home.
    let manager_code = read_limited(&manager, 256 * 1024)?;
    let launcher_code = read_limited(&executable, 2 * 1024 * 1024)?;
    let user_root = format!(
        "USER_ROOT = Path({})",
        serde_json::to_string(&layout.user_home.to_string_lossy())
            .map_err(|_| "无法校验用户路径。")?
    );
    let manager_path = manager.to_string_lossy();
    if !manager_code
        .windows(user_root.len())
        .any(|s| s == user_root.as_bytes())
        || !launcher_code
            .windows(manager_path.len())
            .any(|s| s == manager_path.as_bytes())
    {
        return Err("已有 Dodex 绑定了不同用户目录，无法安全接入。".into());
    }
    let instance = InstanceConfig {
        id: "dodex".into(),
        label: "Dodex".into(),
        database_dir: home.join("sqlite"),
        codex_home: home,
        desktop_user_data: data,
        cli_path: runtime.join("Contents/Resources/codex"),
        runtime_app: runtime,
        launcher_app: launcher,
    };
    validate_instance_paths(layout, &instance)?;
    let config = instance.codex_home.join("config.toml");
    if require_config || exists(&config) {
        validate_config(&config, &instance)?;
    }
    if check_runtime {
        ops.verify_runtime(&instance.runtime_app)?;
    }
    Ok(instance)
}

fn legacy_launcher_bundle(layout: &Layout) -> Result<PathBuf, String> {
    no_symlinks(&layout.system_applications)?;
    let launcher = layout.system_applications.join("Dodex.app");
    let bundle = launcher
        .canonicalize()
        .map_err(|_| "已有 Dodex 启动器缺失或启动别名已失效；未修改现有环境。")?;
    // The original installation exposes Dodex.app as an application alias.
    // Resolve only this known entry-point layout, then authenticate the real
    // executable and plist below. Profile/runtime paths retain strict checks.
    if bundle != launcher && bundle != layout.system_applications.join("Codex B.app") {
        return Err("已有 Dodex 启动别名未指向受支持的 Codex B.app；未修改现有环境。".into());
    }
    no_symlinks(&bundle)?;
    if !bundle.is_dir() {
        return Err("已有 Dodex 启动目标不是应用目录；未修改现有环境。".into());
    }
    Ok(bundle)
}

fn validate_instance_paths(layout: &Layout, instance: &InstanceConfig) -> Result<(), String> {
    for path in [
        &instance.codex_home,
        &instance.desktop_user_data,
        &instance.database_dir,
    ] {
        no_symlinks(path)?;
        if !path.is_dir() {
            return Err("Dodex 隔离目录缺失；未修改现有环境。".into());
        }
    }
    // Check credential locations for redirection without opening their contents.
    for relative in ["auth.json", "config.toml", "sessions", "archived_sessions"] {
        no_symlinks(&instance.codex_home.join(relative))?;
    }
    // SQLite opens redirecting files even when its directory itself is genuine.
    // Reject links (including hard links) using metadata, never database contents.
    use std::os::unix::fs::MetadataExt;
    for entry in fs::read_dir(&instance.database_dir).map_err(|_| "无法校验 Dodex 数据库目录。")?
    {
        let path = entry.map_err(|_| "无法校验 Dodex 数据库目录。")?.path();
        no_symlinks(&path)?;
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file() && meta.nlink() > 1) {
            return Err("Dodex 数据库文件与其他目录共享硬链接，无法确认隔离。".into());
        }
    }
    for relative in ["auth.json", "config.toml"] {
        if fs::symlink_metadata(instance.codex_home.join(relative))
            .is_ok_and(|meta| meta.is_file() && meta.nlink() > 1)
        {
            return Err("Dodex 凭证或配置与其他目录共享硬链接，无法确认隔离。".into());
        }
    }
    let primary_home = layout.user_home.join(".codex");
    let primary_database = agent_companion_core::dashboard::database_home(&primary_home);
    for primary in [
        &primary_home,
        &primary_database,
        &layout.user_home.join("Library/Application Support/Codex"),
    ] {
        for secondary in [
            &instance.codex_home,
            &instance.database_dir,
            &instance.desktop_user_data,
        ] {
            let primary = primary
                .canonicalize()
                .unwrap_or_else(|_| primary.to_path_buf());
            let secondary = secondary
                .canonicalize()
                .unwrap_or_else(|_| secondary.to_path_buf());
            if primary.starts_with(&secondary) || secondary.starts_with(&primary) {
                return Err("Codex 与 Dodex 的数据目录重叠，无法启用双实例支持。".into());
            }
        }
    }
    regular_file(&instance.cli_path)?;
    Ok(())
}
fn config_text(instance: &InstanceConfig) -> String {
    format!(
        "# Independent Dodex profile; sign in separately on first use.\ncli_auth_credentials_store = \"file\"\nsqlite_home = {}\nlog_dir = {}\n",
        toml_string(&instance.database_dir),
        toml_string(&instance.desktop_user_data.join("logs"))
    )
}
fn toml_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}
fn validate_config(path: &Path, instance: &InstanceConfig) -> Result<(), String> {
    let bytes = read_limited(path, 2 * 1024 * 1024)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "Dodex 隔离配置格式无效。")?;
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| "Dodex 隔离配置 TOML 格式无效；未修改配置。")?;
    if document
        .get("cli_auth_credentials_store")
        .and_then(toml_edit::Item::as_str)
        != Some("file")
    {
        return Err(
            "Dodex 配置必须使用 cli_auth_credentials_store = \"file\"；未修改现有配置。".into(),
        );
    }
    fn check(key: &str, value: Option<&str>, instance: &InstanceConfig) -> Result<(), String> {
        let expected = match key {
            "cli_auth_credentials_store" => "file".to_owned(),
            "sqlite_home" => instance.database_dir.to_string_lossy().into_owned(),
            "log_dir" => instance
                .desktop_user_data
                .join("logs")
                .to_string_lossy()
                .into_owned(),
            _ => return Ok(()),
        };
        if value != Some(expected.as_str()) {
            return Err("Dodex 必须使用独立文件凭证、数据库与日志目录；现有配置不兼容。".into());
        }
        Ok(())
    }
    fn walk_value(value: &toml_edit::Value, instance: &InstanceConfig) -> Result<(), String> {
        if let Some(table) = value.as_inline_table() {
            for (key, child) in table.iter() {
                check(key, child.as_str(), instance)?;
                walk_value(child, instance)?;
            }
        } else if let Some(array) = value.as_array() {
            for child in array.iter() {
                walk_value(child, instance)?;
            }
        }
        Ok(())
    }
    fn walk_item(item: &toml_edit::Item, instance: &InstanceConfig) -> Result<(), String> {
        if let Some(table) = item.as_table() {
            for (key, child) in table.iter() {
                check(key, child.as_str(), instance)?;
                walk_item(child, instance)?;
            }
        } else if let Some(tables) = item.as_array_of_tables() {
            for table in tables.iter() {
                for (key, child) in table.iter() {
                    check(key, child.as_str(), instance)?;
                    walk_item(child, instance)?;
                }
            }
        } else if let Some(value) = item.as_value() {
            walk_value(value, instance)?;
        }
        Ok(())
    }
    walk_item(document.as_item(), instance)
}
fn launcher_text(layout: &Layout, instance: &InstanceConfig) -> String {
    // env -i guarantees inherited credentials, session IDs, Electron and dynamic
    // loader overrides cannot redirect the instance. No original profile is read.
    let fields = [
        ("HOME", layout.user_home.to_string_lossy().into_owned()),
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ("LANG", "en_US.UTF-8".into()),
        (
            "CODEX_HOME",
            instance.codex_home.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_INSTALL_DIR",
            instance
                .codex_home
                .join("bin")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "CODEX_ELECTRON_USER_DATA_PATH",
            instance.desktop_user_data.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_SQLITE_HOME",
            instance.database_dir.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_CLI_PATH",
            instance.cli_path.to_string_lossy().into_owned(),
        ),
        ("CODEX_APP_SERVER_FORCE_CLI", "1".into()),
        ("CODEX_APP_SERVER_USE_LOCAL_DAEMON", "0".into()),
        ("CODEX_SPARKLE_ENABLED", "false".into()),
    ];
    let env = fields
        .iter()
        .map(|(key, value)| shell_quote(&format!("{key}={value}")))
        .collect::<Vec<_>>()
        .join(" \\\n  ");
    format!(
        "#!/bin/sh\n# Agent Companion isolated Dodex launcher, schema 1.\nexec /usr/bin/env -i \\\n  {env} \\\n  {} {} {}\n",
        shell_quote(
            &instance
                .runtime_app
                .join("Contents/MacOS/ChatGPT")
                .to_string_lossy()
        ),
        shell_quote(&format!(
            "--user-data-dir={}",
            instance.desktop_user_data.display()
        )),
        shell_quote(&format!(
            "--disk-cache-dir={}",
            instance.desktop_user_data.join("Cache").display()
        ))
    )
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
const LAUNCHER_PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>CFBundleIdentifier</key><string>local.agent-companion.dodex</string>\n<key>CFBundleName</key><string>Dodex</string>\n<key>CFBundleDisplayName</key><string>Dodex</string>\n<key>CFBundleExecutable</key><string>Dodex</string>\n<key>CFBundlePackageType</key><string>APPL</string>\n<key>CFBundleVersion</key><string>1</string>\n<key>LSUIElement</key><true/>\n</dict></plist>\n";
fn build_launcher(layout: &Layout, instance: &InstanceConfig, app: &Path) -> Result<(), String> {
    private_directory(&app.join("Contents/MacOS"))?;
    write_new(
        &app.join("Contents/Info.plist"),
        LAUNCHER_PLIST.as_bytes(),
        0o644,
    )?;
    write_new(
        &app.join("Contents/MacOS/Dodex"),
        launcher_text(layout, instance).as_bytes(),
        0o755,
    )
}
fn validate_launcher(layout: &Layout, instance: &InstanceConfig, app: &Path) -> Result<(), String> {
    if read_limited(&app.join("Contents/Info.plist"), 8192)? != LAUNCHER_PLIST.as_bytes()
        || read_limited(&app.join("Contents/MacOS/Dodex"), 32768)?
            != launcher_text(layout, instance).as_bytes()
    {
        return Err("Dodex 启动配置与隔离目录不一致；未修改现有环境。".into());
    }
    if fs::metadata(app.join("Contents/MacOS/Dodex"))
        .map_err(|_| "无法检查 Dodex 启动权限。")?
        .permissions()
        .mode()
        & 0o111
        == 0
    {
        return Err("Dodex 启动程序不可执行。".into());
    }
    Ok(())
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn no_symlinks(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("双开路径必须是规范的绝对路径。".into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("双开路径包含符号链接，无法确认隔离；未修改现有环境。".into());
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("无法访问双开目录，请检查权限。".into()),
        }
    }
    Ok(())
}
fn regular_file(path: &Path) -> Result<(), String> {
    no_symlinks(path)?;
    if !fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Err("双开环境所需文件缺失或不可访问。".into());
    }
    Ok(())
}
fn private_directory(path: &Path) -> Result<(), String> {
    no_symlinks(path)?;
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.is_dir()
    {
        private_directory(parent)?;
    }
    create_private(path)
}
fn create_private(path: &Path) -> Result<(), String> {
    no_symlinks(path)?;
    fs::create_dir(path).map_err(|_| "无法创建双开目录，请检查目录冲突、权限和可用空间。")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护双开目录权限。".into())
}
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    regular_file(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(no_follow_flag())
        .open(path)
        .map_err(|_| "无法读取双开环境配置。")?;
    let mut buffer = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| "无法读取双开环境配置。")?;
    if buffer.len() as u64 > limit {
        return Err("双开环境配置超过允许大小。".into());
    }
    Ok(buffer)
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&read_limited(path, 65536)?).map_err(|_| "双开部署清单格式无效。".into())
}
fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    no_symlinks(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(no_follow_flag())
        .open(path)
        .map_err(|_| "无法写入双开环境；已有文件未被覆盖。")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "写入双开环境失败，请检查权限和可用空间。".into())
}
fn write_new_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    no_symlinks(path)?;
    let stage = path.with_file_name(format!(".completion-stage-{}", transaction_id()));
    let cleanup = OwnedArtifacts {
        paths: vec![stage.clone()],
    };
    write_new(&stage, bytes, mode)?;
    rename_exclusive(&stage, path)?;
    drop(cleanup);
    Ok(())
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    no_symlinks(path)?;
    let stage = path.with_file_name(format!(".dual-instance-{}.json", transaction_id()));
    let cleanup = OwnedArtifacts {
        paths: vec![stage.clone()],
    };
    write_new(&stage, bytes, 0o600)?;
    fs::rename(&stage, path).map_err(|_| "无法保存双开设置。")?;
    drop(cleanup);
    Ok(())
}
fn transaction_id() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
#[derive(Default)]
struct OwnedArtifacts {
    paths: Vec<PathBuf>,
}
impl Drop for OwnedArtifacts {
    fn drop(&mut self) {
        for path in self.paths.iter().rev() {
            if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }
}
struct DeploymentLock(File);
impl DeploymentLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        no_symlinks(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(no_follow_flag())
            .open(path)
            .map_err(|_| "无法锁定部署目录，请检查权限。")?;
        unsafe extern "C" {
            fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
        }
        // Kernel lock is released on crashes. A stale file never blocks retries.
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err("另一进程正在部署 Dodex，请等待完成。".into());
        }
        Ok(Self(file))
    }
}
impl Drop for DeploymentLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
        }
        let _ = unsafe { flock(self.0.as_raw_fd(), 8) };
    }
}
#[cfg(target_os = "macos")]
fn no_follow_flag() -> i32 {
    0x100
}
#[cfg(not(target_os = "macos"))]
fn no_follow_flag() -> i32 {
    0x20000
}
fn rename_exclusive(source: &Path, destination: &Path) -> Result<(), String> {
    no_symlinks(source)?;
    no_symlinks(destination)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from =
            std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| "部署路径无效。")?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "部署路径无效。")?;
        unsafe extern "C" {
            fn renamex_np(
                from: *const std::ffi::c_char,
                to: *const std::ffi::c_char,
                flags: u32,
            ) -> std::ffi::c_int;
        }
        if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) } != 0 {
            return Err("部署目标已存在或不可写；未覆盖现有文件。".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        if exists(destination) {
            return Err("部署目标已存在；未覆盖现有文件。".into());
        }
        fs::rename(source, destination).map_err(|_| "无法完成部署。".into())
    }
}

#[cfg(test)]
#[path = "macos_deployment/tests.rs"]
mod tests;
