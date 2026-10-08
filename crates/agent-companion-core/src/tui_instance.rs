//! Persistent TUI identities. Desktop installations are deliberately absent.
//! The vendor owns `packages/standalone/current` and all daemon/update state.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus},
};

pub const SCHEMA: u32 = 2;
pub const RECORD: &str = "tui-instances.json";
pub const APP_UNSUPPORTED: &str = "Dodex App is no longer supported. Run dodex in a terminal, or dodex resume to continue a session. / 不再支持 Dodex App，请使用 Dodex TUI。";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Standalone,
    Homebrew,
    Npm,
    Native,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceConfig {
    pub id: String,
    pub label: String,
    pub codex_home: PathBuf,
    pub database_dir: PathBuf,
    pub log_dir: PathBuf,
    pub install_dir: PathBuf,
    /// A stable vendor entry (current/bin/codex for the secondary instance).
    pub cli_path: PathBuf,
    pub command_path: PathBuf,
    pub channel: Channel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updater: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    pub schema: u32,
    pub primary: Option<InstanceConfig>,
    pub dodex: InstanceConfig,
    pub dodex_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub user_home: PathBuf,
    pub support: PathBuf,
}

pub fn executable_name() -> &'static str {
    if cfg!(windows) { "codex.exe" } else { "codex" }
}

impl Layout {
    pub fn current() -> io::Result<Self> {
        let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let user_home = std::env::var_os(variable)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("User home is unavailable"))?;
        #[cfg(target_os = "macos")]
        let support = user_home.join("Library/Application Support/AgentCompanion");
        #[cfg(windows)]
        let support = PathBuf::from(
            std::env::var_os("LOCALAPPDATA")
                .ok_or_else(|| io::Error::other("LOCALAPPDATA is unavailable"))?,
        )
        .join("AgentCompanion");
        #[cfg(not(any(target_os = "macos", windows)))]
        let support = user_home.join(".local/share/agent-companion");
        no_redirects(&user_home)?;
        no_redirects(&support)?;
        Ok(Self { user_home, support })
    }

    pub fn record(&self) -> PathBuf {
        self.support.join(RECORD)
    }

    pub fn public_bin(&self) -> PathBuf {
        if cfg!(windows) {
            self.support.join("bin")
        } else {
            self.user_home.join(".local/bin")
        }
    }

    pub fn secondary(&self, home: PathBuf, database: PathBuf, logs: PathBuf) -> InstanceConfig {
        InstanceConfig {
            id: "dodex".into(),
            label: "Dodex".into(),
            install_dir: home.join("native-bin"),
            cli_path: standalone_entry(&home),
            codex_home: home,
            database_dir: database,
            log_dir: logs,
            command_path: self
                .public_bin()
                .join(if cfg!(windows) { "dodex.exe" } else { "dodex" }),
            channel: Channel::Standalone,
            updater: None,
        }
    }

    pub fn read(&self) -> io::Result<Option<Registry>> {
        if !self.record().try_exists()? {
            return Ok(None);
        }
        let registry: Registry = serde_json::from_slice(&read_limited(&self.record(), 64 * 1024)?)
            .map_err(|_| io::Error::other("TUI instance record is invalid"))?;
        self.validate(&registry)?;
        Ok(Some(registry))
    }

    pub fn validate(&self, registry: &Registry) -> io::Result<()> {
        let second = &registry.dodex;
        if registry.schema != SCHEMA
            || second.id != "dodex"
            || second.channel != Channel::Standalone
            || second.cli_path != standalone_entry(&second.codex_home)
            || second.install_dir != second.codex_home.join("native-bin")
            || second.command_path
                != self
                    .public_bin()
                    .join(if cfg!(windows) { "dodex.exe" } else { "dodex" })
        {
            return Err(io::Error::other("Unsupported TUI instance layout"));
        }
        let primary_home = self.user_home.join(".codex");
        for path in [&second.codex_home, &second.database_dir, &second.log_dir] {
            no_redirects(path)?;
            if !path.starts_with(&self.user_home)
                || path == &self.user_home
                || overlaps(path, &primary_home)
                || overlaps(
                    path,
                    &self.user_home.join("Library/Application Support/Codex"),
                )
            {
                return Err(io::Error::other("Codex and Dodex storage paths overlap"));
            }
        }
        no_redirects(&second.codex_home.join("packages"))?;
        no_redirects(&second.codex_home.join("packages/standalone/releases"))?;
        if let Some(primary) = &registry.primary {
            if primary.id != "codex"
                || primary.codex_home != primary_home
                || !primary.cli_path.is_absolute()
                || !primary.install_dir.is_absolute()
                || primary.command_path != self.public_bin().join(executable_name())
                || primary.cli_path == primary.command_path
            {
                return Err(io::Error::other("Invalid primary TUI binding"));
            }
            for path in [
                &primary.database_dir,
                &primary.log_dir,
                &primary.cli_path,
                &primary.install_dir,
            ] {
                if overlaps(path, &second.codex_home)
                    || overlaps(path, &second.database_dir)
                    || overlaps(path, &second.log_dir)
                {
                    return Err(io::Error::other(
                        "Codex and Dodex runtime or storage paths overlap",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn standalone_entry(home: &Path) -> PathBuf {
    home.join("packages/standalone/current/bin")
        .join(executable_name())
}

pub fn overlaps(left: &Path, right: &Path) -> bool {
    path_within(left, right) || path_within(right, left)
}

pub fn path_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        // canonicalize returns a verbatim prefix on Windows. Compare complete
        // components, case-insensitively, rather than mixing the two spellings.
        let key = |p: &Path| {
            p.to_string_lossy()
                .replace('/', "\\")
                .trim_start_matches(r"\\?\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        let path = key(path);
        let root = key(root);
        path == root || path.starts_with(&(root + "\\"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

pub fn no_redirects(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::other("Expected a normalized absolute path"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if redirected(&metadata) => {
                return Err(io::Error::other("Instance storage contains a redirect"));
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

fn redirected(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn read_limited(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    no_redirects(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::other(
            "Instance metadata is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("Instance metadata is too large"));
    }
    Ok(bytes)
}

/// Locks belong to one identity and never guard the other TUI's update.
pub struct InstanceLock(File);
impl InstanceLock {
    pub fn acquire(layout: &Layout, id: &str) -> io::Result<Self> {
        if !matches!(id, "codex" | "dodex") {
            return Err(io::Error::other("Unknown TUI identity"));
        }
        no_redirects(&layout.support)?;
        fs::create_dir_all(&layout.support)?;
        let path = layout.support.join(format!("{id}-tui.lock"));
        no_redirects(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()
            .map_err(|_| io::Error::other(format!("Another {id} operation is in progress")))?;
        Ok(Self(file))
    }
}
impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl InstanceConfig {
    pub fn runtime(&self) -> io::Result<PathBuf> {
        let native = if self.channel == Channel::Npm {
            npm_native(&self.cli_path)
                .ok_or_else(|| io::Error::other("The original npm Codex package is unavailable"))?
        } else {
            self.cli_path.canonicalize()?
        };
        if self.id == "dodex" {
            let releases = self.codex_home.join("packages/standalone/releases");
            no_redirects(&releases)?;
            if !path_within(&native, &releases)
                || native.file_name() != Some(executable_name().as_ref())
            {
                return Err(io::Error::other(
                    "Dodex current points outside its standalone releases",
                ));
            }
            let package = native
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| io::Error::other("Invalid native package entry"))?;
            package_version(package)?;
        }
        if !is_native(&native) {
            return Err(io::Error::other("TUI entry is not a native executable"));
        }
        Ok(native)
    }

    pub fn environment(&self, command: &mut Command) {
        crate::process_environment::isolate_command(command);
        command
            .env("CODEX_HOME", &self.codex_home)
            .env("CODEX_SQLITE_HOME", &self.database_dir)
            .env("CODEX_INSTALL_DIR", &self.install_dir);
        if self.channel == Channel::Npm {
            command.env("CODEX_MANAGED_BY_NPM", "1");
        }
        // No daemon-disable switches or Electron variables. The installed CLI
        // chooses its native socket, PID, locks and daemon-owned package cache.
    }

    pub fn command(&self, arguments: &[OsString]) -> io::Result<Command> {
        if self.id == "dodex" && crate::codex_args::is_app_command(arguments) {
            return Err(io::Error::other(APP_UNSUPPORTED));
        }
        #[cfg(feature = "config-edit")]
        if self.id == "dodex" {
            crate::install::profile_sync::validate_isolated_overrides(
                &crate::codex_args::config_overrides(arguments),
            )?;
        }
        let mut command = Command::new(self.runtime()?);
        self.environment(&mut command);
        if self.id == "dodex" {
            for setting in [
                "cli_auth_credentials_store=\"file\"".to_owned(),
                format!(
                    "sqlite_home={}",
                    serde_json::to_string(&self.database_dir.to_string_lossy())?
                ),
                format!(
                    "log_dir={}",
                    serde_json::to_string(&self.log_dir.to_string_lossy())?
                ),
            ] {
                command.arg("-c").arg(setting);
            }
        }
        command.args(arguments);
        if crate::codex_args::is_update_command(arguments) {
            // The installer finds its own visible codex and sees PATH already
            // configured. It never mistakes the primary Homebrew entry for an
            // installation to migrate or adds the private prefix to shell rc.
            let path = std::env::join_paths(std::iter::once(self.install_dir.clone()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .map_err(io::Error::other)?;
            command.env("PATH", path);
        }
        Ok(command)
    }
}

pub fn is_native(path: &Path) -> bool {
    let mut magic = [0; 4];
    let valid = File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && (magic[..2] == *b"MZ"
            || magic == [0x7f, b'E', b'L', b'F']
            || matches!(
                magic,
                [0xcf, 0xfa, 0xed, 0xfe]
                    | [0xfe, 0xed, 0xfa, 0xcf]
                    | [0xce, 0xfa, 0xed, 0xfe]
                    | [0xfe, 0xed, 0xfa, 0xce]
                    | [0xca, 0xfe, 0xba, 0xbe]
                    | [0xbe, 0xba, 0xfe, 0xca]
                    | [0xca, 0xfe, 0xba, 0xbf]
                    | [0xbf, 0xba, 0xfe, 0xca]
            ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        valid && fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        valid
    }
}

/// Resolve the official npm package without executing JavaScript. Keep the
/// stable npm entry in the registry and resolve its current vendor on launch.
pub fn npm_native(entry: &Path) -> Option<PathBuf> {
    let resolved = entry.canonicalize().ok()?;
    let package = if resolved.file_name()? == "codex.js" {
        let package = resolved.parent()?.parent()?.to_path_buf();
        if resolved != package.join("bin/codex.js") {
            return None;
        }
        package
    } else if cfg!(windows) && matches!(entry.extension()?.to_str()?, "cmd" | "ps1") {
        entry.parent()?.join("node_modules/@openai/codex")
    } else {
        return None;
    };
    fn manifest(root: &Path) -> Option<serde_json::Value> {
        serde_json::from_slice(&read_limited(&root.join("package.json"), 64 * 1024).ok()?).ok()
    }
    let info = manifest(&package)?;
    if info["name"] != "@openai/codex"
        || !matches!(
            info["bin"]["codex"].as_str(),
            Some("bin/codex.js" | "./bin/codex.js")
        )
    {
        return None;
    }
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        return None;
    };
    let (os, triple) = if cfg!(target_os = "macos") {
        (
            "darwin",
            if arch == "arm64" {
                "aarch64-apple-darwin"
            } else {
                "x86_64-apple-darwin"
            },
        )
    } else if cfg!(windows) {
        (
            "win32",
            if arch == "arm64" {
                "aarch64-pc-windows-msvc"
            } else {
                "x86_64-pc-windows-msvc"
            },
        )
    } else {
        (
            "linux",
            if arch == "arm64" {
                "aarch64-unknown-linux-musl"
            } else {
                "x86_64-unknown-linux-musl"
            },
        )
    };
    let platform = format!("codex-{os}-{arch}");
    for root in [
        package.join("node_modules/@openai").join(&platform),
        package.parent()?.join(&platform),
        package.clone(),
    ] {
        let Some(info) = manifest(&root) else {
            continue;
        };
        if info["name"] != "@openai/codex"
            && info["name"].as_str() != Some(format!("@openai/{platform}").as_str())
        {
            continue;
        }
        for directory in ["bin", "codex"] {
            let native = root
                .join("vendor")
                .join(triple)
                .join(directory)
                .join(executable_name());
            if is_native(&native) {
                return native.canonicalize().ok();
            }
        }
    }
    None
}

pub fn package_version(root: &Path) -> io::Result<String> {
    no_redirects(root)?;
    let value: serde_json::Value =
        serde_json::from_slice(&read_limited(&root.join("codex-package.json"), 64 * 1024)?)?;
    let host = if cfg!(windows) {
        "bin/codex-code-mode-host.exe"
    } else {
        "bin/codex-code-mode-host"
    };
    let rg = if cfg!(windows) {
        "codex-path/rg.exe"
    } else {
        "codex-path/rg"
    };
    for member in [
        format!("bin/{}", executable_name()),
        host.into(),
        "codex-resources".into(),
        rg.into(),
    ] {
        let path = root.join(member);
        no_redirects(&path)?;
        if !path.exists() {
            return Err(io::Error::other("Standalone package is incomplete"));
        }
    }
    if value["layoutVersion"] != 1
        || value["entrypoint"] != format!("bin/{}", executable_name())
        || value["resourcesDir"] != "codex-resources"
        || value["pathDir"] != "codex-path"
        || !root.join("codex-resources").is_dir()
        || !is_native(&root.join("bin").join(executable_name()))
    {
        return Err(io::Error::other("Unsupported standalone package layout"));
    }
    value["version"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("Standalone package version is missing"))
}

/// No job object: an independent native daemon must survive a TUI client exit.
/// Unix exec keeps the PID, terminal handles, exit status and signal semantics.
pub fn run_native(mut command: Command) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, GetConsoleCP, SetConsoleCtrlHandler,
        };
        use windows::core::BOOL;
        unsafe extern "system" fn handler(kind: u32) -> BOOL {
            BOOL::from(kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT)
        }
        // A function handler is not inherited by the child. Both receive Ctrl+C;
        // the child determines its own native exit code, which the shell receives.
        let attached = unsafe { GetConsoleCP() } != 0;
        if attached {
            unsafe { SetConsoleCtrlHandler(Some(handler), true) }.map_err(io::Error::other)?;
        }
        let result = command.status();
        if attached {
            let _ = unsafe { SetConsoleCtrlHandler(Some(handler), false) };
        }
        result
    }
    #[cfg(not(any(unix, windows)))]
    {
        command.status()
    }
}

#[cfg(test)]
#[path = "tui_instance/tests.rs"]
mod tests;
