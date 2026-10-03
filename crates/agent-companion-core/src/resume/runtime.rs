use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    AccountIdentity, Environment, Provenance, ResumeInspection, Session, config, discovery,
    display_path, valid_id,
};

const MANIFEST: &str = "acomp-resume.json";
const SCHEMA: u32 = 1;
// Audited tag: openai/codex rust-v0.159.3 (file auth save, native writer
// namespace, source SQLite, explicit-ID TUI resume, embedded --no-daemon).
// Re-run scripts/tests/test_resume_native.py before changing this allowlist.
// Windows has link implementations, but remains gated until native acceptance.
const VERSION: &str = "0.159.3";
// config/read materializes this built-in value even when no source layer sets
// an endpoint. Only this exact default was observed for the pinned runtime.
const NATIVE_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/";
static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

struct ResumeLock {
    file: File,
}

impl ResumeLock {
    fn acquire(file: File) -> Result<Self, std::fs::TryLockError> {
        file.try_lock()?;
        Ok(Self { file })
    }
}

impl Drop for ResumeLock {
    fn drop(&mut self) {
        // A concurrent fork can retain the open-file description until exec.
        // Release our lock explicitly instead of waiting for every copy to close.
        let _ = self.file.unlock();
    }
}

/// Authentication material deliberately has neither Debug nor Serialize.
struct Account {
    id: String,
    user_id: String,
    label: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    run_id: String,
    owner_pid: u32,
    child_pid: Option<u32>,
    launch_pending: bool,
    source: Environment,
    account_environment: String,
    account_id: String,
    account_user_id: String,
    auth_path: PathBuf,
    session: Session,
    profile: Option<String>,
    provenance: Provenance,
    links: Vec<PathBuf>,
}

/// A prepared original-context home. Dropping it only unlinks resources inside
/// its own managed directory; the history and credential targets are untouched.
pub struct PreparedResume {
    directory: PathBuf,
    manifest: Manifest,
    run_lock: Option<ResumeLock>,
    launch_lock: Option<ResumeLock>,
    source_auth_handle: same_file::Handle,
    settings_model: String,
    database_home: PathBuf,
    keep: bool,
}

pub fn inspect(
    source: &Environment,
    account: &Environment,
    session: &Session,
    profile: Option<&str>,
) -> Result<ResumeInspection, String> {
    let settings = config::inspect(source, session, profile)?;
    let mut blockers = settings.blockers;
    blockers.extend(session.blockers.clone());
    if source.id != session.environment_id {
        blockers.push("选择的设置来源与历史存储来源不一致。".into());
    }
    if !valid_id(&session.id) {
        blockers.push("会话 ID 无效。".into());
    }
    if !source.home.is_absolute() || !account.home.is_absolute() || !source.executable.is_absolute()
    {
        blockers.push("环境路径必须为绝对路径。".into());
    }
    if !session.rollout_path.is_file() {
        blockers.push("原会话历史存储已缺失。".into());
    }
    let native_version = runtime_version(&source.executable);
    match &native_version {
        Ok(version) if version == VERSION => {}
        Ok(_) => blockers.push(format!("此 Codex 运行版本未验证；首版仅支持 {VERSION}。")),
        Err(reason) => blockers.push(reason.clone()),
    }
    if !cfg!(any(target_os = "macos", windows)) && !cfg!(test) {
        blockers.push("本次发行只支持 macOS 和 Windows。".into());
    }
    if cfg!(windows) {
        blockers.push(
            "Windows 原生接力尚未完成验证；目录连接和认证硬链接已实现，当前禁用启动。".into(),
        );
    }
    let account_info = read_account(&account.home);
    let account_label = account_info
        .as_ref()
        .map(|account| account.label.clone())
        .unwrap_or_else(|_| "账号身份无法确认".into());
    if let Err(reason) = &account_info {
        blockers.push(reason.clone());
    }
    if config::auth_store(&account.home)? != "file" {
        blockers.push("所选账号使用非文件式凭据，首版不修改其登录方式。".into());
    }
    if let (Some(forced), Ok(account)) = (&settings.forced_account, &account_info)
        && forced != &account.id
    {
        blockers.push("来源安全策略限制了其他 ChatGPT 工作区账号；接力已禁用。".into());
    }
    match discovery::writer_busy(&source.home, &session.id) {
        Ok(true) => blockers.push("原会话仍有原生客户端占用，请先退出该会话。".into()),
        Ok(false) => {}
        Err(reason) => blockers.push(reason),
    }
    // Inherited account/runtime variables are removed by isolated_environment.
    let mut details = settings.details;
    details.push(("历史实际位置".into(), display_path(&session.rollout_path)));
    details.push(("历史存储环境".into(), display_path(&source.home)));
    details.push((
        "原始创建来源".into(),
        session
            .creation_source
            .clone()
            .unwrap_or_else(|| "未知；旧元数据不足以证明原始创建账号。".into()),
    ));
    details.push((
        "本次认证文件".into(),
        display_path(&account.home.join("auth.json")),
    ));
    details.push(("原生运行程序".into(), display_path(&source.executable)));
    details.push((
        "原生版本".into(),
        native_version.unwrap_or_else(|_| "未验证".into()),
    ));
    details.push((
        "并发保护".into(),
        display_path(&source.home.join("thread-writer-locks")),
    ));
    details.push(("认证验证".into(), "启动前由原生 account/read 确认加载的工作区账号；无法确认则不启动会话。此检查不等同于实际扣费验收。".into()));
    details.push(("运行覆盖".into(), "CODEX_HOME=受管理目录；sqlite_home=原数据库；file 认证；--no-daemon；明确 session ID 和本次显示模型；保留安全策略。".into()));
    let provenance = Provenance {
        rows: vec![
            (
                "会话历史".into(),
                format!("{} · {}", source.label, session.id),
            ),
            (
                if blockers.is_empty() {
                    "所选额度账号（启动前验证）"
                } else {
                    "所选额度账号（不可启动）"
                }
                .into(),
                format!("{} · {}", account.label, account_label),
            ),
            ("个人指令 / 本地记忆".into(), source.label.clone()),
            ("模型 / 工具配置".into(), source.label.clone()),
            ("项目指令".into(), "当前项目".into()),
        ],
        details,
    };
    blockers.sort();
    blockers.dedup();
    Ok(ResumeInspection {
        provenance,
        blockers,
        account_label,
        account_identity: account_info.ok().map(|account| AccountIdentity {
            workspace_id: account.id,
            user_id: account.user_id,
        }),
    })
}

pub fn prepare(
    source: &Environment,
    account: &Environment,
    session: &Session,
    profile: Option<&str>,
    managed_root: &Path,
) -> Result<PreparedResume, String> {
    prepare_impl(source, account, session, profile, managed_root, None)
}

/// Preserve the exact identity shown by an earlier account picker. A re-login
/// between selection and preparation must cause a fresh choice, not silently
/// reinterpret a slot such as "Dodex" as a different user's credentials.
pub fn prepare_from_inspection(
    source: &Environment,
    account: &Environment,
    session: &Session,
    profile: Option<&str>,
    managed_root: &Path,
    inspection: &ResumeInspection,
) -> Result<PreparedResume, String> {
    let identity = inspection
        .account_identity
        .as_ref()
        .ok_or("所选账号身份未确认，请重新选择账号。")?;
    prepare_impl(
        source,
        account,
        session,
        profile,
        managed_root,
        Some(identity),
    )
}

fn prepare_impl(
    source: &Environment,
    account: &Environment,
    session: &Session,
    profile: Option<&str>,
    managed_root: &Path,
    selected: Option<&AccountIdentity>,
) -> Result<PreparedResume, String> {
    let inspection = inspect(source, account, session, profile)?;
    if let Some(selected) = selected
        && inspection.account_identity.as_ref() != Some(selected)
    {
        return Err("账号登录身份在选择后已改变，请重新选择额度账号。".into());
    }
    if !inspection.blockers.is_empty() {
        return Err(inspection.blockers.join("\n"));
    }
    let settings = config::inspect(source, session, profile)?;
    let account_info = read_account(&account.home)?;
    if selected.is_some_and(|selected| {
        selected.workspace_id != account_info.id || selected.user_id != account_info.user_id
    }) {
        return Err("账号登录身份在选择后已改变，请重新选择额度账号。".into());
    }
    if !managed_root.is_absolute() {
        return Err("接力运行目录必须为绝对路径。".into());
    }
    ensure_real_directory(managed_root)?;
    let managed_root = fs::canonicalize(managed_root).map_err(|_| "无法定位受管理运行目录。")?;
    let source_home = fs::canonicalize(&source.home).map_err(|_| "来源目录不可用。")?;
    let account_home = fs::canonicalize(&account.home).map_err(|_| "认证目录不可用。")?;
    if managed_root.starts_with(&source_home)
        || managed_root.starts_with(&account_home)
        || source_home.starts_with(&managed_root)
        || account_home.starts_with(&managed_root)
    {
        return Err("受管理运行目录不能与原历史或认证目录重叠。".into());
    }
    cleanup_stale(&managed_root)?;
    let locks = managed_root.join("launch-locks");
    ensure_real_directory(&locks)?;
    // This lock serializes acomp launchers until the child exits. The child's
    // own original-home writer lock arbitrates with every native client.
    let launch_lock = ResumeLock::acquire(open_private(
        &locks.join(format!("{}.lock", session.id)),
        false,
    )?)
    .map_err(|_| "另一个 acomp 正在恢复同一会话。")?;
    if discovery::writer_busy(&source.home, &session.id)? {
        return Err("原会话仍被占用，请先退出。".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let run_id = format!(
        "run-{}-{now}-{}",
        std::process::id(),
        NEXT_RUN.fetch_add(1, Ordering::Relaxed)
    );
    let directory = managed_root.join(&run_id);
    fs::create_dir(&directory).map_err(|_| "无法创建接力运行目录。")?;
    private_permissions(&directory)?;
    let run_lock = ResumeLock::acquire(open_private(&directory.join("run.lock"), true)?)
        .map_err(|_| "无法锁定接力运行目录。")?;
    let auth_path = account_home.join("auth.json");
    let source_auth_handle =
        same_file::Handle::from_path(&auth_path).map_err(|_| "无法检查认证文件标识。")?;
    let mut stored_session = session.clone();
    // Titles can be arbitrary user prompts (including pasted credentials).
    // The run journal needs routing metadata only, never conversation text.
    stored_session.title.clear();
    let mut prepared = PreparedResume {
        directory,
        manifest: Manifest {
            schema: SCHEMA,
            run_id,
            owner_pid: std::process::id(),
            child_pid: None,
            launch_pending: false,
            source: source.clone(),
            account_environment: account.id.clone(),
            account_id: account_info.id,
            account_user_id: account_info.user_id,
            auth_path,
            session: stored_session,
            profile: settings.profile,
            provenance: inspection.provenance,
            links: Vec::new(),
        },
        run_lock: Some(run_lock),
        launch_lock: Some(launch_lock),
        source_auth_handle,
        settings_model: settings.model.ok_or("无法确认最终模型。")?,
        database_home: settings.database_home,
        keep: false,
    };
    prepared.write_manifest()?;
    let home = prepared.home();
    fs::create_dir(&home).map_err(|_| "无法建立受管理 Codex 目录。")?;
    private_permissions(&home)?;
    // These must exist in the source, so new history, native lock files, and
    // memory files cannot accidentally land in a disposable directory.
    for name in [
        "sessions",
        "archived_sessions",
        "memories",
        "thread-writer-locks",
        "attachments",
        "assets",
        "skills",
        "plugins",
        "shell_snapshots",
        "cache",
        "tui-thread-reference-capabilities",
    ] {
        fs::create_dir_all(source_home.join(name)).map_err(|_| "无法准备原环境资源目录。")?;
    }
    for name in ["session_index.jsonl", "history.jsonl"] {
        if !source_home.join(name).exists() {
            open_private(&source_home.join(name), true)?;
        }
    }
    open_private(
        &source_home.join("thread-writer-locks/.coordination.lock"),
        false,
    )?;
    for entry in fs::read_dir(&source_home).map_err(|_| "无法读取来源资源目录。")? {
        let entry = entry.map_err(|_| "无法检查来源资源。")?;
        let name = entry.file_name();
        if isolated_entry(&name.to_string_lossy()) {
            continue;
        }
        let destination = home.join(&name);
        link_resource(&entry.path(), &destination)?;
        prepared.manifest.links.push(PathBuf::from(name));
        prepared.write_manifest()?;
    }
    link_auth(&prepared.manifest.auth_path, &home.join("auth.json"))?;
    prepared.manifest.links.push(PathBuf::from("auth.json"));
    prepared.write_manifest()?;
    prepared.verify_auth_link()?;
    // Actual native identity, effective config and thread existence are checked
    // before any interactive resume. This can refresh the selected auth in place.
    // A parent killed during preflight must not let recovery unlink a live
    // native validator's home before that subprocess has exited.
    prepared.manifest.launch_pending = true;
    prepared.write_manifest()?;
    let preflight = prepared.preflight();
    prepared.manifest.launch_pending = false;
    prepared.write_manifest()?;
    preflight?;
    prepared.verify_auth_link()?;
    Ok(prepared)
}

impl PreparedResume {
    pub fn runtime_home(&self) -> PathBuf {
        self.home()
    }
    pub fn provenance(&self) -> &Provenance {
        &self.manifest.provenance
    }
    fn home(&self) -> PathBuf {
        self.directory.join("home")
    }
    fn write_manifest(&self) -> Result<(), String> {
        let path = self.directory.join(MANIFEST);
        let mut file = open_private(&path, false)?;
        file.set_len(0).map_err(|_| "无法更新接力清单。")?;
        serde_json::to_writer_pretty(&mut file, &self.manifest)
            .map_err(|_| "无法写入接力清单。")?;
        file.flush().map_err(|_| "无法保存接力清单。".into())
    }
    fn command(&self, app_server: bool) -> Command {
        let mut command = Command::new(&self.manifest.source.executable);
        command.current_dir(&self.manifest.session.cwd);
        isolated_environment(&mut command, &self.home());
        command.env("CODEX_SQLITE_HOME", &self.database_home);
        for value in [
            format!(
                "sqlite_home={}",
                toml_string(&self.database_home.to_string_lossy())
            ),
            "cli_auth_credentials_store=\"file\"".into(),
            "model_provider=\"openai\"".into(),
            format!("model={}", toml_string(&self.settings_model)),
        ] {
            command.arg("-c").arg(value);
        }
        if let Some(profile) = &self.manifest.profile {
            command.arg("--profile").arg(profile);
        }
        if app_server {
            command.args(["app-server", "--listen", "stdio://", "--strict-config"]);
        } else {
            command
                .args(["--no-daemon", "--strict-config", "resume"])
                .arg(&self.manifest.session.id);
        }
        command
    }
    fn verify_auth_link(&self) -> Result<(), String> {
        let current = same_file::Handle::from_path(&self.manifest.auth_path)
            .map_err(|_| "所选账号凭据已移除；接力已停止。")?;
        if current != self.source_auth_handle
            || !same_file::is_same_file(&self.manifest.auth_path, self.home().join("auth.json"))
                .unwrap_or(false)
        {
            return Err("所选账号重新登录或替换了认证文件；认证链接失效，请重新选择账号。".into());
        }
        let account = read_account(self.manifest.auth_path.parent().ok_or("认证路径无效。")?)?;
        if account.id != self.manifest.account_id
            || account.user_id != self.manifest.account_user_id
        {
            return Err("所选账号的登录身份已改变，接力已停止。".into());
        }
        Ok(())
    }
    fn preflight(&self) -> Result<(), String> {
        let mut rpc = Rpc::start(self.command(true))?;
        rpc.call(1, "initialize", json!({"clientInfo":{"name":"agent-companion-resume","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        rpc.notify("initialized", json!({}))?;
        let config = rpc.call(
            2,
            "config/read",
            json!({"includeLayers":true,"cwd":self.manifest.session.cwd}),
        )?;
        let effective = &config["config"];
        if effective["model"].as_str() != Some(&self.settings_model) {
            return Err("原生最终模型与来源标记不一致，接力已禁用。".into());
        }
        if effective["model_provider"].as_str().unwrap_or("openai") != "openai" {
            return Err("原生最终配置使用其他 provider，不能使用所选订阅额度。".into());
        }
        validate_endpoints(effective, true)?;
        if effective["cli_auth_credentials_store"]
            .as_str()
            .unwrap_or("file")
            != "file"
        {
            return Err("原生最终认证存储不是文件，接力已禁用。".into());
        }
        if let Some(layers) = config["layers"].as_array() {
            for layer in layers {
                let name = &layer["name"];
                let kind = name["type"]
                    .as_str()
                    .or_else(|| name.as_str())
                    .unwrap_or("");
                if layer
                    .get("disabledReason")
                    .is_some_and(|reason| !reason.is_null())
                {
                    continue;
                }
                let layer_config = &layer["config"];
                validate_endpoints(layer_config, false)?;
                if layer_config["model_providers"].get("openai").is_some() {
                    return Err("原生配置层覆盖了 OpenAI 服务地址或认证路由，接力已禁用。".into());
                }
                if !matches!(
                    kind,
                    "system" | "user" | "sessionFlags" | "packagedDefaults"
                ) {
                    return Err("原生配置包含未验证的项目、云端或托管覆盖，无法保证来源。".into());
                }
                if kind == "user" {
                    let expected = self.home().join("config.toml");
                    if name["file"].as_str().map(Path::new) != Some(expected.as_path())
                        || name["profile"].as_str().is_some()
                    {
                        return Err("原生加载了其他用户配置来源，接力已禁用。".into());
                    }
                }
            }
        } else {
            return Err("原生未返回配置层级，无法确认实际配置来源。".into());
        }
        if let Some(forced) = effective["forced_chatgpt_workspace_id"].as_str()
            && forced != self.manifest.account_id
        {
            return Err("原生安全策略要求其他账号，接力已禁用。".into());
        }
        if effective["forced_login_method"]
            .as_str()
            .is_some_and(|method| method != "chatgpt")
        {
            return Err("原生安全策略要求其他登录方式，接力已禁用。".into());
        }
        let account = rpc.call(3, "account/read", json!({"refreshToken":false}))?;
        if account["requiresOpenaiAuth"] != true
            || account["account"]["type"] != "chatgpt"
            || account["workspaceRouting"]["chatgptAccountId"].as_str()
                != Some(&self.manifest.account_id)
        {
            return Err("原生认证未确认所选 ChatGPT 工作区账号，未启动会话。".into());
        }
        let thread = rpc.call(
            4,
            "thread/read",
            json!({"threadId":self.manifest.session.id,"includeTurns":false}),
        )?;
        if thread["thread"]["id"].as_str() != Some(&self.manifest.session.id) {
            return Err("原生运行程序无法确认原会话 ID，未启动会话。".into());
        }
        if !thread["thread"]["cwd"].as_str().is_some_and(|cwd| {
            discovery::same_directory(Path::new(cwd), &self.manifest.session.cwd)
        }) {
            return Err("原生会话当前项目目录已改变，请重新选择会话。".into());
        }
        Ok(())
    }
    pub fn launch(mut self) -> Result<ExitStatus, String> {
        self.verify_auth_link()?;
        if discovery::writer_busy(&self.manifest.source.home, &self.manifest.session.id)? {
            return Err("原会话已被其他客户端打开，请先退出。".into());
        }
        if runtime_version(&self.manifest.source.executable)? != VERSION {
            return Err("运行程序已升级，请重新检查兼容性。".into());
        }
        let mut command = self.command(false);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let _signals = TerminalSignals::install(&mut command)?;
        self.manifest.launch_pending = true;
        self.write_manifest()?;
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                self.manifest.launch_pending = false;
                self.write_manifest()?;
                return Err("无法启动原生 Codex TUI。".into());
            }
        };
        self.manifest.child_pid = Some(child.id());
        self.manifest.launch_pending = false;
        // From this point a crashed parent must not remove the live child's home.
        self.keep = true;
        if self.write_manifest().is_err() {
            let _ = child.kill();
            let _ = child.wait();
            self.keep = false;
            return Err("无法记录原生子进程，已停止接力。".into());
        }
        let mut invalid_reads = 0;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.keep = false;
                    return Err("无法等待原生 TUI 退出。".into());
                }
            }
            match self.verify_auth_link() {
                Ok(()) => invalid_reads = 0,
                Err(reason) => {
                    invalid_reads += 1;
                    // Native refresh truncates then writes; tolerate brief partial
                    // JSON while still detecting replacement/re-login promptly.
                    if invalid_reads >= 3 {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.keep = false;
                        return Err(reason);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        self.keep = false;
        self.manifest.child_pid = None;
        self.write_manifest()?;
        self.verify_auth_link()?;
        Ok(status)
    }
}

fn validate_endpoints(config: &Value, effective: bool) -> Result<(), String> {
    for key in [
        "openai_base_url",
        "chatgpt_base_url",
        "chatgpt_auth_tokens_refresh_url",
    ] {
        let Some(value) = config.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        // The resolved configuration contains native defaults, whereas a raw
        // layer containing the same key is an explicit user/system override.
        if effective && key == "chatgpt_base_url" && value.as_str() == Some(NATIVE_CHATGPT_BASE_URL)
        {
            continue;
        }
        return Err("原生配置覆盖了认证或模型服务地址，无法证明使用所选订阅额度。".into());
    }
    Ok(())
}
impl Drop for PreparedResume {
    fn drop(&mut self) {
        if !self.keep {
            // Windows must close handles before unlinking their files.
            self.run_lock.take();
            let _ = remove_managed(&self.directory);
        }
        self.launch_lock.take();
    }
}

fn read_account(home: &Path) -> Result<Account, String> {
    let path = home.join("auth.json");
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "所选账号没有可用的 auth.json，请先在其原环境登录。")?;
    if !metadata.file_type().is_file() || metadata.len() > 1024 * 1024 {
        return Err("认证文件类型或大小未验证。".into());
    }
    let bytes = fs::read(path).map_err(|_| "无法读取所选账号凭据。")?;
    let auth: Value =
        serde_json::from_slice(&bytes).map_err(|_| "所选账号认证文件无效；未展示凭据内容。")?;
    if auth
        .get("OPENAI_API_KEY")
        .is_some_and(|value| !value.is_null())
        || auth["auth_mode"]
            .as_str()
            .is_some_and(|mode| mode != "chatgpt")
    {
        return Err("首版只支持 ChatGPT 订阅的文件式登录，API key 和其他登录方式不兼容。".into());
    }
    let id = auth["tokens"]["account_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("认证文件缺少可确认的 ChatGPT 账号标识。")?;
    if id.len() > 200 || id.chars().any(char::is_control) {
        return Err("认证账号标识无效。".into());
    }
    for name in ["access_token", "refresh_token", "id_token"] {
        if auth["tokens"][name].as_str().is_none_or(str::is_empty) {
            return Err("ChatGPT 登录凭据不完整。".into());
        }
    }
    let jwt = auth["tokens"]["id_token"].as_str().unwrap();
    let payload = jwt.split('.').nth(1).ok_or("登录身份信息无效。")?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| "登录身份信息无效。")?;
    let claims: Value = serde_json::from_slice(&payload).map_err(|_| "登录身份信息无效。")?;
    if claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .is_some_and(|claimed| claimed != id)
    {
        return Err("登录文件中的账号标识不一致。".into());
    }
    let user_id = claims["sub"]
        .as_str()
        .or_else(|| claims["https://api.openai.com/auth"]["chatgpt_user_id"].as_str())
        .filter(|value| !value.is_empty())
        .ok_or("认证文件缺少可确认的用户身份，不能只按工作区区分额度账号。")?;
    if user_id.len() > 200 || user_id.chars().any(char::is_control) {
        return Err("认证用户标识无效。".into());
    }
    let label = format!("{} · {}", masked_id(user_id), masked_id(id));
    Ok(Account {
        id: id.to_owned(),
        user_id: user_id.to_owned(),
        label,
    })
}
fn masked_id(value: &str) -> String {
    if value.chars().count() > 12 {
        format!(
            "{}…{}",
            value.chars().take(8).collect::<String>(),
            value
                .chars()
                .skip(value.chars().count().saturating_sub(4))
                .collect::<String>()
        )
    } else {
        value.to_owned()
    }
}

fn runtime_version(executable: &Path) -> Result<String, String> {
    let mut file = File::open(executable).map_err(|_| "来源 Codex 原生运行程序缺失。")?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|_| "无法校验来源运行程序。")?;
    // A wrapper may silently reset CODEX_HOME and use the wrong account.
    if !matches!(
        magic,
        [0xcf, 0xfa, 0xed, 0xfe]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0x7f, b'E', b'L', b'F']
    ) && &magic[..2] != b"MZ"
    {
        return Err("来源运行入口不是原生二进制；不能使用会改写账号环境的包装器。".into());
    }
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法检查 Codex 版本。")?;
    let stdout = child.stdout.take().ok_or("无法读取版本。")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let result = stdout.take(8192).read_to_string(&mut text).map(|_| text);
        let _ = tx.send(result);
    });
    let output = rx.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    let output = output
        .map_err(|_| "检查 Codex 版本超时。")?
        .map_err(|_| "无法读取 Codex 版本。")?;
    let version = output
        .trim()
        .strip_prefix("codex-cli ")
        .filter(|text| {
            text.bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
        .ok_or("Codex 版本格式未验证。")?;
    Ok(version.to_owned())
}

fn isolated_environment(command: &mut Command, home: &Path) {
    crate::process_environment::isolate_command(command);
    command.env("CODEX_HOME", home);
}

fn isolated_entry(name: &str) -> bool {
    matches!(
        name,
        "auth.json" | "app-server-daemon" | "app-server-control" | "ipc" | "node_repl"
    )
}
fn toml_string(value: &str) -> String {
    toml_edit::Value::from(value).to_string()
}
fn ensure_real_directory(path: &Path) -> Result<(), String> {
    verify_managed_ancestors(path)?;
    fs::create_dir_all(path).map_err(|_| "无法创建受管理目录。")?;
    private_permissions(path)
}
fn private_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法限制接力目录权限。")?;
    }
    let _ = path;
    Ok(())
}
fn open_private(path: &Path, new: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    if new {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|_| "无法打开受管理清单或锁文件。".into())
}
fn link_auth(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination).map_err(|_| "无法建立认证符号链接。")?;
    }
    #[cfg(windows)]
    {
        fs::hard_link(source, destination)
            .map_err(|_| "无法建立认证硬链接；Windows 要求同卷文件且无需管理员权限。")?;
    }
    if !same_file::is_same_file(source, destination).unwrap_or(false) {
        return Err("无法证明认证刷新会写回所选账号。".into());
    }
    Ok(())
}
fn link_resource(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)
            .map_err(|_| "无法连接来源配置或历史资源。")?;
    }
    #[cfg(windows)]
    {
        if source.is_dir() {
            junction::create(source, destination).map_err(|_| "无法创建原环境目录连接。")?;
        } else if source.is_file() {
            fs::hard_link(source, destination)
                .map_err(|_| "无法连接原环境文件；Windows 文件必须在同卷。")?;
        } else {
            return Err("来源资源类型未验证。".into());
        }
    }
    Ok(())
}

/// Recover only directories carrying our schema, owned paths and no live parent,
/// child, launcher or native writer. A corrupt/unknown manifest is left intact.
pub fn cleanup_stale(managed_root: &Path) -> Result<usize, String> {
    verify_managed_ancestors(managed_root)?;
    let entries = match fs::read_dir(managed_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err("无法检查旧接力运行目录。".into()),
    };
    let mut cleaned = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("run-")
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let manifest_path = path.join(MANIFEST);
        if !fs::metadata(&manifest_path).is_ok_and(|meta| meta.len() < 1024 * 1024) {
            continue;
        }
        let Ok(bytes) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
            continue;
        };
        if manifest.schema != SCHEMA
            || path.file_name().and_then(|name| name.to_str()) != Some(&manifest.run_id)
            || !valid_id(&manifest.session.id)
        {
            continue;
        }
        if manifest.launch_pending
            || process_alive(manifest.owner_pid)
            || manifest.child_pid.is_some_and(process_alive)
        {
            continue;
        }
        let Ok(lock) = open_private(&path.join("run.lock"), false) else {
            continue;
        };
        let Ok(lock) = ResumeLock::acquire(lock) else {
            continue;
        };
        if discovery::writer_busy(&manifest.source.home, &manifest.session.id) != Ok(false) {
            continue;
        }
        drop(lock);
        remove_managed(&path)?;
        cleaned += 1;
    }
    Ok(cleaned)
}
fn remove_managed(path: &Path) -> Result<(), String> {
    verify_managed_ancestors(path)?;
    let home = path.join("home");
    if let Ok(meta) = fs::symlink_metadata(&home)
        && redirected(&meta)
    {
        return Err("受管理 home 已被重定向；保留该目录以避免删除原文件。".into());
    }
    // remove_dir_all does not follow Unix symlinks. Windows junctions are
    // explicitly unlinked first; never ask a recursive remover to traverse one.
    #[cfg(windows)]
    {
        if let Ok(entries) = fs::read_dir(&home) {
            for entry in entries.flatten() {
                if junction::exists(entry.path()).unwrap_or(false) {
                    junction::delete(entry.path()).map_err(|_| "无法清理受管理目录连接。")?;
                }
            }
        }
    }
    fs::remove_dir_all(path).map_err(|_| "无法清理接力运行目录；原历史和认证文件未删除。".into())
}
fn verify_managed_ancestors(path: &Path) -> Result<(), String> {
    // macOS /tmp and /var are operating-system aliases. Canonicalize only the
    // preexisting ancestor supplied by the caller; once inside a managed run,
    // no resource-redirection component is accepted for cleanup.
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            #[cfg(unix)]
            if matches!(ancestor.to_str(), Some("/tmp" | "/var" | "/etc")) {
                continue;
            }
            if redirected(&meta) || !meta.is_dir() {
                return Err("受管理目录包含重定向或非目录文件。".into());
            }
        }
    }
    Ok(())
}
fn redirected(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        if pid > libc::pid_t::MAX as u32 {
            return true;
        }
        unsafe {
            libc::kill(pid as libc::pid_t, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => handle,
                Err(error) => return error.code().0 != 0x80070057u32 as i32,
            };
            let mut code = 0;
            let running = GetExitCodeProcess(handle, &mut code).is_err() || code == 259;
            let _ = CloseHandle(handle);
            running
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

struct Rpc {
    child: Child,
    input: std::process::ChildStdin,
    messages: mpsc::Receiver<Value>,
}
impl Rpc {
    fn start(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "无法启动原生验证进程。")?;
        let input = child.stdin.take().ok_or("无法连接原生验证进程。")?;
        let output = child.stdout.take().ok_or("无法读取原生验证结果。")?;
        let (sender, messages) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                match reader
                    .by_ref()
                    .take(4 * 1024 * 1024)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.last() != Some(&b'\n') => break,
                    Ok(_) => {
                        if let Ok(value) = serde_json::from_slice::<Value>(&line)
                            && sender.send(value).is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            messages,
        })
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"method":method,"params":params}))
    }
    fn send(&mut self, request: Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.input, &request).map_err(|_| "无法发送原生验证请求。")?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|_| "无法发送原生验证请求。".into())
    }
    fn call(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let value = self
                .messages
                .recv_timeout(remaining)
                .map_err(|_| format!("原生验证 {} 未完成；未启动会话。", method))?;
            if value["id"].as_u64() != Some(id) {
                continue;
            }
            if value.get("error").is_some() {
                return Err(format!(
                    "原生验证 {} 失败；未记录可能包含凭据的原始错误。",
                    method
                ));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| "原生验证结果格式无效。".into());
        }
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
struct TerminalSignals {
    interrupt: libc::sighandler_t,
    quit: libc::sighandler_t,
}
#[cfg(unix)]
impl TerminalSignals {
    fn install(command: &mut Command) -> Result<Self, String> {
        use std::os::unix::process::CommandExt;
        unsafe {
            let interrupt = libc::signal(libc::SIGINT, libc::SIG_IGN);
            let quit = libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                Ok(())
            });
            Ok(Self { interrupt, quit })
        }
    }
}
#[cfg(unix)]
impl Drop for TerminalSignals {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.interrupt);
            libc::signal(libc::SIGQUIT, self.quit);
        }
    }
}
#[cfg(not(unix))]
struct TerminalSignals;
#[cfg(not(unix))]
impl TerminalSignals {
    fn install(_: &mut Command) -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const ID: &str = "01999999-0000-7000-8000-000000000001";
    #[test]
    fn endpoint_preflight_allows_only_the_pinned_resolved_native_default() {
        // Shape observed from the real 0.159.3 config/read response with only
        // model and file-auth settings in the source configuration.
        let effective = json!({
            "model_provider": "openai",
            "openai_base_url": null,
            "chatgpt_base_url": "https://chatgpt.com/backend-api/"
        });
        validate_endpoints(&effective, true).unwrap();
        validate_endpoints(&json!({"model": "gpt-5.2"}), false).unwrap();
        // Even the default URL is disallowed when explicitly supplied by a
        // source layer; only native materialization is exempted.
        assert!(validate_endpoints(&effective, false).is_err());
        for endpoint in [
            "https://chatgpt.com/backend-api",
            "https://chatgpt.com/backend-api/custom",
            "https://chatgpt.com@custom.invalid/backend-api/",
        ] {
            assert!(validate_endpoints(&json!({"chatgpt_base_url": endpoint}), true).is_err());
        }
        for key in ["openai_base_url", "chatgpt_auth_tokens_refresh_url"] {
            assert!(validate_endpoints(&json!({key: NATIVE_CHATGPT_BASE_URL}), true).is_err());
        }
        assert!(validate_endpoints(&json!({"chatgpt_base_url": false}), true).is_err());
    }
    fn auth(workspace: &str, user: &str, secret: &str) -> String {
        let claims =
            json!({"sub":user,"https://api.openai.com/auth":{"chatgpt_account_id":workspace}});
        let jwt = format!(
            "header.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        json!({"auth_mode":"chatgpt","tokens":{"account_id":workspace,"access_token":secret,"refresh_token":"refresh-private-secret","id_token":jwt}}).to_string()
    }
    fn fixture() -> (TempDir, PreparedResume) {
        let root = TempDir::new().unwrap();
        let canonical = fs::canonicalize(root.path()).unwrap();
        let source = canonical.join("source");
        let account = canonical.join("selected");
        let directory = canonical.join("managed/run-fixture");
        for path in [
            &source,
            &account,
            &directory.join("home"),
            &source.join("sessions"),
            &source.join("thread-writer-locks"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(
            source.join("config.toml"),
            "model='gpt-5'\ncli_auth_credentials_store='file'\n",
        )
        .unwrap();
        fs::write(
            source.join("auth.json"),
            auth("source-workspace", "source-user", "source-private-secret"),
        )
        .unwrap();
        fs::write(
            account.join("auth.json"),
            auth(
                "selected-workspace",
                "selected-user",
                "selected-private-secret",
            ),
        )
        .unwrap();
        File::create(source.join("thread-writer-locks/.coordination.lock")).unwrap();
        let rollout = source.join("sessions/rollout-original.jsonl");
        fs::write(&rollout, "original history\n").unwrap();
        for name in ["config.toml", "sessions", "thread-writer-locks"] {
            link_resource(&source.join(name), &directory.join("home").join(name)).unwrap();
        }
        link_auth(
            &account.join("auth.json"),
            &directory.join("home/auth.json"),
        )
        .unwrap();
        let run_lock =
            ResumeLock::acquire(open_private(&directory.join("run.lock"), true).unwrap()).unwrap();
        let prepared = PreparedResume {
            source_auth_handle: same_file::Handle::from_path(account.join("auth.json")).unwrap(),
            manifest: Manifest {
                schema: SCHEMA,
                run_id: "run-fixture".into(),
                owner_pid: std::process::id(),
                child_pid: None,
                launch_pending: false,
                source: Environment {
                    id: "codex".into(),
                    label: "Codex".into(),
                    home: source.clone(),
                    executable: std::env::current_exe().unwrap(),
                    database_home: None,
                },
                account_environment: "dodex".into(),
                account_id: "selected-workspace".into(),
                account_user_id: "selected-user".into(),
                auth_path: account.join("auth.json"),
                session: Session {
                    id: ID.into(),
                    title: String::new(),
                    cwd: canonical.clone(),
                    rollout_path: rollout,
                    environment_id: "codex".into(),
                    modified_secs: 0,
                    creation_source: None,
                    busy: false,
                    blockers: Vec::new(),
                },
                profile: None,
                provenance: Provenance::default(),
                links: vec!["sessions".into(), "auth.json".into()],
            },
            directory,
            run_lock: Some(run_lock),
            launch_lock: None,
            settings_model: "gpt-5".into(),
            database_home: source,
            keep: false,
        };
        prepared.write_manifest().unwrap();
        (root, prepared)
    }
    #[test]
    fn writes_original_history_and_refreshes_only_selected_account_then_cleans_links() {
        let (_root, prepared) = fixture();
        let source = prepared.manifest.source.home.clone();
        let selected = prepared.manifest.auth_path.clone();
        let managed = prepared.directory.clone();
        OpenOptions::new()
            .append(true)
            .open(prepared.home().join("sessions/rollout-original.jsonl"))
            .unwrap()
            .write_all(b"continued same id\n")
            .unwrap();
        fs::write(
            prepared.home().join("auth.json"),
            auth(
                "selected-workspace",
                "selected-user",
                "refreshed-private-secret",
            ),
        )
        .unwrap();
        prepared.verify_auth_link().unwrap();
        let manifest = fs::read_to_string(prepared.directory.join(MANIFEST)).unwrap();
        assert!(!manifest.contains("private-secret"));
        assert_eq!(prepared.manifest.session.id, ID);
        drop(prepared);
        assert!(!managed.exists());
        assert!(
            fs::read_to_string(source.join("sessions/rollout-original.jsonl"))
                .unwrap()
                .contains("continued same id")
        );
        assert!(
            fs::read_to_string(source.join("auth.json"))
                .unwrap()
                .contains("source-private-secret")
        );
        assert!(
            fs::read_to_string(selected)
                .unwrap()
                .contains("refreshed-private-secret")
        );
    }
    #[test]
    fn shares_entire_native_lock_namespace_with_original_clients() {
        let (_root, prepared) = fixture();
        let original = prepared
            .manifest
            .source
            .home
            .join("thread-writer-locks")
            .join(format!("{ID}.lock"));
        let lock = open_private(&original, true).unwrap();
        lock.try_lock().unwrap();
        assert_eq!(discovery::writer_busy(&prepared.home(), ID), Ok(true));
        assert!(
            same_file::is_same_file(
                &original,
                prepared
                    .home()
                    .join("thread-writer-locks")
                    .join(format!("{ID}.lock"))
            )
            .unwrap()
        );
        drop(lock);
        assert_eq!(discovery::writer_busy(&prepared.home(), ID), Ok(false));
    }
    #[test]
    fn detects_relogin_by_different_user_even_in_same_workspace() {
        let (_root, prepared) = fixture();
        let account_home = prepared.manifest.auth_path.parent().unwrap();
        let original_label = read_account(account_home).unwrap().label;
        fs::write(
            &prepared.manifest.auth_path,
            auth(
                "selected-workspace",
                "other-user",
                "different-private-secret",
            ),
        )
        .unwrap();
        let changed_label = read_account(account_home).unwrap().label;
        assert_ne!(original_label, changed_label);
        assert!(original_label.contains("selected…user"));
        assert!(changed_label.contains("other-user"));
        assert!(!changed_label.contains("private-secret"));
        let error = prepared.verify_auth_link().unwrap_err();
        assert!(error.contains("身份已改变"));
        assert!(!error.contains("private-secret"));
    }
    #[test]
    fn preparation_rejects_identity_changed_after_account_picker_without_writes() {
        let (_root, prepared) = fixture();
        let account_home = prepared.manifest.auth_path.parent().unwrap().to_owned();
        let account = Environment {
            id: "dodex".into(),
            label: "Dodex".into(),
            home: account_home,
            executable: prepared.manifest.source.executable.clone(),
            database_home: None,
        };
        let displayed = ResumeInspection {
            provenance: Provenance::default(),
            blockers: Vec::new(),
            account_label: "previous-user".into(),
            account_identity: Some(AccountIdentity {
                workspace_id: "selected-workspace".into(),
                user_id: "previous-user".into(),
            }),
        };
        let managed_root = prepared
            .directory
            .parent()
            .unwrap()
            .join("must-not-be-created");
        let error = prepare_from_inspection(
            &prepared.manifest.source,
            &account,
            &prepared.manifest.session,
            None,
            &managed_root,
            &displayed,
        )
        .err()
        .unwrap();
        assert!(error.contains("选择后已改变"));
        assert!(!managed_root.exists());
        prepared.verify_auth_link().unwrap();
    }
    #[test]
    fn detects_auth_inode_replaced_even_with_same_account() {
        let (_root, prepared) = fixture();
        let replacement = prepared.manifest.auth_path.with_extension("replacement");
        fs::write(
            &replacement,
            auth(
                "selected-workspace",
                "selected-user",
                "new-login-private-secret",
            ),
        )
        .unwrap();
        fs::rename(replacement, &prepared.manifest.auth_path).unwrap();
        assert!(
            prepared
                .verify_auth_link()
                .unwrap_err()
                .contains("链接失效")
        );
    }
    #[test]
    fn launch_command_pins_database_account_and_explicit_id_without_safety_overrides() {
        let (_root, prepared) = fixture();
        let command = prepared.command(false);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--no-daemon".to_owned()));
        assert_eq!(&args[args.len() - 2..], ["resume", ID]);
        assert!(!args.iter().any(|argument| argument.contains("--last")
            || argument.contains("dangerously")
            || argument.contains("sandbox_mode")));
        let env = command.get_envs().collect::<Vec<_>>();
        assert!(env.iter().any(|(key, value)| *key == "CODEX_SQLITE_HOME"
            && value.is_some_and(|value| value == prepared.database_home.as_os_str())));
    }
    #[test]
    fn stale_cleanup_preserves_live_child_and_removes_only_managed_links_after_exit() {
        let (_root, mut prepared) = fixture();
        let managed = prepared.directory.parent().unwrap().to_owned();
        let source = prepared.manifest.source.home.clone();
        prepared.manifest.owner_pid = 0;
        prepared.manifest.child_pid = Some(std::process::id());
        prepared.write_manifest().unwrap();
        prepared.run_lock.take();
        prepared.keep = true;
        assert_eq!(cleanup_stale(&managed).unwrap(), 0);
        prepared.manifest.child_pid = None;
        prepared.write_manifest().unwrap();
        assert_eq!(cleanup_stale(&managed).unwrap(), 1);
        assert!(source.join("sessions/rollout-original.jsonl").is_file());
        assert!(prepared.manifest.auth_path.is_file());
    }
    #[test]
    fn recovery_retains_spawn_window_without_a_recorded_child_pid() {
        let (_root, mut prepared) = fixture();
        let managed = prepared.directory.parent().unwrap().to_owned();
        prepared.manifest.owner_pid = 0;
        prepared.manifest.child_pid = None;
        prepared.manifest.launch_pending = true;
        prepared.write_manifest().unwrap();
        prepared.run_lock.take();
        prepared.keep = true;
        assert_eq!(cleanup_stale(&managed).unwrap(), 0);
        assert!(prepared.home().join("auth.json").is_file());
        prepared.manifest.launch_pending = false;
        prepared.write_manifest().unwrap();
        assert_eq!(cleanup_stale(&managed).unwrap(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn recovery_releases_run_lock_while_an_inherited_descriptor_remains_open() {
        let (_root, mut prepared) = fixture();
        let managed = prepared.directory.parent().unwrap().to_owned();
        // Like fork(), try_clone retains the same open-file description.
        let inherited = prepared
            .run_lock
            .as_ref()
            .unwrap()
            .file
            .try_clone()
            .unwrap();
        prepared.manifest.owner_pid = 0;
        prepared.write_manifest().unwrap();
        prepared.keep = true;
        assert_eq!(cleanup_stale(&managed).unwrap(), 0);
        prepared.run_lock.take();
        assert_eq!(cleanup_stale(&managed).unwrap(), 1);
        assert!(prepared.manifest.auth_path.is_file());
        drop(inherited);
    }
    #[test]
    fn cleanup_rejects_redirected_managed_home_without_reading_its_children() {
        let (_root, mut prepared) = fixture();
        let original = prepared.manifest.source.home.clone();
        prepared.run_lock.take();
        remove_managed(&prepared.directory).unwrap();
        fs::create_dir(&prepared.directory).unwrap();
        link_resource(&original, &prepared.home()).unwrap();
        prepared.keep = true;
        assert!(remove_managed(&prepared.directory).is_err());
        assert!(original.join("sessions/rollout-original.jsonl").is_file());
    }
    #[test]
    fn launch_locks_exclude_a_second_companion_process() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("launch.lock");
        let first = ResumeLock::acquire(open_private(&path, true).unwrap()).unwrap();
        #[cfg(unix)]
        let inherited = first.file.try_clone().unwrap();
        let second = open_private(&path, false).unwrap();
        assert!(second.try_lock().is_err());
        drop(first);
        let _second = ResumeLock::acquire(second).unwrap();
        #[cfg(unix)]
        drop(inherited);
    }
    #[test]
    fn auth_parsing_failures_never_echo_material() {
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("auth.json"),
            "sk-private-secret-broken-json",
        )
        .unwrap();
        let error = read_account(root.path()).err().unwrap();
        assert!(!error.contains("private-secret"));
        fs::write(
            root.path().join("auth.json"),
            json!({"OPENAI_API_KEY":"sk-private-secret"}).to_string(),
        )
        .unwrap();
        assert!(read_account(root.path()).err().unwrap().contains("API key"));
    }
}
