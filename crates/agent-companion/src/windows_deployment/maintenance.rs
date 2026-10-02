//! Immutable desktop/TUI packages and atomic schema-2 publication. Existing
//! profile directories and old packages survive failures and process crashes.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DesktopPackage {
    pub package: PathBuf,
    pub version: String,
    pub archive_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TuiPackage {
    pub package: PathBuf,
    pub version: String,
    pub hash: String,
}

pub(crate) fn lock() -> Result<File, String> {
    let root = current_root()?;
    let support = root.parent().ok_or("维护目录无效。")?;
    no_redirects(support)?;
    fs::create_dir_all(support).map_err(|_| "无法创建维护目录。")?;
    let path = support.join("software-updates.lock");
    no_redirects(&path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| "无法打开维护锁。")?;
    lock.try_lock()
        .map_err(|_| "另一个部署或维护操作正在进行，请完成后重试。")?;
    Ok(lock)
}

fn managed_package(root: &Path, package: &Path, directory: &str, leaf: &str) -> bool {
    package.file_name().is_some_and(|name| name == leaf)
        && package.parent().and_then(Path::parent) == Some(root.join(directory).as_path())
}

pub(super) fn valid_manifest_layout(root: &Path, manifest: &Manifest) -> bool {
    let mut expected = InstanceConfig::at(root);
    match manifest.schema {
        1 => manifest.desktop.is_none() && manifest.tui.is_none() && manifest.instance == expected,
        2 => {
            let Some(desktop) = &manifest.desktop else {
                return false;
            };
            if !managed_package(root, &desktop.package, "desktop-packages", "runtime")
                || desktop.archive_hash != manifest.archive_hash
            {
                return false;
            }
            expected.runtime_app = desktop.package.join("ChatGPT.exe");
            if let Some(tui) = &manifest.tui {
                if !managed_package(root, &tui.package, "tui-packages", "package") {
                    return false;
                }
                expected.cli_path = tui.package.join("bin/codex.exe");
            }
            manifest.instance == expected
        }
        _ => false,
    }
}

pub(crate) fn verified_instance() -> Result<Option<InstanceConfig>, String> {
    let root = current_root()?;
    if !root.join(MANIFEST).exists() {
        return Ok(None);
    }
    validate(&root, true).map(|manifest| Some(manifest.instance))
}

pub(crate) fn needs_sync(app: bool) -> bool {
    let Ok(root) = current_root() else {
        return true;
    };
    let Ok(manifest) = validate(&root, false) else {
        return true;
    };
    if manifest.schema != 2 {
        return true;
    }
    if app {
        manifest.desktop.is_none()
    } else {
        manifest.tui.is_none() || !shell::is_current().unwrap_or(false)
    }
}

pub(crate) fn runtime_version(runtime: &Path) -> Result<String, String> {
    let value = powershell(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); (Get-Item -LiteralPath (Join-Path $env:COMPANION_RUNTIME_CHECK 'ChatGPT.exe')).VersionInfo.ProductVersion",
        Some(runtime),
    )?;
    crate::software_updates::compare_app_builds(&value, &value)?;
    Ok(value)
}

pub(crate) fn official_desktop() -> Result<PathBuf, String> {
    official_runtime()
}

pub(crate) fn official_tui() -> Result<TuiPackage, String> {
    let primary = primary_home().map_err(|_| "无法定位主账号目录。")?;
    // The vendor's current junction is allowed only as a discovery entry. Every
    // byte we copy comes from its resolved, complete release package.
    let package = primary
        .join("packages/standalone/current")
        .canonicalize()
        .map_err(|_| "未找到官方 standalone TUI；请先安装完整 Codex CLI 包。")?;
    let releases = primary
        .join("packages/standalone/releases")
        .canonicalize()
        .map_err(|_| "官方 standalone 发布目录不可用。")?;
    if package.parent() != Some(releases.as_path()) {
        return Err("官方 TUI junction 未指向受支持的发布目录。".into());
    }
    let version = verify_package_layout(&package)?;
    verify_package_signatures(&package)?;
    let entry = powershell(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $c=Get-Command codex -CommandType Application,ExternalScript -ErrorAction Stop | Select-Object -First 1; if (!$c.Source) { exit 1 }; $c.Source",
        None,
    )?;
    validate_primary_entry(Path::new(&entry), &package.join("bin/codex.exe"))?;
    // cmd.exe also searches the current directory. A workspace-local wrapper
    // must not be hidden by inspecting a different PATH entry in PowerShell.
    let cwd = std::env::current_dir().map_err(|_| "无法核验当前目录的 Codex 入口。")?;
    for name in [
        "codex",
        "codex.com",
        "codex.exe",
        "codex.cmd",
        "codex.bat",
        "codex.ps1",
    ] {
        let candidate = cwd.join(name);
        if candidate.exists() {
            validate_primary_entry(&candidate, &package.join("bin/codex.exe"))?;
        }
    }
    Ok(TuiPackage {
        hash: tree_hash(&package)?,
        package,
        version,
    })
}

fn validate_primary_entry(entry: &Path, expected: &Path) -> Result<(), String> {
    let native = entry
        .canonicalize()
        .map_err(|_| "无法定位实际 codex 命令入口。")?;
    let expected = expected
        .canonicalize()
        .map_err(|_| "官方 TUI 程序不可用。")?;
    if native != expected {
        return Err("当前 codex 命令没有指向已验证的完整 standalone 包；未知 wrapper 或其他安装器入口均未执行或覆盖，请先修复命令路径。".into());
    }
    Ok(())
}

fn validate_native_version(expected: &str, output: &[u8]) -> Result<(), String> {
    let output = std::str::from_utf8(output).map_err(|_| "暂存 TUI 的原生版本输出无效。")?;
    if output.trim().strip_prefix("codex-cli ") != Some(expected) {
        return Err("暂存 TUI 的原生版本与目标和包清单不符；未发布入口或部署清单。".into());
    }
    Ok(())
}

fn verify_package_layout(package: &Path) -> Result<String, String> {
    no_redirects(package)?;
    let metadata: serde_json::Value = read_json(&package.join("codex-package.json"))?;
    let version = metadata["version"].as_str().ok_or("TUI 包没有版本。")?;
    semver::Version::parse(version).map_err(|_| "TUI 包版本无效。")?;
    if metadata["layoutVersion"] != 1
        || metadata["entrypoint"] != "bin/codex.exe"
        || metadata["resourcesDir"] != "codex-resources"
        || metadata["pathDir"] != "codex-path"
    {
        return Err("TUI 包布局不受支持；未发布不完整程序。".into());
    }
    for name in [
        "bin/codex.exe",
        "bin/codex-code-mode-host.exe",
        "codex-resources/codex-command-runner.exe",
        "codex-resources/codex-windows-sandbox-setup.exe",
        "codex-path/rg.exe",
    ] {
        let path = package.join(name);
        no_redirects(&path)?;
        if !path.is_file() {
            return Err(format!("完整 TUI 包缺少 {name}。"));
        }
    }
    Ok(version.into())
}

fn verify_package_signatures(package: &Path) -> Result<(), String> {
    powershell("$ErrorActionPreference='Stop'; $r=$env:COMPANION_RUNTIME_CHECK; foreach ($name in @('bin/codex.exe','bin/codex-code-mode-host.exe','codex-resources/codex-command-runner.exe','codex-resources/codex-windows-sandbox-setup.exe')) { $s=Get-AuthenticodeSignature -LiteralPath (Join-Path $r $name); if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=\"?OpenAI OpCo, LLC\"?,') { exit 1 } }; 'verified'", Some(package)).map(|_| ())
}

pub(super) fn verify_tui(package: &TuiPackage) -> Result<(), String> {
    if verify_package_layout(&package.package)? != package.version
        || tree_hash(&package.package)? != package.hash
    {
        return Err("托管完整 TUI 包已改变；未执行或覆盖程序。".into());
    }
    verify_package_signatures(&package.package)
}

fn tree_hash(root: &Path) -> Result<String, String> {
    fn add(root: &Path, path: &Path, digest: &mut Sha256) -> Result<(), String> {
        no_redirects(path)?;
        let mut entries = fs::read_dir(path)
            .map_err(|_| "无法校验完整 TUI 包。")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法校验完整 TUI 包。")?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            no_redirects(&path)?;
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "TUI 包路径无效。")?
                .to_string_lossy()
                .replace('\\', "/");
            digest.update(relative.as_bytes());
            digest.update([0]);
            if path.is_dir() {
                digest.update(b"directory\0");
                add(root, &path, digest)?;
            } else {
                digest.update(b"file\0");
                let mut file = File::open(path).map_err(|_| "无法校验 TUI 文件。")?;
                digest.update(
                    file.metadata()
                        .map_err(|_| "无法校验 TUI 文件。")?
                        .len()
                        .to_le_bytes(),
                );
                let mut buffer = [0; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer).map_err(|_| "无法校验 TUI 文件。")?;
                    if count == 0 {
                        break;
                    }
                    digest.update(&buffer[..count]);
                }
            }
        }
        Ok(())
    }
    let mut digest = Sha256::new();
    add(root, root, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

/// Fail closed if any executable inside a package we may replace is running.
/// No process is killed, including native App Resources helpers and old TUI.
pub(crate) fn require_stopped() -> Result<(), String> {
    let root = current_root()?;
    let mut paths = vec![
        root.join("runtime"),
        root.join("desktop-packages"),
        root.join("tui-packages"),
    ];
    if let Ok(official) = official_desktop() {
        paths.push(official);
    }
    if let Ok(home) = primary_home() {
        paths.push(home.join("packages/standalone"));
    }
    let result = powershell(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $items=@(Get-CimInstance Win32_Process | Select-Object Name,ExecutablePath); ConvertTo-Json -Compress -InputObject $items",
        None,
    )?;
    let processes: Vec<serde_json::Value> =
        serde_json::from_str(&result).map_err(|_| "无法确认运行中的 App/TUI；未替换程序。")?;
    if processes.iter().any(|process| {
        if let Some(executable) = process["ExecutablePath"].as_str() {
            paths
                .iter()
                .any(|path| path_contains(path, Path::new(executable)))
        } else {
            process["Name"].as_str().is_some_and(|name| {
                ["codex.exe", "codex-code-mode-host.exe", "ChatGPT.exe"]
                    .iter()
                    .any(|known| name.eq_ignore_ascii_case(known))
            })
        }
    }) {
        return Err("Codex / Dodex 的 App、TUI 或辅助进程仍在运行；请自行退出后重试，Companion 不会结束进程。".into());
    }
    Ok(())
}

fn path_contains(root: &Path, executable: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    let root = normalize(root);
    let executable = normalize(executable);
    executable == root || executable.starts_with(&(root + "\\"))
}

pub(crate) fn preflight() -> Result<(), String> {
    let root = current_root()?;
    validate(&root, true)?;
    shell::check_owned()?;
    Ok(())
}

fn write_manifest(root: &Path, original: &[u8], manifest: &Manifest) -> Result<(), String> {
    if fs::read(root.join(MANIFEST)).map_err(|_| "部署清单已变化。")? != original {
        return Err("部署清单在维护期间发生变化；未覆盖文件。".into());
    }
    let mut staged = tempfile::NamedTempFile::new_in(root).map_err(|_| "无法暂存部署清单。")?;
    serde_json::to_writer_pretty(&mut staged, manifest).map_err(|_| "无法保存部署清单。")?;
    staged
        .as_file()
        .sync_all()
        .map_err(|_| "无法持久保存部署清单。")?;
    staged
        .persist(root.join(MANIFEST))
        .map_err(|_| "无法切换部署清单；原安装与暂存包保留。")?;
    let mut state = shared().lock().unwrap_or_else(|error| error.into_inner());
    state.initialized = false;
    state.instance = None;
    Ok(())
}

pub(crate) fn align_desktop(expected: &str) -> Result<(), String> {
    let root = current_root()?;
    with_deployment_lock(&root, || align_desktop_under_lock(&root, expected))
}

fn align_desktop_under_lock(root: &Path, expected: &str) -> Result<(), String> {
    let mut manifest = validate(root, true)?;
    let original = fs::read(root.join(MANIFEST)).map_err(|_| "无法读取部署清单。")?;
    let source = official_desktop()?;
    if runtime_version(&source)? != expected {
        return Err("官方 App 在维护期间发生变化，请重试。".into());
    }
    let packages = root.join("desktop-packages");
    no_redirects(&packages)?;
    fs::create_dir_all(&packages).map_err(|_| "无法创建桌面包目录。")?;
    let staged = tempfile::Builder::new()
        .prefix("release-")
        .tempdir_in(packages)
        .map_err(|_| "无法暂存桌面包。")?;
    let package = staged.path().join("runtime");
    copy_tree(&source, &package)?;
    let hash = verify_runtime(&package, false)?;
    if runtime_version(&package)? != expected {
        return Err("暂存 App 版本验证失败。".into());
    }
    manifest.schema = 2;
    manifest.instance.runtime_app = package.join("ChatGPT.exe");
    manifest.archive_hash = hash.clone();
    manifest.desktop = Some(DesktopPackage {
        package,
        version: expected.into(),
        archive_hash: hash,
    });
    require_stopped()?;
    let _ = staged.keep(); // durable before publishing the sole atomic pointer
    write_manifest(root, &original, &manifest)?;
    validate(root, true)?;
    Ok(())
}

pub(crate) fn align_tui(expected: &str) -> Result<(), String> {
    let root = current_root()?;
    with_deployment_lock(&root, || align_tui_under_lock(&root, expected))
}

fn align_tui_under_lock(root: &Path, expected: &str) -> Result<(), String> {
    let mut manifest = validate(root, true)?;
    if manifest.desktop.is_none() {
        return Err("请先完成 Dodex App 清单迁移。".into());
    }
    let original = fs::read(root.join(MANIFEST)).map_err(|_| "无法读取部署清单。")?;
    let source = official_tui()?;
    if source.version != expected {
        return Err("官方 TUI 在维护期间发生变化，请重试。".into());
    }
    let packages = root.join("tui-packages");
    no_redirects(&packages)?;
    fs::create_dir_all(&packages).map_err(|_| "无法创建 TUI 包目录。")?;
    let staged = tempfile::Builder::new()
        .prefix("release-")
        .tempdir_in(packages)
        .map_err(|_| "无法暂存完整 TUI 包。")?;
    let package = staged.path().join("package");
    copy_tree(&source.package, &package)?;
    let tui = TuiPackage {
        package,
        version: source.version,
        hash: source.hash,
    };
    verify_tui(&tui)?;
    let mut version = isolated_command(
        &tui.package.join("bin/codex.exe"),
        &manifest.instance.codex_home,
        &manifest.instance.database_dir,
    );
    version.arg("--version");
    validate_native_version(expected, &run(&mut version, Duration::from_secs(20))?)?;
    manifest.schema = 2;
    manifest.instance.cli_path = tui.package.join("bin/codex.exe");
    manifest.tui = Some(tui);
    require_stopped()?;
    let _ = staged.keep();
    write_manifest(root, &original, &manifest)?;
    // shell installer knows the previous hashes and rejects unknown edits.
    shell::ensure()?;
    validate(root, true)?;
    Ok(())
}

pub(crate) fn initialize_under_deployment_lock() -> Result<InstanceConfig, String> {
    let root = current_root()?;
    let manifest = validate(&root, true)?;
    if manifest.desktop.is_none() {
        let source = official_desktop()?;
        let version = runtime_version(&source)?;
        // Validate the complete independent CLI before publishing an App-only
        // migration; a missing standalone package is a concrete setup error.
        official_tui()?;
        align_desktop_under_lock(&root, &version)?;
    }
    if validate(&root, false)?.tui.is_none() {
        let source = official_tui()?;
        align_tui_under_lock(&root, &source.version)?;
    }
    validate(&root, true).map(|manifest| manifest.instance)
}

pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut output = tempfile::tempfile().map_err(|_| "无法创建维护输出缓冲区。")?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|_| "无法创建维护输出缓冲区。")?)
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "无法启动维护工具；请检查安装或系统策略。")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                return Err(
                    "维护工具被权限、协议、网络或系统策略阻止；请在最终维护窗口处理后重试。".into(),
                );
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("维护工具超时；已保留安装状态，请重试核验。".into());
            }
        }
    }
    output
        .seek(SeekFrom::Start(0))
        .map_err(|_| "无法读取维护结果。")?;
    let mut bytes = Vec::new();
    output
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取维护结果。")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("维护工具输出过大。".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_manifest_remains_readable_but_new_manifest_cannot_bind_tui_to_desktop() {
        let root = Path::new(r"C:\Users\fixture\Dodex");
        let mut manifest = Manifest {
            schema: 1,
            instance: InstanceConfig::at(root),
            archive_hash: "old".into(),
            desktop: None,
            tui: None,
        };
        assert!(valid_manifest_layout(root, &manifest));
        manifest.schema = 2;
        assert!(!valid_manifest_layout(root, &manifest));
        let package = root.join("desktop-packages/release-fixture/runtime");
        manifest.instance.runtime_app = package.join("ChatGPT.exe");
        manifest.desktop = Some(DesktopPackage {
            package,
            version: "26.9.1".into(),
            archive_hash: "old".into(),
        });
        assert!(valid_manifest_layout(root, &manifest));
        manifest.tui = Some(TuiPackage {
            package: root.join("runtime"),
            version: "0.160.0".into(),
            hash: "fixture".into(),
        });
        assert!(!valid_manifest_layout(root, &manifest));
        let package = root.join("tui-packages/release-fixture/package");
        manifest.tui.as_mut().unwrap().package = package.clone();
        manifest.instance.cli_path = package.join("bin/codex.exe");
        assert!(valid_manifest_layout(root, &manifest));
    }
    #[test]
    fn every_package_child_blocks_replacement_without_prefix_collisions() {
        let root = Path::new(r"C:\Programs\Codex");
        assert!(path_contains(
            root,
            Path::new(r"\\?\c:\programs\codex\resources\codex.exe")
        ));
        assert!(path_contains(
            root,
            Path::new(r"C:\Programs\Codex\Helpers\code-mode-host.exe")
        ));
        assert!(!path_contains(
            root,
            Path::new(r"C:\Programs\Codex-other\codex.exe")
        ));
    }
    #[test]
    fn manifest_commit_keeps_packages_and_profile_across_retry_or_foreign_edit() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let mut manifest = Manifest {
            schema: 1,
            instance: InstanceConfig::at(root),
            archive_hash: "old".into(),
            desktop: None,
            tui: None,
        };
        let original = serde_json::to_vec(&manifest).unwrap();
        fs::write(root.join(MANIFEST), &original).unwrap();
        fs::create_dir_all(&manifest.instance.codex_home).unwrap();
        let auth = manifest.instance.codex_home.join("auth.json");
        fs::write(&auth, b"synthetic unchanged credentials").unwrap();
        let package = root.join("desktop-packages/release-fixture/runtime");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("ChatGPT.exe"), b"prepared validated desktop").unwrap();
        manifest.schema = 2;
        manifest.instance.runtime_app = package.join("ChatGPT.exe");
        manifest.desktop = Some(DesktopPackage {
            package: package.clone(),
            version: "26.9.1".into(),
            archive_hash: "old".into(),
        });
        // Crash before the atomic pointer publication leaves the old manifest
        // and the prepared package usable; retry publishes the same package.
        assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), original);
        write_manifest(root, &original, &manifest).unwrap();
        let committed = fs::read(root.join(MANIFEST)).unwrap();
        write_manifest(root, &committed, &manifest).unwrap();
        assert_eq!(fs::read(&auth).unwrap(), b"synthetic unchanged credentials");
        assert_eq!(
            fs::read(package.join("ChatGPT.exe")).unwrap(),
            b"prepared validated desktop"
        );
        fs::write(root.join(MANIFEST), b"foreign edit").unwrap();
        assert!(write_manifest(root, &committed, &manifest).is_err());
        assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), b"foreign edit");
        assert_eq!(fs::read(&auth).unwrap(), b"synthetic unchanged credentials");
    }

    #[test]
    fn complete_tui_layout_and_resource_bytes_are_part_of_validation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("codex-package.json"), br#"{"layoutVersion":1,"version":"0.160.0","entrypoint":"bin/codex.exe","resourcesDir":"codex-resources","pathDir":"codex-path"}"#).unwrap();
        let resources = [
            "bin/codex.exe",
            "bin/codex-code-mode-host.exe",
            "codex-resources/codex-command-runner.exe",
            "codex-resources/codex-windows-sandbox-setup.exe",
            "codex-path/rg.exe",
        ];
        for relative in resources {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture executable").unwrap();
        }
        assert_eq!(verify_package_layout(root).unwrap(), "0.160.0");
        let original = tree_hash(root).unwrap();
        fs::write(
            root.join("codex-path/rg.exe"),
            b"modified adjacent resource",
        )
        .unwrap();
        assert_ne!(tree_hash(root).unwrap(), original);
        fs::remove_file(root.join("bin/codex-code-mode-host.exe")).unwrap();
        assert!(verify_package_layout(root).is_err());
    }
    #[test]
    fn signed_native_version_must_match_metadata_before_any_publication() {
        validate_native_version("0.160.0", b"codex-cli 0.160.0\r\n").unwrap();
        for actual in [
            b"codex-cli 0.159.0\n".as_slice(),
            b"codex-cli 0.161.0-alpha.1\n",
            b"unknown wrapper output",
        ] {
            assert!(validate_native_version("0.160.0", actual).is_err());
        }
    }

    #[test]
    fn command_entry_must_resolve_to_the_expected_native_package_without_execution() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let package = root.join("package/bin");
        fs::create_dir_all(&package).unwrap();
        let native = package.join("codex.exe");
        fs::write(&native, b"fixture native").unwrap();
        validate_primary_entry(&native, &native).unwrap();
        let unrelated = root.join("codex.cmd");
        fs::write(&unrelated, b"@echo off\nexit /b 0\n").unwrap();
        assert!(validate_primary_entry(&unrelated, &native).is_err());
        assert_eq!(fs::read(&unrelated).unwrap(), b"@echo off\nexit /b 0\n");
        let other = root.join("other.exe");
        fs::write(&other, b"fixture native").unwrap();
        assert!(validate_primary_entry(&other, &native).is_err());
    }
}
