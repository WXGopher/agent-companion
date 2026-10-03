use super::{Operations, Release, Target, Version, numeric_version};
use crate::managed_tui::{self, Binding, no_redirects, read_limited, render_wrapper};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[path = "entries.rs"]
pub(crate) mod entries;

const CLI_RELEASE: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const APP_FEED: &str = "https://persistent.oaistatic.com/codex-app-prod/appcast.xml";
const OFFICIAL_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.openai.codex\" and certificate leaf[subject.OU] = \"2DC432GLL2\"";
const MAX_METADATA: u64 = 4 * 1024 * 1024;
const APP_PENDING: &str = "software-updates-app-pending.json";
const CLI_PENDING: &str = "software-updates-cli-pending.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct PrimaryEntryPublication {
    schema: u32,
    entry: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppPublication {
    schema: u32,
    destination: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    original: Version,
    target: Version,
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod tests;

pub(super) struct System {
    home: PathBuf,
    support: PathBuf,
}
pub(super) struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

impl System {
    pub fn new() -> Result<Self, String> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("无法定位用户目录。")?);
        no_redirects(&home)?;
        Ok(Self {
            support: home.join("Library/Application Support/AgentCompanion"),
            home,
        })
    }
    pub fn lock(&self) -> Result<Lock, String> {
        private_directory(&self.support)?;
        acquire_lock(&self.support.join("software-updates.lock"))
    }
    pub fn managed_cli(&self) -> Result<Option<PathBuf>, String> {
        let Some(binding) = self.managed_binding()? else {
            return Ok(None);
        };
        binding.executable().map(Some)
    }

    fn primary_app(&self) -> Result<PathBuf, String> {
        crate::macos_primary_app::discover(
            Path::new("/Applications"),
            &self.home.join("Applications"),
        )
        .map(|app| app.app)
        .ok_or_else(|| "未找到官方 Codex App。".into())
    }
    fn secondary(&self) -> Result<crate::macos_deployment::InstanceConfig, String> {
        let instance = crate::macos_deployment::maintenance_instance()
            .ok_or("未找到已接入的 Dodex 账号目录；请先接入现有双开环境。")?;
        for path in [
            &instance.codex_home,
            &instance.database_dir,
            &instance.desktop_user_data,
        ] {
            no_redirects(path)?;
            if !path.starts_with(&self.home)
                || path == &self.home
                || path.starts_with(self.home.join(".codex"))
                || path.starts_with(self.home.join("Library/Application Support/Codex"))
            {
                return Err("Dodex 的账号或会话目录与主账号重叠；未修改程序。".into());
            }
        }
        if !instance.codex_home.is_dir() {
            return Err("Dodex 账号目录不存在；未创建替代账号目录。".into());
        }
        Ok(instance)
    }
    fn command(&self, name: &str) -> Option<PathBuf> {
        managed_tui::command(&self.home, name)
    }
    fn primary_cli(&self) -> Result<PathBuf, String> {
        self.command("codex")
            .ok_or_else(|| "未找到 codex 终端命令。".into())
    }
    fn primary_native(&self) -> Result<PathBuf, String> {
        let native = entries::primary_native(&self.home, &self.primary_cli()?)?;
        let package = package_root(&native)?;
        let value: serde_json::Value = serde_json::from_slice(&read_limited(
            &package.join("codex-package.json"),
            64 * 1024,
        )?)
        .map_err(|_| "官方 TUI 清单无效。")?;
        let expected = Version {
            version: value["version"]
                .as_str()
                .ok_or("官方 TUI 版本无效。")?
                .into(),
            build: None,
        };
        validate_package(&package, &expected)?;
        Ok(native)
    }
    fn dodex_entry(&self) -> PathBuf {
        self.command("dodex")
            .unwrap_or_else(|| self.home.join(".local/bin/dodex"))
    }
    fn managed_binding(&self) -> Result<Option<Binding>, String> {
        let Some(binding) = managed_tui::read_binding(&self.home, &self.dodex_entry())? else {
            return Ok(None);
        };
        let profile = self.secondary()?;
        binding.validate_profile(
            &profile.codex_home,
            &profile.database_dir,
            &profile.desktop_user_data,
        )?;
        Ok(Some(binding))
    }
    fn cli_version(&self, path: &Path, profile: &Path) -> Result<Version, String> {
        let output = run(
            isolated_command(path, profile).arg("--version"),
            Duration::from_secs(20),
        )?;
        parse_cli_version(&output)
    }
    fn align_cli(&self, expected: &Version) -> Result<(), String> {
        let _deployment = acquire_lock(&self.support.join("deployment.lock"))?;
        if crate::macos_deployment::maintenance_app_needs_sync() {
            return Err(
                "Dodex App 启动器尚未对齐，未发布 TUI 入口；请先完成 App 更新后重试。".into(),
            );
        }
        let profile = self.secondary()?;
        let native = self.primary_native()?;
        let source = package_root(&native)?;
        validate_package(&source, expected)?;
        let old_binding = self.managed_binding()?;
        let entry = self.dodex_entry();
        let original = match &old_binding {
            Some(binding) => binding.original.clone(),
            None => {
                let bytes = read_limited(&entry, 128 * 1024).map_err(
                    |_| "未找到可保留原生 App 行为的 Dodex 启动入口；请先接入现有 Dodex。",
                )?;
                // Only the audited adapter has a fixed manager path and can be
                // relocated without changing its app / workspace semantics.
                entries::legacy_adapter(&self.home, &bytes)?;
                self.support.join("Tui/original-dodex")
            }
        };
        private_directory(&self.support.join("Tui/packages"))?;
        let staged = tempfile::Builder::new()
            .prefix(&format!("codex-{}-", expected.version))
            .tempdir_in(self.support.join("Tui/packages"))
            .map_err(|_| "无法暂存完整 TUI 安装包。")?;
        let staged_package = staged.path().join("package");
        run(
            Command::new("/usr/bin/ditto")
                .arg(&source)
                .arg(&staged_package),
            Duration::from_secs(300),
        )?;
        validate_package(&staged_package, expected)?;
        if self.cli_version(&staged_package.join("bin/codex"), &profile.codex_home)? != *expected {
            return Err("暂存的 Dodex TUI 版本验证失败。".into());
        }
        // Keep the complete versioned package, including code-mode-host and all
        // adjacent resources. No account data is copied into this directory.
        self.require_stopped()?;
        if old_binding.is_none() {
            preserve_original(&entry, &original)?;
        }
        let binding = Binding {
            package: staged_package,
            entry: entry.clone(),
            profile_home: profile.codex_home,
            app: crate::macos_deployment::settings_presentation()
                .app_path
                .ok_or("新版 Dodex App 入口尚未就绪；未发布 TUI 入口。")?,
            sqlite_home: profile.database_dir,
            log_dir: profile.desktop_user_data.join("logs"),
            desktop_data: profile.desktop_user_data,
            original,
            companion: std::env::current_exe().map_err(|_| "无法定位 Companion 程序。")?,
            version: expected.version.clone(),
        };
        self.require_stopped()?;
        let wrapper = render_wrapper(&binding)?;
        // Make the package durable before the entry can reference it. A crash
        // may leave an unused immutable package, never a dangling live entry.
        let _ = staged.keep();
        atomic_write(&entry, &wrapper, 0o755)?;
        // Publication is the commit point. Earlier failures clean up the full
        // staged package; a verified original-entry backup is reusable on retry.
        Ok(())
    }
    fn update_cli(&self, release: &Release) -> Result<(), String> {
        let entry = self.primary_cli()?;
        let native = self.primary_native()?;
        let primary_home = self.home.join(".codex");
        if self
            .cli_version(&native, &primary_home)?
            .compare(&release.version)?
            != std::cmp::Ordering::Less
        {
            return Ok(());
        }
        if native.starts_with(primary_home.join("packages/standalone/releases")) {
            let wrapped = !fs::symlink_metadata(&entry)
                .map_err(|_| "无法检查官方 TUI 入口。")?
                .file_type()
                .is_symlink()
                && entry != native;
            if wrapped {
                atomic_write(
                    &self.support.join(CLI_PENDING),
                    &serde_json::to_vec(&PrimaryEntryPublication {
                        schema: 1,
                        entry: entry.clone(),
                    })
                    .map_err(|_| "无法保存主账号入口恢复记录。")?,
                    0o600,
                )?;
            }
            // Preserve the vendor's complete package/resource layout and updater.
            let result = run(
                isolated_command(&native, &primary_home).arg("update"),
                Duration::from_secs(1200),
            );
            // A successful updater can replace our known wrapper with its
            // official link. Restore account isolation even on partial failure.
            if wrapped {
                self.recover_primary_entry()?;
            }
            result?;
        } else {
            return Err("当前版本管理要求官方 standalone 安装；不会自动迁移 npm、Homebrew 或其他安装管理器。".into());
        }
        Ok(())
    }

    fn recover_primary_entry(&self) -> Result<(), String> {
        let journal = self.support.join(CLI_PENDING);
        if !journal.exists() {
            return Ok(());
        }
        no_redirects(&journal)?;
        let pending: PrimaryEntryPublication =
            serde_json::from_slice(&read_limited(&journal, 8192)?)
                .map_err(|_| "主账号入口恢复记录无效。")?;
        if pending.schema != 1
            || ![
                self.home.join(".local/bin/codex"),
                PathBuf::from("/usr/local/bin/codex"),
                PathBuf::from("/opt/homebrew/bin/codex"),
            ]
            .contains(&pending.entry)
        {
            return Err("主账号入口恢复路径不受支持；未覆盖文件。".into());
        }
        let native = if pending.entry.exists() {
            entries::primary_native(&self.home, &pending.entry)?
        } else {
            entries::primary_native(
                &self.home,
                &self
                    .home
                    .join(".codex/packages/standalone/current/bin/codex"),
            )?
        };
        let package = package_root(&native)?;
        let metadata: serde_json::Value = serde_json::from_slice(&read_limited(
            &package.join("codex-package.json"),
            64 * 1024,
        )?)
        .map_err(|_| "官方 TUI 清单无效。")?;
        let expected = Version {
            version: metadata["version"]
                .as_str()
                .ok_or("官方 TUI 版本无效。")?
                .into(),
            build: None,
        };
        validate_package(&package, &expected)?;
        if self.cli_version(&native, &self.home.join(".codex"))? != expected {
            return Err("主账号入口恢复时的实际原生版本与包清单不符。".into());
        }
        self.require_stopped()?;
        atomic_write(
            &pending.entry,
            &entries::render_primary_wrapper(&self.home)?,
            0o755,
        )?;
        fs::remove_file(journal).map_err(|_| "主账号隔离入口已恢复；恢复记录仍保留。".into())
    }
    fn update_app(&self, release: &Release) -> Result<(), String> {
        let destination = self.primary_app()?;
        no_redirects(&destination)?;
        let parent = destination.parent().ok_or("App 安装目录无效。")?;
        // Stage on the destination volume, so publication/rollback are renames.
        let work = tempfile::Builder::new()
            .prefix(".codex-update-")
            .tempdir_in(parent)
            .map_err(|_| "无法写入 Codex App 的安装目录；请检查权限后重试。")?;
        let archive = work.path().join("official.zip");
        download(&release.url, &archive, release.length)?;
        let extracted = work.path().join("extracted");
        fs::create_dir(&extracted).map_err(|_| "无法创建 App 暂存目录。")?;
        // Validate names and symlink destinations before allowing ditto to
        // preserve Apple's bundle symlinks and extended attributes.
        run(
            Command::new("/usr/bin/python3")
                .args(["-B", "-c", include_str!("validate-zip.py")])
                .arg(&archive),
            Duration::from_secs(90),
        )?;
        run(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(&archive)
                .arg(&extracted),
            Duration::from_secs(300),
        )?;
        let apps = fs::read_dir(&extracted)
            .map_err(|_| "无法读取暂存 App。")?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .collect::<Vec<_>>();
        if apps.len() != 1 {
            return Err("官方安装包没有唯一的 App，未替换现有程序。".into());
        }
        let staged = &apps[0];
        no_redirects(staged)?;
        run(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "-R", OFFICIAL_REQUIREMENT])
                .arg(staged),
            Duration::from_secs(180),
        )?;
        if app_version(staged)?.as_ref() != Some(&release.version) {
            return Err("下载的 App 版本与官方发布记录不一致。".into());
        }
        // Coordinate with the existing Dodex startup mirror as well as other
        // settings editors. Do not let it read the primary midway through swap.
        let _deployment = acquire_lock(&self.support.join("deployment.lock"))?;
        self.require_stopped()?;
        let backup = parent.join(format!(
            ".{}-before-companion-{}",
            destination.file_name().unwrap().to_string_lossy(),
            release.version.build.as_deref().ok_or("App 缺少构建号。")?
        ));
        if fs::symlink_metadata(&backup).is_ok() {
            return Err("App 更新备份已存在，未覆盖。".into());
        }
        let original = app_version(&destination)?.ok_or("原 App 不可用。")?;
        if original.compare(&release.version)? == std::cmp::Ordering::Greater {
            return Err("本机 App 在暂存期间已更新到更高版本；已保留，未降级。".into());
        }
        let pending = AppPublication {
            schema: 1,
            destination: destination.clone(),
            staged: staged.clone(),
            backup,
            original,
            target: release.version.clone(),
        };
        let _ = work.keep();
        atomic_write(
            &self.support.join(APP_PENDING),
            &serde_json::to_vec(&pending).map_err(|_| "无法保存 App 恢复记录。")?,
            0o600,
        )?;
        resume_app_publication(&pending, verify_app)?;
        fs::remove_file(self.support.join(APP_PENDING))
            .map_err(|_| "App 已更新；恢复记录保留，将在下次维护时核验。")?;
        Ok(())
    }

    fn recover_app(&self) -> Result<(), String> {
        let path = self.support.join(APP_PENDING);
        if !path.exists() {
            return Ok(());
        }
        no_redirects(&path)?;
        let pending: AppPublication = serde_json::from_slice(&read_limited(&path, 64 * 1024)?)
            .map_err(|_| "App 恢复记录无效；未修改现有安装。")?;
        validate_app_publication(&pending, &self.home)?;
        let _deployment = acquire_lock(&self.support.join("deployment.lock"))?;
        self.require_stopped()?;
        let output = run(
            Command::new("/bin/ps").args(["-axww", "-o", "comm="]),
            Duration::from_secs(10),
        )?;
        if process_matches(
            &output,
            &[
                pending.destination.clone(),
                pending.staged.clone(),
                pending.backup.clone(),
            ],
        ) {
            return Err("App 恢复目录中的程序仍在运行；请退出后重试。".into());
        }
        resume_app_publication(&pending, verify_app)?;
        fs::remove_file(path).map_err(|_| "无法清理已验证的 App 恢复记录。".into())
    }
}

impl Operations for System {
    fn recover(&self) -> Result<(), String> {
        self.recover_app()?;
        self.recover_primary_entry()
    }
    fn cli_needs_sync(&self) -> bool {
        let Ok(Some(binding)) = self.managed_binding() else {
            return true;
        };
        let Some(app) = crate::macos_deployment::settings_presentation().app_path else {
            return true;
        };
        let Ok(companion) = std::env::current_exe() else {
            return true;
        };
        entries::needs_migration(&binding, &app, &companion)
    }
    fn app_needs_sync(&self) -> bool {
        crate::macos_deployment::maintenance_app_needs_sync()
    }
    fn installed(&self, target: Target) -> Result<Option<Version>, String> {
        match target {
            Target::CodexTui => {
                if self.command("codex").is_none() {
                    Ok(None)
                } else {
                    self.cli_version(&self.primary_native()?, &self.home.join(".codex"))
                        .map(Some)
                }
            }
            Target::DodexTui => {
                if let Some(binding) = self.managed_binding()? {
                    if !binding.package.join("bin/codex").is_file() {
                        return Ok(None);
                    }
                    validate_package(
                        &binding.package,
                        &Version {
                            version: binding.version.clone(),
                            build: None,
                        },
                    )?;
                    return self
                        .cli_version(&binding.package.join("bin/codex"), &binding.profile_home)
                        .map(Some);
                }
                let Some(instance) = crate::macos_deployment::maintenance_instance() else {
                    return Ok(None);
                };
                if !instance.cli_path.is_file() {
                    return Ok(None);
                }
                self.cli_version(&instance.cli_path, &instance.codex_home)
                    .map(Some)
            }
            Target::CodexApp => match self.primary_app() {
                Ok(path) => app_version(&path),
                Err(_) => Ok(None),
            },
            Target::DodexApp => {
                let presentation = crate::macos_deployment::settings_presentation();
                if let Some(path) = presentation.app_path {
                    return app_version(&path);
                }
                crate::macos_deployment::maintenance_instance()
                    .map(|instance| app_version(&instance.runtime_app))
                    .transpose()
                    .map(Option::flatten)
            }
        }
    }
    fn latest(&self, app: bool) -> Result<Release, String> {
        if app {
            let os = run(
                Command::new("/usr/bin/sw_vers").arg("-productVersion"),
                Duration::from_secs(5),
            )?;
            parse_appcast(
                &fetch(APP_FEED)?,
                std::env::consts::ARCH,
                String::from_utf8_lossy(&os).trim(),
            )
        } else {
            parse_cli_release(&fetch(CLI_RELEASE)?)
        }
    }
    fn preflight(&self, cli: bool, app: bool) -> Result<(), String> {
        self.secondary()?;
        if cli {
            let native = self.primary_native()?;
            let package = package_root(&native)?;
            let current = self.cli_version(&native, &self.home.join(".codex"))?;
            validate_package(&package, &current)?;
            if self.managed_binding()?.is_none() {
                let entry = self.dodex_entry();
                let bytes = read_limited(&entry, 128 * 1024)?;
                entries::legacy_adapter(&self.home, &bytes)?;
            }
        }
        if app {
            no_redirects(&self.primary_app()?)?;
        }
        Ok(())
    }
    fn require_stopped(&self) -> Result<(), String> {
        let mut paths = Vec::new();
        if let Ok(app) = self.primary_app() {
            paths.push(app);
        }
        if let Some(instance) = crate::macos_deployment::maintenance_instance() {
            paths.extend([
                instance.runtime_app,
                instance.launcher_app,
                instance.cli_path,
            ]);
        }
        if let Some(app) = crate::macos_deployment::settings_presentation().app_path {
            paths.push(app);
        }
        if let Ok(entry) = self.primary_cli() {
            paths.push(entry.clone());
            if let Ok(real) = self.primary_native() {
                if let Ok(package) = package_root(&real) {
                    paths.push(package);
                }
                paths.push(real);
            }
        }
        if let Some(binding) = self.managed_binding()? {
            paths.push(binding.package);
        }
        let output = run(
            Command::new("/bin/ps").args(["-axww", "-o", "comm="]),
            Duration::from_secs(10),
        )?;
        if process_matches(&output, &paths) {
            return Err("Codex / Dodex 的 App 或 TUI 仍在运行。请先自行退出相关 App 和终端会话，再点击重试；Companion 不会结束进程。".into());
        }
        Ok(())
    }
    fn update_primary(&self, app: bool, release: &Release) -> Result<(), String> {
        if app {
            self.update_app(release)
        } else {
            self.update_cli(release)
        }
    }
    fn align_secondary(&self, app: bool, expected: &Version) -> Result<(), String> {
        if app {
            crate::macos_deployment::sync_desktop_under_maintenance_lock().map(|_| ())
        } else {
            self.align_cli(expected)
        }
    }
}

fn acquire_lock(path: &Path) -> Result<Lock, String> {
    no_redirects(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "无法获取更新锁。")?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("另一个设置窗口或 Dodex 启动器正在操作，请完成后重试。".into());
    }
    Ok(Lock(file))
}
fn process_matches(output: &[u8], paths: &[PathBuf]) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .any(|value| {
            let executable = Path::new(value);
            matches!(value, "codex" | "dodex" | "codex-code-mode-host")
                || paths
                    .iter()
                    .any(|path| executable == path || executable.starts_with(path))
        })
}
fn preserve_original(entry: &Path, backup: &Path) -> Result<(), String> {
    no_redirects(backup)?;
    let original = read_limited(entry, 128 * 1024)?;
    if backup.exists() {
        if read_limited(backup, 128 * 1024)? != original {
            return Err("原 Dodex 入口与已有备份不一致；未覆盖。".into());
        }
        return Ok(());
    }
    let mut file = tempfile::NamedTempFile::new_in(backup.parent().ok_or("备份目录无效。")?)
        .map_err(|_| "无法备份原 Dodex 入口。")?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法设置备份权限。")?;
    file.write_all(&original)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "无法保存原 Dodex 入口备份。")?;
    file.persist_noclobber(backup)
        .map_err(|_| "无法发布原 Dodex 入口备份。")?;
    Ok(())
}
fn parse_cli_version(output: &[u8]) -> Result<Version, String> {
    let text = String::from_utf8_lossy(output);
    let version = text
        .trim()
        .strip_prefix("codex-cli ")
        .ok_or("TUI 未返回可识别的版本号。")?;
    semver::Version::parse(version).map_err(|_| "TUI 版本号无效。")?;
    Ok(Version {
        version: version.into(),
        build: None,
    })
}
fn app_version(app: &Path) -> Result<Option<Version>, String> {
    if !app.exists() {
        return Ok(None);
    }
    let output = run(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(app.join("Contents/Info.plist")),
        Duration::from_secs(5),
    )?;
    let plist: serde_json::Value =
        serde_json::from_slice(&output).map_err(|_| "App 的版本信息格式无效。")?;
    let value = |key| {
        plist[key]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "App 缺少版本信息。".to_owned())
    };
    Ok(Some(Version {
        version: value("CFBundleShortVersionString")?,
        build: Some(value("CFBundleVersion")?),
    }))
}
fn package_root(native: &Path) -> Result<PathBuf, String> {
    let root = native
        .parent()
        .and_then(Path::parent)
        .ok_or("无法定位完整 TUI 安装包。")?;
    if native != root.join("bin/codex") || !root.join("codex-package.json").is_file() {
        return Err("该 TUI 安装不是完整的官方 standalone 包，无法保证 Dodex 的附属资源与原生行为；请先用官方 standalone 安装 Codex。".into());
    }
    Ok(root.to_path_buf())
}
fn validate_package(root: &Path, expected: &Version) -> Result<(), String> {
    if managed_tui::package_version(root)? != expected.version {
        return Err("官方 TUI 安装包布局或版本不兼容；未发布不完整的 Dodex TUI。".into());
    }
    // Both native executables must retain their valid original code signatures.
    for file in ["bin/codex", "bin/codex-code-mode-host"] {
        run(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(root.join(file)),
            Duration::from_secs(30),
        )?;
    }
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let parent = path.parent().ok_or("入口目录无效。")?;
    no_redirects(parent)?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法暂存入口，请检查目录权限。")?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| "无法设置入口权限。")?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "无法写入隔离入口。")?;
    file.persist(path)
        .map_err(|_| "无法发布隔离入口；原入口保留。")?;
    sync_directory(parent)?;
    Ok(())
}
fn publish_with_backup(staged: &Path, destination: &Path, backup: &Path) -> Result<(), String> {
    fs::rename(destination, backup).map_err(|_| "无法备份原 Codex App；未替换。")?;
    sync_directory(destination.parent().ok_or("App 安装路径无效。")?)?;
    if fs::rename(staged, destination).is_err() {
        if fs::rename(backup, destination).is_err() {
            return Err(format!(
                "App 发布与回滚失败，原 App 保留在 {}。",
                backup.display()
            ));
        }
        return Err("App 发布失败，已恢复原 App。".into());
    }
    sync_directory(destination.parent().ok_or("App 安装路径无效。")?)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "无法持久保存更新切换；恢复记录与备份保留。".into())
}

fn verify_app(path: &Path, expected: &Version) -> Result<(), String> {
    no_redirects(path)?;
    run(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "-R", OFFICIAL_REQUIREMENT])
            .arg(path),
        Duration::from_secs(180),
    )?;
    if app_version(path)?.as_ref() != Some(expected) {
        return Err("App 恢复目录的版本与记录不符；未覆盖程序。".into());
    }
    Ok(())
}

fn validate_app_publication(pending: &AppPublication, home: &Path) -> Result<(), String> {
    let parent = pending.destination.parent().ok_or("App 恢复路径无效。")?;
    let stage_root = pending
        .staged
        .parent()
        .and_then(Path::parent)
        .ok_or("App 暂存路径无效。")?;
    let build = pending
        .target
        .build
        .as_deref()
        .ok_or("App 恢复版本无效。")?;
    numeric_version(build)?;
    let name = pending
        .destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("App 恢复路径无效。")?;
    if pending.schema != 1
        || !matches!(name, "ChatGPT.app" | "Codex.app")
        || (parent != Path::new("/Applications") && parent != home.join("Applications"))
        || pending.backup != parent.join(format!(".{name}-before-companion-{build}"))
        || stage_root.parent() != Some(parent)
        || !stage_root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".codex-update-"))
        || pending.staged.parent() != Some(stage_root.join("extracted").as_path())
    {
        return Err("App 恢复记录包含不受支持的路径；未覆盖文件。".into());
    }
    for path in [&pending.destination, &pending.staged, &pending.backup] {
        no_redirects(path)?;
    }
    Ok(())
}

/// Resume each durable state around the two renames. Verification precedes
/// every destructive step, and the original bundle is always retained.
fn resume_app_publication(
    pending: &AppPublication,
    verify: impl Fn(&Path, &Version) -> Result<(), String>,
) -> Result<(), String> {
    let AppPublication {
        destination,
        staged,
        backup,
        original,
        target,
        ..
    } = pending;
    if destination.exists() && verify(destination, target).is_ok() {
        verify(backup, original)?;
        return Ok(());
    }
    verify(staged, target)?;
    if destination.exists() {
        verify(destination, original)?;
        if backup.exists() {
            return Err("App 目标与恢复备份同时存在冲突；未覆盖文件。".into());
        }
        publish_with_backup(staged, destination, backup)?;
    } else {
        verify(backup, original)?;
        fs::rename(staged, destination)
            .map_err(|_| format!("无法继续 App 发布；原版本保留在 {}。", backup.display()))?;
    }
    verify(destination, target)
}
fn private_directory(path: &Path) -> Result<(), String> {
    no_redirects(path)?;
    fs::create_dir_all(path).map_err(|_| "无法创建更新目录。")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法设置更新目录权限。".into())
}
fn isolated_command(program: &Path, profile: &Path) -> Command {
    let mut command = Command::new(program);
    agent_companion_core::process_environment::isolate_command(&mut command);
    command.env("CODEX_HOME", profile);
    // Finder's PATH often omits the package manager, including npm's node.
    let mut paths = vec![
        program
            .parent()
            .unwrap_or(Path::new("/usr/bin"))
            .to_path_buf(),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    command
}
fn run(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let explicit: Vec<_> = command
        .get_envs()
        .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value.to_owned())))
        .collect();
    agent_companion_core::process_environment::isolate_command(command);
    command.envs(explicit);
    let mut stdout = tempfile::tempfile().map_err(|_| "无法创建命令输出缓冲区。")?;
    let stderr = tempfile::tempfile().map_err(|_| "无法创建命令输出缓冲区。")?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|_| "无法准备命令输出。")?)
        .stderr(stderr)
        .spawn()
        .map_err(|_| "无法启动更新工具，请检查安装和权限。")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                return Err("更新工具执行失败；请检查网络、磁盘空间及安装权限后重试。".into());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("更新工具超时或已中断；请检查安装状态后重试。".into());
            }
        }
    }
    stdout
        .seek(SeekFrom::Start(0))
        .map_err(|_| "无法读取工具结果。")?;
    let mut bytes = Vec::new();
    stdout
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取工具结果。")?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err("更新工具输出过大，请检查安装状态。".into());
    }
    Ok(bytes)
}
fn network(timeout: Duration, redirects: u32) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(timeout))
        .max_redirects(redirects)
        .user_agent(concat!("agent-companion/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut response = network(Duration::from_secs(30), 0)
        .get(url)
        .call()
        .map_err(|_| "无法连接官方版本服务；版本状态未知，可稍后重试。")?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取官方版本信息。")?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err("官方版本信息超出预期大小。".into());
    }
    Ok(bytes)
}
fn download(url: &str, destination: &Path, length: u64) -> Result<(), String> {
    if !official_app_url(url) || length == 0 || length > 2 * 1024 * 1024 * 1024 {
        return Err("官方安装包地址或大小无效。".into());
    }
    let mut response = network(Duration::from_secs(1200), 3)
        .get(url)
        .call()
        .map_err(|_| "无法下载官方安装包，请检查网络后重试。")?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination)
        .map_err(|_| "无法暂存官方安装包。")?;
    let count = std::io::copy(
        &mut response.body_mut().as_reader().take(length + 1),
        &mut file,
    )
    .map_err(|_| "官方安装包下载不完整。")?;
    if count != length {
        return Err("官方安装包大小与发布记录不符。".into());
    }
    file.sync_all().map_err(|_| "无法保存官方安装包。".into())
}
fn official_app_url(url: &str) -> bool {
    url.starts_with("https://persistent.oaistatic.com/codex-app-prod/")
        && url.ends_with(".zip")
        && !url.contains(['?', '#', '\\'])
        && !url.contains("/../")
}
fn parse_cli_release(bytes: &[u8]) -> Result<Release, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "官方 TUI 版本信息无效。")?;
    let version = value["tag_name"]
        .as_str()
        .and_then(|tag| tag.strip_prefix("rust-v"))
        .ok_or("官方 TUI 版本标签无效。")?;
    let parsed = semver::Version::parse(version).map_err(|_| "官方 TUI 版本号无效。")?;
    if value["draft"] != false || value["prerelease"] != false || !parsed.pre.is_empty() {
        return Err("官方记录不是 TUI 稳定版。".into());
    }
    Ok(Release {
        version: Version {
            version: version.into(),
            build: None,
        },
        url: String::new(),
        length: 0,
    })
}
fn parse_appcast(bytes: &[u8], arch: &str, os: &str) -> Result<Release, String> {
    let xml = std::str::from_utf8(bytes).map_err(|_| "官方 App 版本信息编码无效。")?;
    let document = roxmltree::Document::parse(xml).map_err(|_| "官方 App 版本信息格式无效。")?;
    let arch = if arch == "aarch64" { "arm64" } else { arch };
    let os = numeric_version(os)?;
    let mut releases = Vec::new();
    for item in document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
    {
        let field = |name| {
            item.children()
                .find(|node| node.is_element() && node.tag_name().name() == name)
                .and_then(|node| node.text())
        };
        if field("channel").is_some_and(|value| !value.is_empty() && value != "stable") {
            continue;
        }
        if field("minimumSystemVersion")
            .map(numeric_version)
            .transpose()?
            .is_some_and(|min| min > os)
        {
            continue;
        }
        // Only a direct enclosure is a full release; nested sparkle:deltas
        // contain partial patches which must never be installed as an App.
        for enclosure in item
            .children()
            .filter(|node| node.has_tag_name("enclosure"))
        {
            let attribute = |name| {
                enclosure
                    .attributes()
                    .find(|attribute| attribute.name() == name)
                    .map(|attribute| attribute.value())
            };
            if attribute("arch").is_some_and(|value| value != arch && value != "universal") {
                continue;
            }
            let Some(url) = attribute("url").filter(|url| official_app_url(url)) else {
                continue;
            };
            // Some Sparkle feeds encode the architecture only in the asset name.
            if !url.contains(&format!("darwin-{arch}-")) && attribute("arch").is_none() {
                continue;
            }
            let version = field("shortVersionString")
                .or_else(|| attribute("shortVersionString"))
                .ok_or("官方 App 缺少版本号。")?;
            let build = field("version")
                .or_else(|| attribute("version"))
                .ok_or("官方 App 缺少构建号。")?;
            let length = attribute("length")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|length| *length > 0 && *length <= 2 * 1024 * 1024 * 1024)
                .ok_or("官方 App 安装包大小无效。")?;
            releases.push((
                numeric_version(build)?,
                Release {
                    version: Version {
                        version: version.into(),
                        build: Some(build.into()),
                    },
                    url: url.into(),
                    length,
                },
            ));
        }
    }
    releases
        .into_iter()
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, release)| release)
        .ok_or_else(|| "未找到适用于此 Mac 的官方稳定版 App。".into())
}
