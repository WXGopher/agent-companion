//! Persistent TUI identities. Desktop installations are deliberately absent.
//! The vendor owns `packages/standalone/current` and all daemon/update state.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
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
            if ![&self.user_home, &self.support]
                .iter()
                .any(|root| path_within(path, root) && !path_within(root, path))
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
        // A drive prefix alone (especially \\?\C:) is not a filesystem
        // endpoint. Inspect it only after RootDir completes the drive root.
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if redirected(&metadata) => {
                return Err(io::Error::other("Instance storage contains a redirect"));
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Cannot inspect instance path {}: {error}",
                        current.display()
                    ),
                ));
            }
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
            let paths = crate::install::profile_sync::IsolationPaths {
                sqlite_home: self.database_dir.clone(),
                log_dir: self.log_dir.clone(),
            };
            crate::install::profile_sync::validate_isolated_overrides(
                &crate::codex_args::config_overrides(arguments),
            )?;
            crate::install::profile_sync::validate_isolated_profile(&self.codex_home, &paths)?;
            let profiles = self.validate_named_profiles(arguments, &paths)?;
            if let Some(directory) =
                crate::codex_args::project_directory(arguments, &std::env::current_dir()?)
            {
                let system = crate::install::profile_sync::system_config_path()?;
                let system_layer = read_configuration(&system)?;
                #[cfg(unix)]
                let managed = read_configuration(&system.with_file_name("managed_config.toml"))?;
                // Current native Windows uses ProgramData config/requirements;
                // it does not load a legacy managed_config.toml config layer.
                #[cfg(not(unix))]
                let managed: Option<toml_edit::DocumentMut> = None;
                self.validate_project_layers(
                    arguments,
                    &directory,
                    &profiles,
                    system_layer.as_ref(),
                    managed.as_ref(),
                    &paths,
                )?;
            }
        }
        let mut command = Command::new(self.runtime()?);
        self.environment(&mut command);
        // Isolation lives in the validated user config, never synthetic -c
        // arguments: any CLI override can make native Codex select its embedded
        // server and prevents queue/agents from joining the shared daemon.
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

    fn validate_named_profiles(
        &self,
        arguments: &[OsString],
        paths: &crate::install::profile_sync::IsolationPaths,
    ) -> io::Result<Vec<toml_edit::DocumentMut>> {
        let mut layers = Vec::new();
        for profile in crate::codex_args::named_profiles(arguments) {
            let path = self.codex_home.join(format!("{profile}.config.toml"));
            let Some(document) = read_configuration(&path)? else {
                // The native parser owns missing/unknown profile diagnostics.
                continue;
            };
            self.validate_overlay(&document, paths)?;
            layers.push(document);
        }
        Ok(layers)
    }

    fn validate_overlay(
        &self,
        document: &toml_edit::DocumentMut,
        paths: &crate::install::profile_sync::IsolationPaths,
    ) -> io::Result<()> {
        // Only these top-level fields define the binding. Preserve all other
        // policy/MCP settings and native handling of unsupported project keys.
        let mut isolation = toml_edit::DocumentMut::new();
        for (key, value) in [
            ("cli_auth_credentials_store", "file".to_owned()),
            (
                "sqlite_home",
                self.database_dir.to_string_lossy().into_owned(),
            ),
            ("log_dir", self.log_dir.to_string_lossy().into_owned()),
        ] {
            isolation[key] = document
                .get(key)
                .cloned()
                .unwrap_or_else(|| toml_edit::value(value));
        }
        crate::install::profile_sync::validate_isolated_config(
            isolation.to_string().as_bytes(),
            paths,
        )
    }

    fn validate_project_layers(
        &self,
        arguments: &[OsString],
        directory: &Path,
        profiles: &[toml_edit::DocumentMut],
        system: Option<&toml_edit::DocumentMut>,
        managed: Option<&toml_edit::DocumentMut>,
        paths: &crate::install::profile_sync::IsolationPaths,
    ) -> io::Result<()> {
        if !directory.is_dir() {
            // Keep native errors for a missing or invalid --cd directory.
            return Ok(());
        }
        // Native discovers layers from the requested path. Canonicalization is
        // for trust lookup only; resolving it here loses the original trust key.
        let mut normalized = PathBuf::new();
        for component in directory.components() {
            match component {
                Component::CurDir => (),
                Component::ParentDir => {
                    normalized.pop();
                }
                _ => normalized.push(component),
            }
        }
        let directory = normalized;
        let mut discovery = ProjectDiscovery::default();
        let mut effective = toml_edit::DocumentMut::new();
        if let Some(system) = system {
            discovery.apply(system);
            copy_isolation(system, &mut effective);
        }
        if !crate::codex_args::ignores_user_config(arguments) {
            let base = read_configuration(&self.codex_home.join("config.toml"))?;
            for layer in base.as_ref().into_iter().chain(profiles) {
                discovery.apply(layer);
                copy_isolation(layer, &mut effective);
            }
        }
        for raw in crate::codex_args::config_overrides(arguments) {
            let parsed = raw.parse::<toml_edit::DocumentMut>().or_else(|_| {
                let (key, value) = raw.split_once('=').unwrap_or((&raw, ""));
                format!("{key}={}", toml_edit::Value::from(value)).parse()
            });
            if let Ok(layer) = parsed {
                discovery.apply(&layer);
            }
        }
        if let Some(managed) = managed {
            discovery.apply(managed);
        }
        let project_root = directory
            .ancestors()
            .find(|ancestor| {
                discovery.markers.iter().any(|marker| {
                    let path = ancestor.join(marker);
                    path.exists()
                        && (marker != ".git" || !path.is_dir() || path.join("HEAD").exists())
                })
            })
            .unwrap_or(&directory);
        let repository = repository_trust_root(&directory);
        let mut directories = Vec::new();
        for ancestor in directory.ancestors() {
            directories.push(ancestor);
            if ancestor == project_root {
                break;
            }
        }
        for ancestor in directories.into_iter().rev() {
            let trusted = discovery
                .trust(ancestor)
                .or_else(|| discovery.trust(project_root))
                .or_else(|| repository.as_deref().and_then(|root| discovery.trust(root)))
                == Some(true);
            let folder = ancestor.join(".codex");
            let own_home = folder
                .canonicalize()
                .ok()
                .zip(self.codex_home.canonicalize().ok())
                .is_some_and(|(left, right)| left == right);
            if trusted
                && !own_home
                && let Some(layer) = read_configuration(&folder.join("config.toml"))?
            {
                copy_isolation(&layer, &mut effective);
            }
        }
        if let Some(managed) = managed {
            copy_isolation(managed, &mut effective);
        }
        self.validate_overlay(&effective, paths)
    }
}

fn copy_isolation(source: &toml_edit::DocumentMut, destination: &mut toml_edit::DocumentMut) {
    for key in ["cli_auth_credentials_store", "sqlite_home", "log_dir"] {
        if let Some(value) = source.get(key) {
            destination[key] = value.clone();
        }
    }
}

fn read_configuration(path: &Path) -> io::Result<Option<toml_edit::DocumentMut>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut text = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 1024 * 1024 {
        return Err(io::Error::other("TUI configuration layer is too large"));
    }
    text.parse().map(Some).map_err(|_| {
        // TOML errors can quote unrelated private values.
        io::Error::other("TUI configuration layer contains invalid TOML")
    })
}

struct ProjectDiscovery {
    markers: Vec<String>,
    projects: BTreeMap<String, bool>,
}
impl Default for ProjectDiscovery {
    fn default() -> Self {
        Self {
            markers: vec![".git".into()],
            projects: BTreeMap::new(),
        }
    }
}
impl ProjectDiscovery {
    fn apply(&mut self, layer: &toml_edit::DocumentMut) {
        if let Some(markers) = layer
            .get("project_root_markers")
            .and_then(toml_edit::Item::as_array)
            && let Some(markers) = markers
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        {
            self.markers = markers;
        }
        if let Some(projects) = layer
            .get("projects")
            .and_then(toml_edit::Item::as_table_like)
        {
            for (directory, project) in projects.iter() {
                if let Some(trust) = project.get("trust_level").and_then(toml_edit::Item::as_str) {
                    match trust {
                        "trusted" | "untrusted" => {
                            self.projects
                                .insert(directory.to_owned(), trust == "trusted");
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    fn trust(&self, directory: &Path) -> Option<bool> {
        let canonical = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_owned());
        #[cfg(windows)]
        let canonical = {
            // Native dunce::canonicalize uses the regular DOS/UNC spelling.
            let text = canonical.to_string_lossy();
            if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{rest}"))
            } else {
                PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
            }
        };
        for path in [&canonical, directory] {
            let key = path.to_string_lossy().into_owned();
            #[cfg(windows)]
            let key = key.to_ascii_lowercase();
            if let Some(trusted) = self.projects.get(&key) {
                return Some(*trusted);
            }
            #[cfg(windows)]
            if let Some((_, trusted)) = self
                .projects
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&key))
            {
                return Some(*trusted);
            }
        }
        None
    }
}

fn repository_trust_root(directory: &Path) -> Option<PathBuf> {
    for ancestor in directory.ancestors() {
        let git = ancestor.join(".git");
        if git.is_dir() && git.join("HEAD").exists() {
            return Some(ancestor.to_owned());
        }
        if git.is_file() {
            let pointer = fs::read_to_string(git).ok()?;
            let git = ancestor.join(pointer.trim().strip_prefix("gitdir: ")?);
            if let Ok(common) = fs::read_to_string(git.join("commondir")) {
                return git
                    .join(common.trim())
                    .canonicalize()
                    .ok()?
                    .parent()
                    .map(Path::to_owned);
            }
            return Some(ancestor.to_owned());
        }
    }
    None
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
