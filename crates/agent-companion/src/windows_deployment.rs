//! Explicitly deployed Windows Dodex runtime and isolated profile. No credentials
//! or personal configuration are copied, and deployment never launches the app.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::Write,
    os::windows::{fs::MetadataExt, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

const MANIFEST: &str = "companion-deployment.json";
const READY: &str = "环境已就绪。打开 Dodex 后，请使用第二个账号登录。";

mod shell;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceConfig {
    pub codex_home: PathBuf,
    pub database_dir: PathBuf,
    pub desktop_user_data: PathBuf,
    pub runtime_app: PathBuf,
    pub cli_path: PathBuf,
}

impl InstanceConfig {
    fn at(root: &Path) -> Self {
        Self {
            codex_home: root.join("codex-home"),
            database_dir: root.join("codex-home/sqlite"),
            desktop_user_data: root.join("desktop-data"),
            runtime_app: root.join("runtime/ChatGPT.exe"),
            cli_path: root.join("runtime/resources/codex.exe"),
        }
    }
}

#[derive(Clone, Default)]
pub struct DeploymentStatus {
    pub enabled: bool,
    pub deployed: bool,
    pub busy: bool,
    pub phase: String,
    pub message: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    instance: InstanceConfig,
    archive_hash: String,
}

#[derive(Serialize, Deserialize)]
struct Preference {
    schema: u32,
    enabled: bool,
}

#[derive(Default)]
struct State {
    status: DeploymentStatus,
    instance: Option<InstanceConfig>,
    initialized: bool,
    stamp: Option<SystemTime>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static OPERATION: Mutex<()> = Mutex::new(());

fn shared() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn current_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("无法定位本地应用目录。")?;
    let path = PathBuf::from(base).join("AgentCompanion/Dodex");
    no_redirects(&path)?;
    Ok(path)
}

pub fn primary_home() -> std::io::Result<PathBuf> {
    crate::util::home_dir()
        .map(|home| home.join(".codex"))
        .ok_or_else(|| std::io::Error::other("user profile unavailable"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    no_redirects(path)?;
    let meta = fs::metadata(path).map_err(|_| "无法读取双开配置。")?;
    if meta.len() > 64 * 1024 {
        return Err("双开配置过大。".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| "无法读取双开配置。")?)
        .map_err(|_| "双开配置无效；未覆盖现有环境。".into())
}

fn save_preference(root: &Path, enabled: bool) -> Result<(), String> {
    let parent = root.parent().ok_or("无效的双开目录。")?;
    let path = parent.join("dual-instance.json");
    no_redirects(parent)?;
    no_redirects(&path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法保存双开设置。")?;
    serde_json::to_writer(&mut temp, &Preference { schema: 1, enabled })
        .map_err(|_| "无法保存双开设置。")?;
    temp.flush().map_err(|_| "无法保存双开设置。")?;
    temp.persist(path).map_err(|_| "无法保存双开设置。")?;
    Ok(())
}

pub fn status() -> DeploymentStatus {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    let layout = current_root().ok();
    let stamp = layout.as_ref().and_then(|root| preference_stamp(root));
    if !state.status.busy && state.stamp != stamp {
        *state = State::default();
    }
    if !state.initialized {
        state.initialized = true;
        state.stamp = stamp;
        state.status.message = "创建独立环境，首次使用需要自行登录。".into();
        if let Some(root) = layout {
            let preference = root.parent().unwrap().join("dual-instance.json");
            if preference.exists() {
                match read_json::<Preference>(&preference) {
                    Ok(saved) if saved.schema == 1 => {
                        state.status.deployed = root.join(MANIFEST).exists();
                        if saved.enabled {
                            state.status.busy = true;
                            std::thread::spawn(|| {
                                let _ = perform(|| {
                                    let root = current_root()?;
                                    let saved: Preference = read_json(
                                        &root.parent().unwrap().join("dual-instance.json"),
                                    )?;
                                    if saved.schema != 1 {
                                        return Err("双开设置版本不兼容。".into());
                                    }
                                    let instance = if saved.enabled {
                                        Some(validate(&root, true)?.instance)
                                    } else {
                                        None
                                    };
                                    Ok((instance, saved.enabled, None))
                                });
                            });
                        } else {
                            state.status.message = "双开支持已停用，Dodex 环境仍保留。".into();
                        }
                    }
                    _ => {
                        state.status.phase = "failed".into();
                        state.status.message = "双开设置无效；未启用第二实例。".into();
                    }
                }
            }
        }
    }
    state.status.clone()
}

fn preference_stamp(root: &Path) -> Option<SystemTime> {
    fs::metadata(root.parent()?.join("dual-instance.json"))
        .ok()?
        .modified()
        .ok()
}

pub fn active_instance() -> Option<InstanceConfig> {
    if !status().enabled {
        return None;
    }
    let validation = current_root().and_then(|root| validate(&root, false));
    if let Err(error) = validation {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.status.enabled = false;
        state.status.phase = "failed".into();
        state.status.message = error;
        state.instance = None;
        return None;
    }
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .instance
        .clone()
}

fn perform(
    action: impl FnOnce() -> Result<(Option<InstanceConfig>, bool, Option<String>), String>,
) -> Result<DeploymentStatus, String> {
    let _guard = OPERATION.try_lock().map_err(|_| "双开操作正在进行。")?;
    {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.initialized = true;
        state.status.busy = true;
        state.status.phase = "verifying".into();
    }
    let result = (|| {
        let root = current_root()?;
        let parent = root.parent().unwrap();
        fs::create_dir_all(parent).map_err(|_| "无法创建双开配置目录。")?;
        let lock_path = parent.join("dual-instance.lock");
        no_redirects(&lock_path)?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|_| "无法锁定双开设置。")?;
        lock.try_lock()
            .map_err(|_| "另一个 Companion 正在修改双开设置，请稍后重试。")?;
        action()
    })();
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.status.busy = false;
    state.stamp = current_root().ok().and_then(|root| preference_stamp(&root));
    match result {
        Ok((instance, enabled, message)) => {
            state.status.enabled = enabled;
            state.status.deployed = instance.is_some() || state.status.deployed;
            state.instance = instance;
            state.status.phase = if enabled { "ready" } else { "disabled" }.into();
            state.status.message = message.unwrap_or_else(|| {
                if enabled {
                    READY
                } else {
                    "双开支持已停用，Dodex 环境仍保留。"
                }
                .into()
            });
            Ok(state.status.clone())
        }
        Err(error) => {
            state.status.enabled = false;
            state.status.deployed = current_root().is_ok_and(|root| root.join(MANIFEST).is_file());
            state.instance = None;
            state.status.phase = "failed".into();
            state.status.message = error.clone();
            Err(error)
        }
    }
}

pub fn set_enabled(enabled: bool) -> Result<DeploymentStatus, String> {
    if enabled {
        return deploy();
    }
    perform(|| {
        let root = current_root()?;
        save_preference(&root, false)?;
        Ok((None, false, None))
    })
}

pub fn deploy() -> Result<DeploymentStatus, String> {
    perform(|| {
        let root = current_root()?;
        let instance =
            ensure_instance(&root, official_runtime, &|path| verify_runtime(path, false))?;
        shared()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .phase = "shell".into();
        let shell = shell::ensure()?;
        save_preference(&root, true)?;
        Ok((
            Some(instance),
            true,
            Some(format!("{READY} {}", shell.message())),
        ))
    })
}

/// Both the checkbox and the deploy/repair button use this path. A valid
/// existing profile is adopted without copying or rewriting any user data.
fn ensure_instance(
    root: &Path,
    source: impl FnOnce() -> Result<PathBuf, String>,
    verify: &impl Fn(&Path) -> Result<String, String>,
) -> Result<InstanceConfig, String> {
    validate_primary_separation(root)?;
    no_redirects(root)?;
    if root.exists() {
        let manifest = validate_with(root, true, verify)?;
        prepare_sandbox_bin(&manifest.instance.codex_home)?;
        return Ok(manifest.instance);
    }
    let source = source()?;
    let parent = root.parent().ok_or("无效的双开目录。")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建双开目录。")?;
    no_redirects(parent)?;
    let stage = tempfile::Builder::new()
        .prefix("dodex-")
        .tempdir_in(parent)
        .map_err(|_| "无法创建部署临时目录。")?;
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status
        .phase = "copying".into();
    copy_tree(&source, &stage.path().join("runtime"))?;
    let instance = InstanceConfig::at(root);
    for directory in [
        "codex-home/sqlite",
        "desktop-data/logs",
        "desktop-data/Cache",
    ] {
        fs::create_dir_all(stage.path().join(directory)).map_err(|_| "无法创建隔离目录。")?;
    }
    prepare_sandbox_bin(&stage.path().join("codex-home"))?;
    fs::write(
        stage.path().join("codex-home/config.toml"),
        configuration(&instance),
    )
    .map_err(|_| "无法创建独立配置。")?;
    let manifest = Manifest {
        schema: 1,
        instance: instance.clone(),
        archive_hash: verify(&stage.path().join("runtime"))?,
    };
    fs::write(
        stage.path().join(MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|_| "无法保存部署清单。")?;
    // Renaming a directory never replaces an existing Windows destination.
    fs::rename(stage.path(), root).map_err(|_| "已有双开环境或部署冲突；未覆盖任何文件。")?;
    Ok(instance)
}

fn configuration(instance: &InstanceConfig) -> String {
    format!(
        "cli_auth_credentials_store = \"file\"\nsqlite_home = {}\nlog_dir = {}\n",
        serde_json::to_string(&instance.database_dir).unwrap(),
        serde_json::to_string(&instance.desktop_user_data.join("logs")).unwrap()
    )
}

fn prepare_sandbox_bin(home: &Path) -> Result<(), String> {
    let directory = home.join(".sandbox-bin");
    no_redirects(&directory)?;
    // Create this as the desktop user before Codex's elevated setup. Otherwise
    // the helper owns it as Administrators and its later unelevated refresh
    // cannot set the protected DACL (helper_sandbox_lock_failed, Windows 5).
    // Leave existing contents and permissions entirely to Codex.
    match fs::create_dir(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {
            Ok(())
        }
        Err(_) => Err("无法准备 Dodex 的 Windows 初始化目录。".into()),
    }
}

fn validate(root: &Path, runtime: bool) -> Result<Manifest, String> {
    validate_with(root, runtime, &|path| verify_runtime(path, false))
}

fn validate_with(
    root: &Path,
    runtime: bool,
    verify: &impl Fn(&Path) -> Result<String, String>,
) -> Result<Manifest, String> {
    no_redirects(root)?;
    let manifest: Manifest = read_json(&root.join(MANIFEST))?;
    if manifest.schema != 1 || manifest.instance != InstanceConfig::at(root) {
        return Err("双开目录与部署清单不一致。".into());
    }
    let instance = &manifest.instance;
    for path in [
        &instance.codex_home,
        &instance.database_dir,
        &instance.desktop_user_data,
        &instance.runtime_app,
        &instance.cli_path,
        &instance.codex_home.join("auth.json"),
        &instance.codex_home.join("config.toml"),
    ] {
        no_redirects(path)?;
    }
    for entry in fs::read_dir(&instance.database_dir).map_err(|_| "独立数据库目录不可用。")?
    {
        no_redirects(&entry.map_err(|_| "无法检查数据库目录。")?.path())?;
    }
    validate_primary_separation(root)?;
    let config_path = instance.codex_home.join("config.toml");
    if fs::metadata(&config_path)
        .map_err(|_| "独立配置缺失。")?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("独立配置过大。".into());
    }
    let config = fs::read_to_string(config_path).map_err(|_| "独立配置缺失。")?;
    validate_config(&config, instance)?;
    if runtime && verify(&root.join("runtime"))? != manifest.archive_hash {
        return Err("Dodex 运行程序已改变，请检查部署环境。".into());
    }
    Ok(manifest)
}

fn validate_primary_separation(root: &Path) -> Result<(), String> {
    let primary = primary_home().map_err(|_| "无法定位 Codex 目录。")?;
    let primary_database = agent_companion_core::dashboard::database_home(&primary);
    for path in [&primary, &primary_database] {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if paths_overlap(root, path)
            || paths_overlap(
                &root.canonicalize().unwrap_or_else(|_| root.to_owned()),
                &canonical,
            )
        {
            return Err("Codex 和 Dodex 的目录不能重叠。".into());
        }
    }
    Ok(())
}

fn validate_config(config: &str, instance: &InstanceConfig) -> Result<(), String> {
    let document = config
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| "Dodex 配置格式无效。")?;
    let expected = configuration(instance)
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    for key in ["cli_auth_credentials_store", "sqlite_home", "log_dir"] {
        if document.get(key).and_then(toml_edit::Item::as_str)
            != expected.get(key).and_then(toml_edit::Item::as_str)
        {
            return Err("Dodex 必须使用独立文件凭证、数据库与日志目录。".into());
        }
    }
    // Profiles must not override the isolation settings either.
    fn walk(item: &toml_edit::Item, expected: &toml_edit::DocumentMut) -> bool {
        if let Some(table) = item.as_table_like() {
            table.iter().all(|(key, child)| {
                (expected.get(key).is_none()
                    || child.as_str() == expected.get(key).and_then(toml_edit::Item::as_str))
                    && walk(child, expected)
            })
        } else if let Some(tables) = item.as_array_of_tables() {
            tables
                .iter()
                .all(|table| walk(&toml_edit::Item::Table(table.clone()), expected))
        } else {
            true
        }
    }
    if !walk(document.as_item(), &expected) {
        return Err("配置中的覆盖项会破坏实例隔离。".into());
    }
    Ok(())
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        let mut normalized = PathBuf::new();
        for component in p.components() {
            match component {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => (),
                other => normalized.push(other.as_os_str()),
            }
        }
        normalized
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (a, b) = (normalize(a), normalize(b));
    a == b || a.starts_with(&(b.clone() + "\\")) || b.starts_with(&(a + "\\"))
}

fn no_redirects(path: &Path) -> Result<(), String> {
    no_links(path, false)
}

fn no_links(path: &Path, allow_hardlinks: bool) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("双开路径必须是绝对路径且不能包含上级目录。".into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_attributes() & 0x400 != 0 => {
                return Err("双开目录不能包含链接或重定向。".into());
            }
            Ok(meta) if meta.is_file() && !allow_hardlinks => {
                use std::os::windows::io::AsRawHandle;
                use windows::Win32::Foundation::HANDLE;
                use windows::Win32::Storage::FileSystem::{
                    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
                };
                let file = File::open(ancestor).map_err(|_| "无法检查文件隔离。")?;
                let mut info = BY_HANDLE_FILE_INFORMATION::default();
                unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
                    .map_err(|_| "无法检查文件隔离。")?;
                if info.nNumberOfLinks > 1 {
                    return Err("双开文件不能使用硬链接。".into());
                }
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("无法检查双开目录。".into()),
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    // MSIX can hard-link its immutable application files. Copy their bytes into
    // new files; never preserve a link in the deployed runtime or user profile.
    no_links(source, true)?;
    fs::create_dir(target).map_err(|_| "无法创建运行程序目录。")?;
    for entry in fs::read_dir(source).map_err(|_| "无法读取官方运行程序。")? {
        let entry = entry.map_err(|_| "无法读取官方运行程序。")?;
        no_links(&entry.path(), true)?;
        let to = target.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|_| "无法读取运行程序。")?
            .is_dir()
        {
            copy_tree(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to).map_err(|_| "复制运行程序失败。")?;
        }
    }
    Ok(())
}

fn powershell(script: &str, path: Option<&Path>) -> Result<String, String> {
    let system = std::env::var_os("SystemRoot").ok_or("无法定位 Windows。")?;
    let mut command =
        Command::new(PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"]);
    // A PowerShell 7 host's module path points Windows PowerShell at incompatible
    // Security assemblies. Let the system interpreter build its own module path.
    command.env_remove("PSModulePath");
    if let Some(path) = path {
        command.env("COMPANION_RUNTIME_CHECK", path);
    }
    let mut child = command
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法验证官方 Codex 程序。")?;
    // stdin avoids Windows PowerShell's distinct command-line quote parsing.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{script}\n").as_bytes())
        .map_err(|_| "无法验证官方 Codex 程序。")?;
    let output = child
        .wait_with_output()
        .map_err(|_| "无法验证官方 Codex 程序。")?;
    if !output.status.success() {
        return Err("需要签名有效的官方 Codex Windows 应用。".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('\u{feff}')
        .to_owned())
}

fn official_runtime() -> Result<PathBuf, String> {
    let path = powershell(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $p=@(Get-AppxPackage -Name OpenAI.Codex | Where-Object { $_.PackageFamilyName -eq 'OpenAI.Codex_2p2nqsd0c76g0' -and $_.Status -eq 'Ok' }); if ($p.Count -ne 1) { exit 1 }; Write-Output (Join-Path $p[0].InstallLocation 'app')",
        None,
    )?;
    let path = PathBuf::from(path);
    no_redirects(&path)?;
    verify_runtime(&path, true)?;
    Ok(path)
}

pub fn check_runtime() -> Result<String, String> {
    official_runtime().map(|_| "官方 Codex 签名与运行程序检查通过。".into())
}

fn verify_runtime(path: &Path, allow_hardlinks: bool) -> Result<String, String> {
    for file in ["ChatGPT.exe", "resources/codex.exe", "resources/app.asar"] {
        no_links(&path.join(file), allow_hardlinks)?;
    }
    powershell(
        "$ErrorActionPreference='Stop'; $r=$env:COMPANION_RUNTIME_CHECK; $s=Get-AuthenticodeSignature -LiteralPath (Join-Path $r 'ChatGPT.exe'); if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=\"?OpenAI OpCo, LLC\"?,') { exit 1 }; if (!(Test-Path -LiteralPath (Join-Path $r 'resources/codex.exe'))) { exit 1 }; (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $r 'resources/app.asar')).Hash",
        Some(path),
    )
}

/// Start from a small OS environment, never inheriting account/session overrides.
pub fn isolated_command(executable: &Path, home: &Path, database: &Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear();
    for name in [
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
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
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("CODEX_HOME", home)
        .env("CODEX_SQLITE_HOME", database);
    command
}

/// Launch the deployed CLI in the caller's terminal and working directory. The
/// desktop entry point below deliberately has different window/stdio behavior.
pub fn launch_cli(arguments: &[OsString]) -> Result<ExitStatus, String> {
    let manifest = validate(&current_root()?, true)?;
    prepare_sandbox_bin(&manifest.instance.codex_home)?;
    wait_for_cli(&mut cli_command(&manifest.instance, arguments))
        .map_err(|error| format!("无法运行 Dodex CLI：{error}"))
}

fn cli_command(instance: &InstanceConfig, arguments: &[OsString]) -> Command {
    let mut command = isolated_command(
        &instance.cli_path,
        &instance.codex_home,
        &instance.database_dir,
    );
    // Preserve terminal capabilities and locale without inheriting another
    // Codex account/session, app-server routing, or Electron/Node overrides.
    for name in [
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "WT_SESSION",
        "WT_PROFILE_ID",
        "ConEmuANSI",
        "ANSICON",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

fn wait_for_cli(command: &mut Command) -> std::io::Result<ExitStatus> {
    use windows::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, GetConsoleCP, SetConsoleCtrlHandler,
    };
    use windows::core::BOOL;

    unsafe extern "system" fn handle_control(event: u32) -> BOOL {
        // Codex receives the same console event and owns its interpretation.
        // Keep this wrapper alive so the shell waits until Codex really exits.
        BOOL::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }
    struct ConsoleHandler(bool);
    impl Drop for ConsoleHandler {
        fn drop(&mut self) {
            if self.0 {
                let _ = unsafe { SetConsoleCtrlHandler(Some(handle_control), false) };
            }
        }
    }

    // Custom handlers are not inherited by children (unlike the NULL/ignore
    // handler), so Codex's Ctrl+C behavior is unchanged.
    // Redirected noninteractive invocations may have no console at all.
    let attached = unsafe { GetConsoleCP() } != 0;
    if attached {
        unsafe { SetConsoleCtrlHandler(Some(handle_control), true) }?;
    }
    let _handler = ConsoleHandler(attached);
    command.status()
}

pub fn launch(thread: Option<&str>) -> Result<(), String> {
    let root = current_root()?;
    let manifest = validate(&root, true)?;
    let instance = manifest.instance;
    prepare_sandbox_bin(&instance.codex_home)?;
    let mut command = isolated_command(
        &instance.runtime_app,
        &instance.codex_home,
        &instance.database_dir,
    );
    command
        .env("CODEX_ELECTRON_USER_DATA_PATH", &instance.desktop_user_data)
        .env("CODEX_CLI_PATH", &instance.cli_path)
        .env("CODEX_INSTALL_DIR", instance.codex_home.join("bin"))
        .env("CODEX_APP_SERVER_FORCE_CLI", "1")
        .env("CODEX_APP_SERVER_USE_LOCAL_DAEMON", "0")
        .arg(format!(
            "--user-data-dir={}",
            instance.desktop_user_data.display()
        ))
        .arg(format!(
            "--disk-cache-dir={}",
            instance.desktop_user_data.join("Cache").display()
        ))
        .current_dir(&instance.desktop_user_data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(thread) = thread {
        let uri = crate::app::win::codex::thread_uri(thread).ok_or("无效的会话 ID。")?;
        command.arg(uri);
    }
    command
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "无法打开 Dodex。")?;
    Ok(())
}

#[cfg(test)]
mod tests;
