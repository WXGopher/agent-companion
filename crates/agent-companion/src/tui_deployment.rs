//! Install and repair an independent standalone TUI; never deploy a desktop app.
pub use agent_companion_core::tui_instance::InstanceConfig;
use agent_companion_core::{
    install::profile_sync::{self, Direction, FileKind, IsolationPaths, ProfilePair, SyncOutcome},
    tui_instance::{
        self, Channel, InstanceLock, Layout, Registry, SCHEMA, executable_name, no_redirects,
        read_limited,
    },
};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[cfg(windows)]
#[path = "tui_deployment/windows_retirement.rs"]
mod windows_retirement;

#[cfg(target_os = "macos")]
#[path = "tui_deployment/shell.rs"]
mod shell;

const READY: &str = "Dodex TUI 已就绪。打开终端运行 dodex；首次使用请自行登录副账号。";
const PENDING: &str = "tui-install-pending.json";
const OWNER: &str = "tui-command-ownership.json";

#[derive(Clone, Debug, Default)]
pub struct DeploymentStatus {
    pub enabled: bool,
    pub deployed: bool,
    pub busy: bool,
    pub phase: String,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct SettingsPresentation {
    pub command_path: Option<PathBuf>,
    pub package_path: Option<PathBuf>,
    pub profile_home: Option<PathBuf>,
    pub database_dir: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub tui_available: bool,
    pub tui_configured: bool,
}

static PROGRESS: OnceLock<Mutex<Option<DeploymentStatus>>> = OnceLock::new();
fn progress_slot() -> &'static Mutex<Option<DeploymentStatus>> {
    PROGRESS.get_or_init(Default::default)
}
fn progress(phase: &str, message: &str) {
    *progress_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(DeploymentStatus {
        busy: true,
        phase: phase.into(),
        message: message.into(),
        ..Default::default()
    });
}

pub fn primary_home() -> io::Result<PathBuf> {
    Ok(Layout::current()?.user_home.join(".codex"))
}

pub fn status() -> DeploymentStatus {
    if let Some(current) = progress_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return current;
    }
    let result = Layout::current().and_then(|layout| layout.read());
    match result {
        Ok(Some(record)) => {
            let available = record.dodex.runtime().is_ok();
            DeploymentStatus {
                deployed: true,
                enabled: record.dodex_enabled && available,
                busy: false,
                phase: if available { "ready" } else { "failed" }.into(),
                message: if !available {
                    "Dodex TUI 程序包不可用，请修复安装；账号数据仍保留。"
                } else if record.dodex_enabled {
                    READY
                } else {
                    "Dodex TUI 监控已关闭；终端、账号及历史仍可使用。"
                }
                .into(),
            }
        }
        Ok(None) => DeploymentStatus {
            phase: "not_installed".into(),
            message: "安装独立 Dodex TUI，首次使用请自行登录。".into(),
            ..Default::default()
        },
        Err(error) => DeploymentStatus {
            phase: "failed".into(),
            message: error.to_string(),
            ..Default::default()
        },
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn settings_status() -> DeploymentStatus {
    status()
}

pub fn settings_presentation() -> SettingsPresentation {
    let Some(instance) = saved_instance() else {
        return SettingsPresentation::default();
    };
    let runtime = instance.runtime().ok();
    SettingsPresentation {
        command_path: Some(instance.command_path.clone()),
        package_path: runtime
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf),
        profile_home: Some(instance.codex_home.clone()),
        database_dir: Some(instance.database_dir.clone()),
        log_dir: Some(instance.log_dir.clone()),
        tui_available: runtime.is_some(),
        tui_configured: runtime.is_some() && instance.command_path.is_file(),
    }
}

pub fn saved_instance() -> Option<InstanceConfig> {
    Layout::current().ok()?.read().ok()??.dodex.into()
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn active_instance() -> Option<InstanceConfig> {
    let record = Layout::current().ok()?.read().ok()??;
    if !record.dodex_enabled || record.dodex.runtime().is_err() {
        return None;
    }
    Some(record.dodex)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn monitor_instance() -> Option<(InstanceConfig, Option<String>)> {
    let record = Layout::current().ok()?.read().ok()??;
    if !record.dodex_enabled {
        return None;
    }
    let error = record
        .dodex
        .runtime()
        .err()
        .map(|_| "Dodex TUI 程序包不可用，请修复安装。".into());
    Some((record.dodex, error))
}

pub fn settings_config_path() -> Option<PathBuf> {
    saved_instance().map(|instance| instance.codex_home.join("config.toml"))
}

pub fn set_enabled(enabled: bool) -> Result<DeploymentStatus, String> {
    (|| -> io::Result<()> {
        let layout = Layout::current()?;
        let _lock = InstanceLock::acquire(&layout, "dodex")?;
        let mut record = layout
            .read()?
            .ok_or_else(|| io::Error::other("请先安装 Dodex TUI。"))?;
        if enabled {
            record.dodex.runtime()?;
        }
        record.dodex_enabled = enabled;
        write_registry(&layout, &record)
    })()
    .map_err(|e| e.to_string())?;
    Ok(status())
}

pub fn profile_sync_paths() -> Result<ProfilePair, String> {
    let instance = saved_instance().ok_or("请先安装 Dodex TUI，再同步配置。")?;
    sync_pair(&primary_home().map_err(|e| e.to_string())?, &instance).map_err(|e| e.to_string())
}
fn sync_pair(primary: &Path, instance: &InstanceConfig) -> io::Result<ProfilePair> {
    Ok(ProfilePair {
        primary: profile_sync::profile_paths(primary)?,
        secondary: profile_sync::profile_paths(&instance.codex_home)?,
        secondary_isolation: IsolationPaths {
            sqlite_home: instance.database_dir.clone(),
            log_dir: instance.log_dir.clone(),
        },
    })
}
pub fn sync_profile_file(kind: FileKind, direction: Direction) -> Result<SyncOutcome, String> {
    (|| -> io::Result<SyncOutcome> {
        let layout = Layout::current()?;
        let _lock = InstanceLock::acquire(&layout, "dodex")?;
        let instance = layout
            .read()?
            .ok_or_else(|| io::Error::other("Dodex TUI 未安装。"))?
            .dodex;
        profile_sync::sync_file(
            &sync_pair(&layout.user_home.join(".codex"), &instance)?,
            kind,
            direction,
        )
    })()
    .map_err(|e| e.to_string())
}

/// Installation and repair share a recoverable, idempotent publication. Existing
/// profiles stay in place; no auth, database, session, or log file is copied.
pub fn install_and_configure() -> Result<DeploymentStatus, String> {
    let result = (|| -> io::Result<()> {
        let layout = Layout::current()?;
        let _lock = InstanceLock::acquire(&layout, "dodex")?;
        progress("preparing", "正在接入独立 TUI 包并保留副账号目录…");
        install(&layout)
    })();
    *progress_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    result.map_err(|e| e.to_string())?;
    Ok(status())
}

fn install(layout: &Layout) -> io::Result<()> {
    let pending_path = layout.support.join(PENDING);
    let (mut record, legacy_package) = if pending_path.is_file() {
        let record: Registry = serde_json::from_slice(&read_limited(&pending_path, 64 * 1024)?)?;
        layout.validate(&record)?;
        (record, None)
    } else if let Some(record) = layout.read()? {
        (record, None)
    } else {
        migrate_legacy(layout)?
    };
    if record.primary.is_none() {
        record.primary = discover_primary(layout)?;
    }
    layout.validate(&record)?;
    backup_metadata(layout)?;
    private_directory(&record.dodex.codex_home)?;
    private_directory(&record.dodex.database_dir)?;
    private_directory(&record.dodex.log_dir)?;
    configure_profile(&record.dodex)?;
    if record.dodex.runtime().is_err() {
        if let Some(source) = legacy_package {
            progress("copying", "正在接入已有完整 standalone 包…");
            adopt_package(&record.dodex, &source)?;
        } else {
            progress("installing", "正在安装官方完整 standalone 包…");
            install_native(&record.dodex)?;
        }
    }
    record.dodex.runtime()?;
    verify_package(&record.dodex)?;
    publish_private_entry(&record.dodex)?;
    if let Some(primary) = &record.primary
        && primary.channel == Channel::Standalone
    {
        publish_private_entry(primary)?;
    }
    // Journal before changing public commands. Repeating --repair completes
    // publication after interruption, using this exact original profile.
    atomic_json(&pending_path, &record)?;
    progress("commands", "正在配置独立 codex / dodex 终端入口…");
    publish_commands(layout, &record)?;
    #[cfg(target_os = "macos")]
    shell::configure(&layout.user_home, &record.dodex.command_path).map_err(io::Error::other)?;
    #[cfg(windows)]
    crate::cli_install::register_user_path(&layout.public_bin())?;
    write_registry(layout, &record)?;
    #[cfg(windows)]
    windows_retirement::retire(layout, &record)?;
    retire_legacy_records(layout)?;
    fs::remove_file(pending_path)?;
    Ok(())
}

fn configure_profile(instance: &InstanceConfig) -> io::Result<()> {
    let path = instance.codex_home.join("config.toml");
    no_redirects(&path)?;
    let original = if path.exists() {
        read_limited(&path, 4 * 1024 * 1024)?
    } else {
        Vec::new()
    };
    let text =
        std::str::from_utf8(&original).map_err(|_| io::Error::other("Dodex 配置不是 UTF-8。"))?;
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| io::Error::other("Dodex 配置无效；未覆盖配置。"))?;
    for (key, value) in [
        ("cli_auth_credentials_store", "file".to_owned()),
        (
            "sqlite_home",
            instance.database_dir.to_string_lossy().into_owned(),
        ),
        ("log_dir", instance.log_dir.to_string_lossy().into_owned()),
    ] {
        if !document.contains_key(key) {
            document[key] = toml_edit::value(value);
        }
    }
    // Preserve existing spelling and let the shared isolation validator check
    // path identity (including equivalent absolute Windows path spellings).
    // Nothing is written until every existing setting has passed validation.
    profile_sync::validate_isolated_config(
        document.to_string().as_bytes(),
        &IsolationPaths {
            sqlite_home: instance.database_dir.clone(),
            log_dir: instance.log_dir.clone(),
        },
    )?;
    let updated = document.to_string();
    if updated.as_bytes() != original {
        if !original.is_empty() && !path.with_extension("toml.before-tui").exists() {
            atomic_write(&path.with_extension("toml.before-tui"), &original, 0o600)?;
        }
        atomic_write(&path, updated.as_bytes(), 0o600)?;
    }
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct Ownership {
    schema: u32,
    hashes: std::collections::BTreeMap<PathBuf, Vec<String>>,
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn command_is_owned(path: &Path, bytes: &[u8]) -> bool {
    Layout::current()
        .ok()
        .and_then(|layout| {
            let owner: Ownership =
                serde_json::from_slice(&read_limited(&layout.support.join(OWNER), 64 * 1024).ok()?)
                    .ok()?;
            Some(
                owner.schema == 1
                    && owner
                        .hashes
                        .get(path)
                        .is_some_and(|hashes| hashes.contains(&digest(bytes))),
            )
        })
        .unwrap_or(false)
}
#[cfg(not(windows))]
fn packaged_entry_matches(entry: &Path) -> io::Result<bool> {
    let source =
        std::env::current_exe()?.with_file_name(if cfg!(windows) { "dodex.exe" } else { "dodex" });
    Ok(source.is_file()
        && fs::metadata(entry)?.len() == fs::metadata(&source)?.len()
        && digest(&fs::read(entry)?) == digest(&fs::read(source)?))
}

fn vendor_entry_link(entry: &Path, record: &Registry) -> bool {
    let Some(primary) = &record.primary else {
        return false;
    };
    entry == primary.command_path
        && fs::symlink_metadata(entry).is_ok_and(|m| m.file_type().is_symlink())
        && primary
            .runtime()
            .ok()
            .is_some_and(|runtime| entry.canonicalize().ok() == Some(runtime))
}
fn publish_commands(layout: &Layout, record: &Registry) -> io::Result<()> {
    let source =
        std::env::current_exe()?.with_file_name(if cfg!(windows) { "dodex.exe" } else { "dodex" });
    if !tui_instance::is_native(&source) {
        return Err(io::Error::other(
            "打包的 dodex 终端程序缺失，请重新安装 Companion。",
        ));
    }
    #[cfg(windows)]
    let aliases = windows_alias_directories(layout);
    #[cfg(not(windows))]
    let aliases = Vec::new();
    publish_commands_from(layout, record, &fs::read(source)?, &aliases)
}
#[cfg(windows)]
fn windows_alias_directories(layout: &Layout) -> Vec<PathBuf> {
    let mut directories = vec![
        layout.user_home.join(".local/bin"),
        layout.user_home.join(".cargo/bin"),
    ];
    if let Some(roaming) = std::env::var_os("APPDATA") {
        directories.push(PathBuf::from(roaming).join("npm"));
    }
    if let Some(local) = layout.support.parent() {
        directories.push(local.join("Microsoft/WindowsApps"));
    }
    directories
}
fn publish_commands_from(
    layout: &Layout,
    record: &Registry,
    bytes: &[u8],
    alias_directories: &[PathBuf],
) -> io::Result<()> {
    let path = layout.support.join(OWNER);
    let mut owner: Ownership = if path.is_file() {
        serde_json::from_slice(&read_limited(&path, 64 * 1024)?)?
    } else {
        Ownership {
            schema: 1,
            ..Default::default()
        }
    };
    if owner.schema != 1 {
        return Err(io::Error::other("TUI command ownership is invalid"));
    }
    let mut entries: Vec<_> = record
        .primary
        .iter()
        .chain(std::iter::once(&record.dodex))
        .map(|instance| instance.command_path.clone())
        .collect();
    // Older Windows releases could register the console alias in a different
    // existing PATH directory. Upgrade only hash-verified owned entries so an
    // earlier alias cannot keep routing to the removed desktop executable.
    for directory in alias_directories {
        let entry = directory.join("dodex.exe");
        if !entry.is_file() {
            continue;
        }
        let existing = fs::read(&entry)?;
        if owner
            .hashes
            .get(&entry)
            .is_some_and(|hashes| hashes.contains(&digest(&existing)))
            || legacy_entry(layout, &entry, &existing)?
        {
            entries.push(entry);
        }
    }
    let desired = digest(bytes);
    for entry in &entries {
        if vendor_entry_link(entry, record) {
            no_redirects(entry.parent().unwrap())?;
            continue;
        }
        no_redirects(entry)?;
        if entry.exists() {
            let existing = fs::read(entry)?;
            if !owner
                .hashes
                .get(entry)
                .is_some_and(|hashes| hashes.contains(&digest(&existing)))
                && digest(&existing) != desired
                && !crate::cli_install::command_is_owned(entry)
                && !legacy_entry(layout, entry, &existing)?
            {
                return Err(io::Error::other(
                    "Existing TUI command is not owned by Companion; no command was overwritten",
                ));
            }
        }
    }
    // Save both generations before publishing to survive a crash between the
    // entries. Existing identical binaries are skipped (also on Windows).
    for entry in &entries {
        let backup = entry.with_extension("before-tui");
        if vendor_entry_link(entry, record) {
            #[cfg(unix)]
            if fs::symlink_metadata(&backup).is_err() {
                std::os::unix::fs::symlink(fs::read_link(entry)?, &backup)?;
            }
        } else if entry.exists() && !backup.exists() {
            atomic_write(&backup, &fs::read(entry)?, 0o700)?;
        }
        let hashes = owner.hashes.entry(entry.clone()).or_default();
        if !hashes.contains(&desired) {
            hashes.push(desired.clone());
        }
    }
    atomic_json(&path, &owner)?;
    for entry in entries {
        if fs::read(&entry).is_ok_and(|old| old == bytes) {
            continue;
        }
        if vendor_entry_link(&entry, record) {
            atomic_write_inner(&entry, bytes, 0o755, true)?;
        } else {
            atomic_write(&entry, bytes, 0o755)?;
        }
    }
    Ok(())
}

fn legacy_entry(layout: &Layout, entry: &Path, bytes: &[u8]) -> io::Result<bool> {
    #[cfg(windows)]
    if entry.file_stem().is_some_and(|name| name == "dodex") {
        let marker = entry.with_file_name("dodex.agent-companion.json");
        if marker.is_file() {
            let owner: serde_json::Value =
                serde_json::from_slice(&read_limited(&marker, 64 * 1024)?)?;
            return Ok(matches!(owner["schema"].as_u64(), Some(1 | 2))
                && owner["owner"] == "agent-companion/dodex"
                && owner["hashes"].as_array().is_some_and(|hashes| {
                    hashes
                        .iter()
                        .any(|hash| hash.as_str() == Some(&digest(bytes)))
                }));
        }
    }
    let text = String::from_utf8_lossy(bytes);
    if entry.file_stem().is_some_and(|name| name == "dodex")
        && let Some(json) = text
            .lines()
            .find_map(|line| line.strip_prefix("# agent-companion-dodex: "))
    {
        let literal = serde_json::to_string(json)?;
        return Ok([
            include_str!("tui_deployment/legacy-entry-v1.py"),
            include_str!("tui_deployment/legacy-entry-v2.py"),
        ]
        .iter()
        .any(|template| {
            template
                .replace(
                    "# __BINDING_MARKER__",
                    &format!("# agent-companion-dodex: {json}"),
                )
                .replace("__BINDING_JSON__", &literal)
                .as_bytes()
                == bytes
        }));
    }
    if entry.file_stem().is_some_and(|name| name == "codex") {
        // Exact historical wrappers only; never execute an arbitrary shell file.
        fn literal(path: &Path) -> String {
            let value = path.to_string_lossy();
            let quote = if value.contains('\'') && !value.contains('"') {
                '"'
            } else {
                '\''
            };
            format!(
                "{quote}{}{quote}",
                value
                    .replace('\\', "\\\\")
                    .replace(quote, &format!("\\{quote}"))
            )
        }
        let primary = layout.user_home.join(".codex");
        return Ok([
            include_str!("tui_deployment/legacy-primary.py"),
            include_str!("tui_deployment/legacy-primary-v1.py"),
        ]
        .iter()
        .any(|template| {
            template
                .replace("__PRIMARY_HOME__", &literal(&primary))
                .replace(
                    "__TARGET__",
                    &literal(&tui_instance::standalone_entry(&primary)),
                )
                .as_bytes()
                == bytes
        }));
    }
    Ok(false)
}

/// Old desktop fields are read solely to locate already existing storage and a
/// complete standalone package. App-bundled CLI paths are never adopted.
fn migrate_legacy(layout: &Layout) -> io::Result<(Registry, Option<PathBuf>)> {
    // Windows AF_UNIX addresses include the canonical home. Keep new homes
    // short enough for native daemon attachment; legacy storage stays in place.
    let mut home = if cfg!(windows) {
        layout.user_home.join(".dodex")
    } else {
        layout.support.join("Dodex/codex-home")
    };
    let mut database = home.join("sqlite");
    let mut logs = home.join("log");
    let mut source = None;
    let mut enabled = true;
    for file in [
        layout.support.join("dual-instance.json"),
        layout.support.join("Dodex/companion-deployment.json"),
    ] {
        if !file.is_file() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&read_limited(&file, 64 * 1024)?)?;
        if !matches!(value["schema"].as_u64(), Some(1 | 2)) {
            return Err(io::Error::other("Unsupported legacy instance record"));
        }
        if let Some(preference) = value["enabled"].as_bool() {
            enabled = preference;
        }
        if let Some(instance) = value.get("instance") {
            home = json_path(instance, "codex_home")?;
            database = json_path(instance, "database_dir")?;
            logs = instance
                .get("desktop_user_data")
                .and_then(|v| v.as_str())
                .map(|v| PathBuf::from(v).join("logs"))
                .unwrap_or_else(|| home.join("log"));
            source = value
                .pointer("/tui/package")
                .and_then(|v| v.as_str())
                .map(PathBuf::from);
            break;
        }
    }
    let entry = layout
        .public_bin()
        .join(if cfg!(windows) { "dodex.exe" } else { "dodex" });
    #[cfg(not(windows))]
    if entry.is_file()
        && !packaged_entry_matches(&entry)?
        && !crate::cli_install::command_is_owned(&entry)
    {
        let bytes = read_limited(&entry, 128 * 1024)?;
        if !legacy_entry(layout, &entry, &bytes)? {
            return Err(io::Error::other(
                "Unknown existing Dodex entry; installation was not changed",
            ));
        }
        if let Some(json) = String::from_utf8_lossy(&bytes)
            .lines()
            .find_map(|line| line.strip_prefix("# agent-companion-dodex: "))
        {
            let value: serde_json::Value = serde_json::from_str(json)?;
            let bound_home = json_path(&value, "profile_home")?;
            let bound_db = json_path(&value, "sqlite_home")?;
            if home.exists() && (home != bound_home || database != bound_db) {
                return Err(io::Error::other(
                    "Legacy TUI and saved account paths disagree",
                ));
            }
            home = bound_home;
            database = bound_db;
            logs = json_path(&value, "log_dir")?;
            source = Some(json_path(&value, "package")?);
        }
    }
    #[cfg(windows)]
    let _ = entry;
    if let Some(package) = &source {
        let allowed = tui_instance::path_within(package, &layout.support.join("Tui/packages"))
            || tui_instance::path_within(package, &layout.support.join("Dodex/tui-packages"));
        if !allowed {
            return Err(io::Error::other(
                "Legacy standalone package is outside the managed install directory",
            ));
        }
        tui_instance::package_version(package)?;
    }
    let record = Registry {
        schema: SCHEMA,
        primary: discover_primary(layout)?,
        dodex: layout.secondary(home, database, logs),
        dodex_enabled: enabled,
    };
    layout.validate(&record)?;
    Ok((record, source))
}
fn json_path(value: &serde_json::Value, key: &str) -> io::Result<PathBuf> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("Legacy instance paths are invalid"))
}

pub fn discover_primary(layout: &Layout) -> io::Result<Option<InstanceConfig>> {
    discover_primary_in(layout, &std::env::var_os("PATH").unwrap_or_default())
}

fn discover_primary_in(
    layout: &Layout,
    path: &std::ffi::OsStr,
) -> io::Result<Option<InstanceConfig>> {
    let home = layout.user_home.join(".codex");
    let mut candidates = Vec::new();
    for directory in std::env::split_paths(path) {
        candidates.push(directory.join(executable_name()));
        #[cfg(windows)]
        candidates.push(directory.join("codex.cmd"));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ]);
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs/OpenAI/Codex/bin/codex.exe"));
    }
    candidates.push(tui_instance::standalone_entry(&home));
    for candidate in candidates {
        let npm = tui_instance::npm_native(&candidate);
        let Some(native) = npm.clone().or_else(|| candidate.canonicalize().ok()) else {
            continue;
        };
        if !tui_instance::is_native(&native)
            || tui_instance::path_within(&native, &layout.support)
            || native
                .components()
                .any(|component| component.as_os_str().to_string_lossy().ends_with(".app"))
            || (candidate == layout.public_bin().join(executable_name())
                && !tui_instance::path_within(&native, &home.join("packages/standalone/releases")))
        {
            continue;
        }
        let (channel, updater) = if npm.is_some() {
            let name = if cfg!(windows) { "npm.cmd" } else { "npm" };
            let updater = std::env::split_paths(path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file());
            (Channel::Npm, updater)
        } else if native.starts_with("/opt/homebrew/Caskroom/codex") {
            (
                Channel::Homebrew,
                Some(PathBuf::from("/opt/homebrew/bin/brew")),
            )
        } else if native.starts_with("/usr/local/Caskroom/codex") {
            (
                Channel::Homebrew,
                Some(PathBuf::from("/usr/local/bin/brew")),
            )
        } else if tui_instance::path_within(&native, &home.join("packages/standalone/releases")) {
            (Channel::Standalone, None)
        } else {
            (Channel::Native, None)
        };
        let cli_path = if channel == Channel::Standalone {
            tui_instance::standalone_entry(&home)
        } else {
            candidate
        };
        let database_dir = agent_companion_core::dashboard::database_home(&home);
        return Ok(Some(InstanceConfig {
            id: "codex".into(),
            label: "Codex".into(),
            codex_home: home.clone(),
            database_dir,
            log_dir: home.join("log"),
            install_dir: if channel == Channel::Standalone {
                home.join("native-bin")
            } else {
                cli_path.parent().unwrap().to_path_buf()
            },
            cli_path,
            command_path: layout.public_bin().join(executable_name()),
            channel,
            updater,
        }));
    }
    Ok(None)
}

pub fn primary_instance() -> Option<InstanceConfig> {
    let layout = Layout::current().ok()?;
    layout
        .read()
        .ok()
        .flatten()
        .and_then(|record| record.primary)
        .or_else(|| discover_primary(&layout).ok().flatten())
}

fn backup_metadata(layout: &Layout) -> io::Result<()> {
    let backup = layout.support.join("TuiMigration");
    private_directory(&backup)?;
    for name in [
        "dual-instance.json",
        "Dodex/companion-deployment.json",
        tui_instance::RECORD,
    ] {
        let original = layout.support.join(name);
        let destination = backup.join(format!("{}.before-tui", name.replace('/', "-")));
        if original.is_file() && !destination.exists() {
            atomic_write(&destination, &read_limited(&original, 64 * 1024)?, 0o600)?;
        }
    }
    Ok(())
}

fn retire_legacy_records(layout: &Layout) -> io::Result<()> {
    for name in ["dual-instance.json", "Dodex/companion-deployment.json"] {
        let original = layout.support.join(name);
        if original.is_file() {
            let backup = layout
                .support
                .join("TuiMigration")
                .join(format!("{}.before-tui", name.replace('/', "-")));
            if fs::read(&original)? != fs::read(backup)? {
                return Err(io::Error::other(
                    "Legacy record changed during migration; repeat repair",
                ));
            }
            fs::remove_file(original)?;
        }
    }
    Ok(())
}

fn adopt_package(instance: &InstanceConfig, source: &Path) -> io::Result<()> {
    let version = tui_instance::package_version(source)?;
    semver::Version::parse(&version).map_err(io::Error::other)?;
    let releases = instance.codex_home.join("packages/standalone/releases");
    private_directory(&releases)?;
    let destination = releases.join(&version);
    if !destination.exists() {
        let stage = tempfile::Builder::new()
            .prefix(".adopting-")
            .tempdir_in(&releases)?;
        let package = stage.path().join("package");
        copy_tree(source, &package)?;
        verify_package_at(instance, &package)?;
        fs::rename(package, &destination)?;
    } else if tui_instance::package_version(&destination)? != version {
        return Err(io::Error::other(
            "Existing Dodex release is incomplete; it was not overwritten",
        ));
    }
    verify_package_at(instance, &destination)?;
    #[cfg(unix)]
    {
        let root = releases.parent().unwrap();
        let staged = root.join(format!(".current.companion.{}", std::process::id()));
        if staged.exists() || fs::symlink_metadata(&staged).is_ok() {
            fs::remove_file(&staged)?;
        }
        std::os::unix::fs::symlink(&destination, &staged)?;
        fs::rename(staged, root.join("current"))?;
    }
    #[cfg(windows)]
    {
        // Let the official installer create/retarget its own junction and
        // visible-bin junction. It reuses the complete copied release.
        install_native_release(instance, Some(&version))?;
    }
    Ok(())
}
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    no_redirects(source)?;
    private_directory(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        no_redirects(&path)?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(&path, target)?;
        } else {
            return Err(io::Error::other(
                "Package contains an unsupported special file",
            ));
        }
    }
    Ok(())
}

pub(crate) fn install_native(instance: &InstanceConfig) -> io::Result<()> {
    install_native_release(instance, None)
}
fn install_native_release(instance: &InstanceConfig, release: Option<&str>) -> io::Result<()> {
    #[cfg(not(windows))]
    let (url, name) = ("https://chatgpt.com/codex/install.sh", "install.sh");
    #[cfg(windows)]
    let (url, name) = ("https://chatgpt.com/codex/install.ps1", "install.ps1");
    let temporary = tempfile::tempdir()?;
    let script = temporary.path().join(name);
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .header("User-Agent", "agent-companion-tui-installer")
        .call()
        .map_err(io::Error::other)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .map_err(io::Error::other)?;
    fs::write(&script, bytes)?;
    #[cfg(not(windows))]
    let mut command = Command::new("/bin/sh");
    #[cfg(windows)]
    let mut command = maintenance_powershell_command()?;
    #[cfg(windows)]
    command.args(["-ExecutionPolicy", "Bypass", "-File"]);
    command.arg(script);
    #[cfg(not(windows))]
    if let Some(release) = release {
        command.args(["--release", release]);
    }
    #[cfg(windows)]
    if let Some(release) = release {
        command.args(["-Release", release]);
    }
    instance.environment(&mut command);
    command.env("CODEX_NON_INTERACTIVE", "1");
    #[cfg(not(windows))]
    let installer_path = vec![
        instance.install_dir.clone(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];
    #[cfg(windows)]
    let installer_path = vec![
        instance.install_dir.clone(),
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_default()).join("System32"),
    ];
    let path = std::env::join_paths(installer_path).map_err(io::Error::other)?;
    command.env("PATH", path);
    run_bounded(&mut command, Duration::from_secs(900))?;
    instance.runtime()?;
    Ok(())
}

fn publish_private_entry(instance: &InstanceConfig) -> io::Result<()> {
    #[cfg(unix)]
    {
        private_directory(&instance.install_dir)?;
        for (name, target) in [
            ("codex", instance.cli_path.clone()),
            (
                "codex-code-mode-host",
                instance.cli_path.with_file_name("codex-code-mode-host"),
            ),
        ] {
            let path = instance.install_dir.join(name);
            if fs::read_link(&path).is_ok_and(|link| link == target) {
                continue;
            }
            if fs::symlink_metadata(&path).is_ok() {
                return Err(io::Error::other(
                    "Private native command is not installer-owned",
                ));
            }
            std::os::unix::fs::symlink(target, path)?;
        }
    }
    #[cfg(windows)]
    {
        if fs::symlink_metadata(&instance.install_dir).is_err() {
            let mut command = maintenance_powershell_command()?;
            command.arg("-Command").arg("$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:AC_TUI_PREFIX -Target $env:AC_TUI_BIN | Out-Null")
                .env("AC_TUI_PREFIX", &instance.install_dir)
                .env("AC_TUI_BIN", instance.cli_path.parent().unwrap());
            run_bounded(&mut command, Duration::from_secs(30))?;
        }
        if instance.install_dir.join("codex.exe").canonicalize()? != instance.runtime()? {
            return Err(io::Error::other(
                "Private native command is not installer-owned",
            ));
        }
    }
    Ok(())
}

pub fn verify_package(instance: &InstanceConfig) -> io::Result<()> {
    let native = instance.runtime()?;
    let root = native
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("Invalid package"))?;
    verify_package_at(instance, root)
}

fn verify_package_at(instance: &InstanceConfig, root: &Path) -> io::Result<()> {
    let expected = tui_instance::package_version(root)?;
    semver::Version::parse(&expected).map_err(io::Error::other)?;
    #[cfg(target_os = "macos")]
    for name in ["codex", "codex-code-mode-host"] {
        run_bounded(
            Command::new("/usr/bin/codesign")
                .args([
                    "--verify",
                    "--strict",
                    "-R=anchor apple generic and certificate leaf[subject.OU] = \"2DC432GLL2\"",
                ])
                .arg(root.join("bin").join(name)),
            Duration::from_secs(30),
        )?;
    }
    #[cfg(windows)]
    for name in ["codex.exe", "codex-code-mode-host.exe"] {
        let mut command = maintenance_powershell_command()?;
        command.arg("-Command").arg("$ErrorActionPreference='Stop'; $s=Get-AuthenticodeSignature -LiteralPath $env:AC_TUI_NATIVE; if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'OpenAI') { exit 1 }")
            .env("AC_TUI_NATIVE", root.join("bin").join(name));
        run_bounded(&mut command, Duration::from_secs(30))?;
    }
    let mut command = Command::new(root.join("bin").join(executable_name()));
    instance.environment(&mut command);
    command.arg("--version");
    let actual = run_bounded(&mut command, Duration::from_secs(20))?;
    if String::from_utf8_lossy(&actual).trim() != format!("codex-cli {expected}") {
        return Err(io::Error::other(
            "Standalone package and native executable versions disagree",
        ));
    }
    Ok(())
}

pub fn open_terminal(session: Option<&str>) -> Result<(), String> {
    (|| -> io::Result<()> {
        let instance = saved_instance().ok_or_else(|| io::Error::other("请先安装 Dodex TUI。"))?;
        instance.runtime()?;
        if session.is_some_and(|id| !valid_session(id)) { return Err(io::Error::other("Invalid TUI session ID")); }
        #[cfg(target_os = "macos")]
        {
            let mut request = Command::new("/usr/bin/osascript");
            request.args(["-e", "on run argv\n tell application \"Terminal\"\n activate\n do script (item 1 of argv)\n end tell\nend run"]);
            let mut shell = shell_quote(&instance.command_path.to_string_lossy());
            if let Some(id) = session { shell.push_str(&format!(" resume {}", shell_quote(id))); }
            request.arg(shell);
            run_bounded(&mut request, Duration::from_secs(30))?;
        }
        #[cfg(windows)]
        {
            let mut command = powershell_command()?;
            command.arg("-Command").arg("$ErrorActionPreference='Stop'; if ($env:AC_TUI_SESSION) { Start-Process -FilePath $env:AC_TUI_ENTRY -ArgumentList @('resume', $env:AC_TUI_SESSION) } else { Start-Process -FilePath $env:AC_TUI_ENTRY }")
                .env("AC_TUI_ENTRY", &instance.command_path).env("AC_TUI_SESSION", session.unwrap_or(""));
            run_bounded(&mut command, Duration::from_secs(30))?;
        }
        Ok(())
    })().map_err(|e| e.to_string())
}
fn valid_session(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn powershell_command() -> io::Result<Command> {
    use std::os::windows::process::CommandExt;
    let root = PathBuf::from(
        std::env::var_os("SystemRoot")
            .ok_or_else(|| io::Error::other("SystemRoot is unavailable"))?,
    );
    let mut command = Command::new(root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    command
        .args(["-NoLogo", "-NoProfile", "-NonInteractive"])
        .creation_flags(0x08000000);
    Ok(command)
}

#[cfg(windows)]
fn maintenance_powershell_command() -> io::Result<Command> {
    let mut command = powershell_command()?;
    // Native callers can inherit PowerShell 7 module paths that Windows
    // PowerShell cannot load. Maintenance needs only its own bundled modules.
    // Keep terminal launches on the inherited environment for user modules.
    let modules = Path::new(command.get_program()).with_file_name("Modules");
    command.env("PSModulePath", modules);
    Ok(command)
}

pub fn run_bounded(command: &mut Command, timeout: Duration) -> io::Result<Vec<u8>> {
    let output = tempfile::tempfile()?;
    let errors = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(errors.try_clone()?);
    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            use std::io::{Seek, SeekFrom};
            let mut output = output;
            let mut errors = errors;
            output.seek(SeekFrom::Start(0))?;
            errors.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            output.take(1024 * 1024).read_to_end(&mut bytes)?;
            if status.success() {
                return Ok(bytes);
            }
            let mut diagnostic = String::new();
            errors.take(4096).read_to_string(&mut diagnostic)?;
            return Err(io::Error::other(if diagnostic.trim().is_empty() {
                format!("TUI operation failed with {status}")
            } else {
                diagnostic.trim().to_owned()
            }));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "TUI operation timed out; repeat repair to recover the installation",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    atomic_write_inner(path, bytes, mode, false)
}
fn atomic_write_inner(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    replace_vendor_link: bool,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid destination"))?;
    no_redirects(parent)?;
    if !replace_vendor_link {
        no_redirects(path)?;
    }
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    atomic_write(path, &serde_json::to_vec(value)?, 0o600)
}
fn write_registry(layout: &Layout, record: &Registry) -> io::Result<()> {
    layout.validate(record)?;
    atomic_json(&layout.record(), record)
}
pub fn private_directory(path: &Path) -> io::Result<()> {
    no_redirects(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tui_deployment/tests.rs"]
mod tests;
