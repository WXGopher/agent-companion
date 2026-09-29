//! A desktop copy refreshed on launch or manually from the installed official app. Only the
//! public app bundle is replaced; the old TUI, manager and hidden runtime are
//! deliberately outside this module's write set.
use super::*;
use serde_json::{Map, Value, json};

const MIRROR_SCHEMA: u32 = 1;
const PACKAGING_REVISION: u32 = 1;
const BUNDLE_ID: &str = "local.agent-companion.dodex";
const LAUNCHER_EXECUTABLE: &str = "DodexLauncher";
const LAUNCHER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/DodexLauncher"));
const MIRROR_MANIFEST: &str = "Contents/Resources/agent-companion-mirror.json";
const ICON: &[u8] = include_bytes!("../../macos/Dodex.icns");
const REQUIRED_FEATURES: [&[u8]; 3] = [
    b"CODEX_ELECTRON_USER_DATA_PATH",
    b"CODEX_HOME",
    b"CODEX_SPARKLE_ENABLED",
];

#[cfg(test)]
#[path = "mirror_launcher_tests.rs"]
mod launcher_tests;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct MirrorProfile {
    pub codex_home: PathBuf,
    pub desktop_user_data: PathBuf,
    pub database_dir: PathBuf,
}
impl MirrorProfile {
    pub(super) fn from_instance(instance: &InstanceConfig) -> Self {
        Self {
            codex_home: instance.codex_home.clone(),
            desktop_user_data: instance.desktop_user_data.clone(),
            database_dir: instance.database_dir.clone(),
        }
    }
    fn fresh(layout: &Layout) -> Self {
        let root = layout.support.join("DodexApp");
        Self {
            codex_home: root.join("codex-home"),
            desktop_user_data: root.join("desktop-data"),
            database_dir: root.join("codex-home/sqlite"),
        }
    }
    fn instance(&self, app: &Path) -> InstanceConfig {
        InstanceConfig {
            id: "dodex".into(),
            label: "Dodex".into(),
            codex_home: self.codex_home.clone(),
            desktop_user_data: self.desktop_user_data.clone(),
            database_dir: self.database_dir.clone(),
            runtime_app: app.to_path_buf(),
            launcher_app: app.to_path_buf(),
            // Recent official apps manage their own runtime and have no bundled
            // CLI here. This is a candidate path, never a TUI fallback.
            cli_path: app.join("Contents/Resources/codex"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct MirrorStatus {
    pub instance: InstanceConfig,
    pub source_app: PathBuf,
    pub source_version: String,
    pub source_build: String,
    pub up_to_date: bool,
    pub backup_app: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SourceFingerprint {
    version: String,
    build: String,
    executable: String,
    plist_sha256: String,
    executable_sha256: String,
    archive_sha256: String,
    seal_sha256: String,
    cli_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MirrorManifest {
    schema: u32,
    #[serde(default)]
    packaging_revision: u32,
    #[serde(default)]
    updater_executable: Option<PathBuf>,
    app: PathBuf,
    profile: MirrorProfile,
    source_app: PathBuf,
    source: SourceFingerprint,
    backup_app: Option<PathBuf>,
}

fn target(layout: &Layout) -> Result<PathBuf, String> {
    let system = layout.system_applications.join("Dodex.app");
    let user = layout.applications.join("Dodex.app");
    if exists(&system) && exists(&user) {
        return Err("发现两个公共 Dodex App；请先保留一个入口再同步。".into());
    }
    Ok(if exists(&system) { system } else { user })
}

pub(super) fn deployed_app(layout: &Layout) -> Option<PathBuf> {
    let app = target(layout).ok()?;
    exists(&app.join(MIRROR_MANIFEST)).then_some(app)
}

pub(super) fn is_deployed(layout: &Layout) -> bool {
    deployed_app(layout).is_some()
}

/// Cheap settings-only observation of a completed mirror. Do not call `check`
/// here: polling the UI must not hash an App or verify either signature.
pub(super) fn installed_presentation(layout: &Layout) -> Option<(InstanceConfig, String)> {
    let app = target(layout).ok()?;
    let manifest = read_manifest(&app).ok()?;
    if manifest.app != app {
        return None;
    }
    validate_profile(layout, &manifest.profile).ok()?;
    executable_path(&app, LAUNCHER_EXECUTABLE).ok()?;
    executable_path(&app, &manifest.source.executable).ok()?;
    regular_file(&app.join("Contents/Info.plist")).ok()?;
    regular_file(&app.join("Contents/Resources/app.asar")).ok()?;
    let version = match (
        manifest.source.version.as_str(),
        manifest.source.build.as_str(),
    ) {
        ("", "") => String::new(),
        (version, "") => version.into(),
        ("", build) => format!("build {build}"),
        (version, build) => format!("{version} ({build})"),
    };
    Some((manifest.profile.instance(&app), version))
}

fn saved_profile(layout: &Layout, app: &Path) -> Result<MirrorProfile, String> {
    if exists(&app.join(MIRROR_MANIFEST)) {
        return Ok(read_manifest(app)?.profile);
    }
    if exists(&layout.settings()) {
        let record: Record = read_json(&layout.settings())?;
        if record.schema != SCHEMA {
            return Err("已有 Dodex 配置记录版本不兼容；未修改任何文件。".into());
        }
        return Ok(MirrorProfile::from_instance(&record.instance));
    }
    // The original installation may predate Companion's preference record.
    // Its existing directories are reused without opening credentials/config.
    if exists(app) {
        let profile = MirrorProfile {
            codex_home: layout.user_home.join(".codex-second"),
            desktop_user_data: layout.user_home.join("Library/Application Support/Codex-B"),
            database_dir: layout.user_home.join(".codex-second/sqlite"),
        };
        if profile.codex_home.is_dir() && profile.desktop_user_data.is_dir() {
            return Ok(profile);
        }
    }
    Ok(MirrorProfile::fresh(layout))
}

pub(super) fn sync(
    layout: &Layout,
    notify: impl FnMut(&str, &str),
) -> Result<MirrorStatus, String> {
    let app = target(layout)?;
    let profile = saved_profile(layout, &app)?;
    sync_profile(layout, &profile, notify)
}

pub(super) fn sync_profile(
    layout: &Layout,
    profile: &MirrorProfile,
    notify: impl FnMut(&str, &str),
) -> Result<MirrorStatus, String> {
    let source =
        crate::macos_primary_app::discover(&layout.system_applications, &layout.applications)
            .ok_or("未找到已安装的官方 Codex App；请先安装或更新官方应用。")?
            .app;
    sync_with(layout, profile, &source, &SystemMirrorOps, notify)
}

/// Called with the deployment lock held by the native bootstrap's direct child.
/// Matching release metadata avoids copying, hashing and signature checks at launch.
pub(super) fn sync_on_launch(layout: &Layout, app: &Path) -> Result<(), String> {
    let parent_pid = unsafe { libc::getppid() } as u32;
    sync_on_launch_with(layout, app, parent_pid, &SystemMirrorOps).map(|_| ())
}

fn sync_on_launch_with(
    layout: &Layout,
    requested_app: &Path,
    parent_pid: u32,
    ops: &dyn MirrorOperations,
) -> Result<MirrorStatus, String> {
    let app = target(layout)?;
    if requested_app != app {
        return Err("启动同步只支持当前公共 Dodex App。".into());
    }
    no_symlinks(&app)?;
    verify_launch_parent(&processes(&ops.processes()?)?, &app, parent_pid)?;
    let manifest = read_manifest(&app)?;
    if manifest.app != app {
        return Err("Dodex App 记录与公共入口不一致。".into());
    }
    let source =
        crate::macos_primary_app::discover(&layout.system_applications, &layout.applications)
            .ok_or("未找到已安装的官方 Codex App；保留现有 Dodex。")?
            .app;
    let (version, build) = source_version(&source)?;
    let updater = updater_executable()?;
    if manifest.source.version == version
        && manifest.source.build == build
        && current_packaging(&manifest, &updater)
    {
        return Ok(status(&manifest, true));
    }
    sync_with_caller(
        layout,
        &manifest.profile,
        &source,
        ops,
        Some(parent_pid),
        |_, _| {},
    )
}

fn updater_executable() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|_| "无法定位 Dodex App 更新程序。".into())
}

fn current_packaging(manifest: &MirrorManifest, updater: &Path) -> bool {
    manifest.packaging_revision == PACKAGING_REVISION
        && manifest.updater_executable.as_deref() == Some(updater)
}

pub(super) fn check(layout: &Layout) -> Result<MirrorStatus, String> {
    let app = target(layout)?;
    let manifest = validate_app(layout, &app, &app, &SystemMirrorOps)?;
    let current =
        crate::macos_primary_app::discover(&layout.system_applications, &layout.applications)
            .and_then(|source| read_source(&source.app, &SystemMirrorOps).ok());
    Ok(status(
        &manifest,
        current.as_ref() == Some(&manifest.source),
    ))
}

fn status(manifest: &MirrorManifest, up_to_date: bool) -> MirrorStatus {
    MirrorStatus {
        instance: manifest.profile.instance(&manifest.app),
        source_app: manifest.source_app.clone(),
        source_version: manifest.source.version.clone(),
        source_build: manifest.source.build.clone(),
        up_to_date,
        backup_app: manifest.backup_app.clone(),
    }
}

trait MirrorOperations {
    fn verify_official(&self, app: &Path) -> Result<(), String>;
    fn copy(&self, source: &Path, destination: &Path) -> Result<(), String>;
    fn sign(&self, app: &Path) -> Result<(), String>;
    fn verify_local(&self, app: &Path) -> Result<(), String>;
    fn processes(&self) -> Result<Vec<u8>, String>;
    fn register(&self, app: &Path);
}
struct SystemMirrorOps;
impl MirrorOperations for SystemMirrorOps {
    fn verify_official(&self, app: &Path) -> Result<(), String> {
        if !icon_signature::verify(app)? {
            return Err("Codex 源应用不是有效的 OpenAI 官方签名；未修改 Dodex。".into());
        }
        Ok(())
    }
    fn copy(&self, source: &Path, destination: &Path) -> Result<(), String> {
        SystemOps.copy_runtime(source, destination)
    }
    fn sign(&self, app: &Path) -> Result<(), String> {
        let manifest = read_manifest(app)?;
        let native = executable_path(app, &manifest.source.executable)?;
        let output = Command::new("/usr/bin/codesign")
            .args(["--display", "--entitlements", ":-"])
            .arg(&native)
            .output()
            .map_err(|_| "无法读取官方 App 的运行权限。")?;
        if !output.status.success() {
            return Err("无法读取官方 App 的运行权限。".into());
        }
        let entitlements = plist_from_bytes(&output.stdout)?;
        let entitlements = sanitized_entitlements(entitlements)?;
        let temporary = tempfile::Builder::new()
            .prefix("dodex-signing-")
            .tempdir()
            .map_err(|_| "无法创建本地签名临时目录。")?;
        let path = temporary
            .path()
            .canonicalize()
            .map_err(|_| "无法定位本地签名临时目录。")?
            .join("entitlements.plist");
        write_new(&path, &serde_json::to_vec(&entitlements).unwrap(), 0o600)?;
        command_ok(
            Command::new("/usr/bin/plutil")
                .args(["-convert", "xml1"])
                .arg(&path),
        )?;
        // The native executable retains its runtime permissions, without the
        // vendor identity/shared-access entitlements. Sign it before sealing the
        // bundle whose entry point is now the small same-bundle native bootstrap.
        command_ok(
            Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--timestamp=none",
                    "--identifier",
                    BUNDLE_ID,
                ])
                .arg("--entitlements")
                .arg(path)
                .arg(native),
        )?;
        command_ok(
            Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--timestamp=none",
                    "--identifier",
                    BUNDLE_ID,
                ])
                .arg(app),
        )
    }
    fn verify_local(&self, app: &Path) -> Result<(), String> {
        let manifest = read_manifest(app)?;
        let native = executable_path(app, &manifest.source.executable)?;
        command_ok(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict", "-R"])
                .arg(format!("=identifier \"{BUNDLE_ID}\""))
                .arg(native),
        )?;
        command_ok(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "-R"])
                .arg(format!("=identifier \"{BUNDLE_ID}\""))
                .arg(app),
        )?;
        let output = Command::new("/usr/bin/codesign")
            .args(["--display", "--verbose=2"])
            .arg(app)
            .output()
            .map_err(|_| "无法检查 Dodex 本地签名。")?;
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stderr)
                .lines()
                .any(|line| line == "Signature=adhoc")
        {
            return Err("Dodex 不是预期的本地 ad-hoc 签名。".into());
        }
        Ok(())
    }
    fn processes(&self) -> Result<Vec<u8>, String> {
        let output = Command::new("/bin/ps")
            .args(["-axww", "-o", "pid=", "-o", "comm="])
            .output()
            .map_err(|_| "无法检查 Dodex 桌面进程。")?;
        if !output.status.success() {
            return Err("无法检查 Dodex 桌面进程；未同步 App。".into());
        }
        Ok(output.stdout)
    }
    fn register(&self, app: &Path) {
        // Registration refreshes Finder/Dock metadata; it does not launch an app.
        let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
            .arg("-f")
            .arg(app)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn command_ok(command: &mut Command) -> Result<(), String> {
    if command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "无法执行 Dodex App 文件或签名操作。")?
        .success()
    {
        Ok(())
    } else {
        Err("Dodex App 文件或签名操作失败；现有应用尚未替换。".into())
    }
}

fn plist_from_bytes(bytes: &[u8]) -> Result<Value, String> {
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "--", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法读取 App 属性列表。")?;
    child
        .stdin
        .take()
        .ok_or("无法读取 App 属性列表。")?
        .write_all(bytes)
        .map_err(|_| "无法读取 App 属性列表。")?;
    let output = child
        .wait_with_output()
        .map_err(|_| "无法读取 App 属性列表。")?;
    if !output.status.success() {
        return Err("App 属性列表格式无效。".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "App 属性列表格式无效。".into())
}

fn read_plist(app: &Path) -> Result<Value, String> {
    plist_from_bytes(&read_limited(
        &app.join("Contents/Info.plist"),
        2 * 1024 * 1024,
    )?)
}

fn string_field<'a>(plist: &'a Value, key: &str) -> Result<&'a str, String> {
    plist
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Codex App 缺少 {key} 信息。"))
}

fn executable_name(plist: &Value) -> Result<String, String> {
    let name = string_field(plist, "CFBundleExecutable")?;
    validate_executable_name(name)?;
    Ok(name.into())
}

fn validate_executable_name(name: &str) -> Result<(), String> {
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("Codex App 的主程序路径无效。".into());
    }
    Ok(())
}

fn executable_path(app: &Path, name: &str) -> Result<PathBuf, String> {
    validate_executable_name(name)?;
    let path = app.join("Contents/MacOS").join(name);
    regular_file(&path)?;
    if fs::metadata(&path)
        .map_err(|_| "无法检查 App 主程序。")?
        .permissions()
        .mode()
        & 0o111
        == 0
    {
        return Err("App 的主程序不可执行。".into());
    }
    Ok(path)
}

fn digest(path: &Path) -> Result<String, String> {
    file_sha256(path).ok_or_else(|| "无法计算 App 文件指纹。".into())
}

fn source_version(app: &Path) -> Result<(String, String), String> {
    no_symlinks(app)?;
    let plist = read_plist(app)?;
    if string_field(&plist, "CFBundleIdentifier")? != "com.openai.codex" {
        return Err("只能从本机官方 Codex App 同步 Dodex。".into());
    }
    Ok((
        string_field(&plist, "CFBundleShortVersionString")?.into(),
        string_field(&plist, "CFBundleVersion")?.into(),
    ))
}

fn read_source(app: &Path, ops: &dyn MirrorOperations) -> Result<SourceFingerprint, String> {
    no_symlinks(app)?;
    let plist = read_plist(app)?;
    if string_field(&plist, "CFBundleIdentifier")? != "com.openai.codex" {
        return Err("只能从本机官方 Codex App 同步 Dodex。".into());
    }
    let executable = executable_name(&plist)?;
    if executable == LAUNCHER_EXECUTABLE {
        return Err("Codex App 的主程序与 Dodex 启动入口冲突。".into());
    }
    let executable_path = executable_path(app, &executable)?;
    ops.verify_official(app)?;
    let archive = app.join("Contents/Resources/app.asar");
    if !supports_environment(&archive)? {
        return Err("该官方 Codex 版本不支持 Dodex 所需的目录隔离和手动更新设置。".into());
    }
    let cli = app.join("Contents/Resources/codex");
    Ok(SourceFingerprint {
        version: string_field(&plist, "CFBundleShortVersionString")?.into(),
        build: string_field(&plist, "CFBundleVersion")?.into(),
        executable,
        plist_sha256: digest(&app.join("Contents/Info.plist"))?,
        executable_sha256: digest(&executable_path)?,
        archive_sha256: digest(&archive)?,
        seal_sha256: digest(&app.join("Contents/_CodeSignature/CodeResources"))?,
        cli_sha256: exists(&cli).then(|| digest(&cli)).transpose()?,
    })
}

fn supports_environment(path: &Path) -> Result<bool, String> {
    regular_file(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(no_follow_flag())
        .open(path)
        .map_err(|_| "无法读取官方 App 的运行资源。")?;
    let overlap = REQUIRED_FEATURES
        .iter()
        .map(|feature| feature.len())
        .max()
        .unwrap()
        - 1;
    let mut buffer = vec![0; 65536 + overlap];
    let mut retained = 0;
    let mut found = [false; REQUIRED_FEATURES.len()];
    loop {
        let count = file
            .read(&mut buffer[retained..])
            .map_err(|_| "无法校验官方 App 的运行资源。")?;
        if count == 0 {
            return Ok(false);
        }
        let length = retained + count;
        for (index, feature) in REQUIRED_FEATURES.iter().enumerate() {
            found[index] |= buffer[..length]
                .windows(feature.len())
                .any(|slice| slice == *feature);
        }
        if found.iter().all(|found| *found) {
            return Ok(true);
        }
        retained = overlap.min(length);
        buffer.copy_within(length - retained..length, 0);
    }
}

fn environment(profile: &MirrorProfile, app: &Path, bundled_cli: bool) -> Value {
    let cli = if bundled_cli {
        app.join("Contents/Resources/codex")
    } else {
        PathBuf::new()
    };
    json!({
        "CODEX_HOME": profile.codex_home,
        "CODEX_INSTALL_DIR": profile.codex_home.join("bin"),
        "CODEX_ELECTRON_USER_DATA_PATH": profile.desktop_user_data,
        "CODEX_SQLITE_HOME": profile.database_dir,
        "CODEX_CLI_PATH": cli,
        "CODEX_APP_SERVER_FORCE_CLI": if bundled_cli { "1" } else { "0" },
        "CODEX_APP_SERVER_USE_LOCAL_DAEMON": "0",
        "CODEX_APP_SERVER_WS_URL": "",
        "CODEX_SPARKLE_ENABLED": "false"
    })
}

fn customize_plist(
    mut plist: Value,
    profile: &MirrorProfile,
    app: &Path,
    native_executable: &str,
    bundled_cli: bool,
    updater: Option<&Path>,
) -> Result<Value, String> {
    let fields = plist.as_object_mut().ok_or("App 属性列表不是字典。")?;
    for (key, value) in [
        ("CFBundleIdentifier", BUNDLE_ID),
        ("CFBundleExecutable", LAUNCHER_EXECUTABLE),
        ("DodexNativeExecutable", native_executable),
        ("CFBundleName", "Dodex"),
        ("CFBundleDisplayName", "Dodex"),
        ("CFBundleIconFile", "Dodex.icns"),
        ("CrProductDirName", BUNDLE_ID),
    ] {
        fields.insert(key.into(), Value::String(value.into()));
    }
    fields.insert("CFBundleAlternateNames".into(), json!(["Dodex"]));
    fields.insert("LSHasLocalizedDisplayName".into(), Value::Bool(false));
    if let Some(updater) = updater {
        fields.insert("DodexUpdaterExecutable".into(), json!(updater));
    } else {
        fields.remove("DodexUpdaterExecutable");
    }
    // An asset catalog or Dock tile plug-in can override CFBundleIconFile.
    for key in [
        "CFBundleIconName",
        "NSDockTilePlugIn",
        "CFBundleURLTypes",
        "LSUIElement",
    ] {
        fields.remove(key);
    }
    let mut env = fields
        .remove("LSEnvironment")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    env.extend(
        environment(profile, app, bundled_cli)
            .as_object()
            .unwrap()
            .clone(),
    );
    fields.insert("LSEnvironment".into(), Value::Object(env));
    Ok(plist)
}

fn sanitized_entitlements(value: Value) -> Result<Value, String> {
    let fields = value.as_object().ok_or("官方 App 运行权限格式无效。")?;
    Ok(Value::Object(
        fields
            .iter()
            .filter(|(key, _)| {
                key.starts_with("com.apple.security.")
                    && key.as_str() != "com.apple.security.application-groups"
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<_, _>>(),
    ))
}

fn validate_profile(layout: &Layout, profile: &MirrorProfile) -> Result<(), String> {
    let primary = [
        layout.user_home.join(".codex"),
        layout.user_home.join("Library/Application Support/Codex"),
    ];
    for path in [
        &profile.codex_home,
        &profile.desktop_user_data,
        &profile.database_dir,
    ] {
        no_symlinks(path)?;
        if !path.starts_with(&layout.user_home)
            || path == &layout.user_home
            || primary
                .iter()
                .any(|primary| path.starts_with(primary) || primary.starts_with(path))
        {
            return Err("Dodex App 数据目录必须位于当前用户目录内，且不能与官方实例重叠。".into());
        }
        if exists(path) && !path.is_dir() {
            return Err("Dodex App 数据目录与现有文件冲突。".into());
        }
    }
    Ok(())
}

fn prepare_profile(layout: &Layout, profile: &MirrorProfile, app: &Path) -> Result<(), String> {
    validate_profile(layout, profile)?;
    let fresh = profile == &MirrorProfile::fresh(layout)
        && !exists(&profile.codex_home.join("config.toml"));
    for path in [
        &profile.codex_home,
        &profile.desktop_user_data,
        &profile.database_dir,
    ] {
        private_directory(path)?;
    }
    // Existing configs, auth files, sessions and databases are never rewritten.
    if fresh {
        write_new(
            &profile.codex_home.join("config.toml"),
            config_text(&profile.instance(app)).as_bytes(),
            0o600,
        )?;
    }
    Ok(())
}

fn read_manifest(app: &Path) -> Result<MirrorManifest, String> {
    let manifest: MirrorManifest = read_json(&app.join(MIRROR_MANIFEST))?;
    if manifest.schema != MIRROR_SCHEMA {
        return Err("Dodex App 镜像记录版本不兼容。".into());
    }
    match (manifest.packaging_revision, &manifest.updater_executable) {
        (0, None) => {}
        (PACKAGING_REVISION, Some(updater)) if updater.is_absolute() => {}
        _ => return Err("Dodex App 启动更新记录版本不兼容。".into()),
    }
    Ok(manifest)
}

fn validate_app(
    layout: &Layout,
    app: &Path,
    final_app: &Path,
    ops: &dyn MirrorOperations,
) -> Result<MirrorManifest, String> {
    no_symlinks(app)?;
    let manifest = read_manifest(app)?;
    if manifest.app != final_app {
        return Err("Dodex App 记录与公共入口不一致。".into());
    }
    validate_profile(layout, &manifest.profile)?;
    let plist = read_plist(app)?;
    let expected = customize_plist(
        plist.clone(),
        &manifest.profile,
        final_app,
        &manifest.source.executable,
        manifest.source.cli_sha256.is_some(),
        manifest.updater_executable.as_deref(),
    )?;
    executable_path(app, LAUNCHER_EXECUTABLE)?;
    executable_path(app, &manifest.source.executable)?;
    if plist != expected
        || manifest.source.executable == LAUNCHER_EXECUTABLE
        || digest(&app.join("Contents/Resources/app.asar"))? != manifest.source.archive_sha256
        || read_limited(&app.join("Contents/Resources/Dodex.icns"), 8 * 1024 * 1024)? != ICON
    {
        return Err("Dodex App 的图标、隔离设置或官方资源发生变化；未覆盖现有 App。".into());
    }
    let cli = app.join("Contents/Resources/codex");
    if exists(&cli).then(|| digest(&cli)).transpose()? != manifest.source.cli_sha256 {
        return Err("Dodex App 的内置运行程序发生变化。".into());
    }
    let expected_backup = final_app.with_file_name(".Dodex-mirror-backup.app");
    if manifest
        .backup_app
        .as_ref()
        .is_some_and(|backup| backup != &expected_backup)
    {
        return Err("Dodex App 备份路径无效。".into());
    }
    ops.verify_local(app)?;
    Ok(manifest)
}

struct DesktopProcess {
    pid: u32,
    executable: PathBuf,
}

fn processes(output: &[u8]) -> Result<Vec<DesktopProcess>, String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (pid, executable) = line
                .trim_start()
                .split_once(char::is_whitespace)
                .ok_or("无法读取 Dodex 桌面进程信息。")?;
            let pid = pid
                .parse::<u32>()
                .ok()
                .filter(|pid| *pid > 0)
                .ok_or("无法读取 Dodex 桌面进程编号。")?;
            Ok(DesktopProcess {
                pid,
                executable: PathBuf::from(executable.trim_start()),
            })
        })
        .collect()
}

fn verify_launch_parent(
    processes: &[DesktopProcess],
    app: &Path,
    parent_pid: u32,
) -> Result<(), String> {
    let launcher = app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE);
    if parent_pid <= 1
        || !processes
            .iter()
            .any(|process| process.pid == parent_pid && process.executable == launcher)
    {
        return Err("启动同步必须由当前 Dodex App 启动器直接调用。".into());
    }
    Ok(())
}

fn require_stopped(
    layout: &Layout,
    app: &Path,
    ops: &dyn MirrorOperations,
    launch_parent: Option<u32>,
) -> Result<(), String> {
    let processes = processes(&ops.processes()?)?;
    if let Some(parent) = launch_parent {
        // Recheck the parent for each snapshot, including just before publication.
        verify_launch_parent(&processes, app, parent)?;
    }
    let apps = [
        app.to_path_buf(),
        layout.system_applications.join(".Dodex/Dodex.app"),
        layout.system_applications.join("Codex B Runtime.app"),
        layout.root().join("Runtime.app"),
    ];
    let launcher = app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE);
    if processes.iter().any(|process| {
        if launch_parent == Some(process.pid) && process.executable == launcher {
            return false;
        }
        apps.iter().any(|app| {
            process.executable.starts_with(app.join("Contents/MacOS"))
                || process
                    .executable
                    .starts_with(app.join("Contents/Frameworks"))
        })
    }) {
        return Err("请先退出 Dodex 桌面 App，再同步官方版本；TUI 可以继续运行。".into());
    }
    Ok(())
}

fn validate_previous(app: &Path) -> Result<(), String> {
    no_symlinks(app)?;
    let plist = read_plist(app)?;
    if string_field(&plist, "CFBundleIdentifier")? != BUNDLE_ID
        || executable_name(&plist)? != "Dodex"
    {
        return Err("公共 Dodex.app 不是受支持的旧启动器；未覆盖任何文件。".into());
    }
    regular_file(&app.join("Contents/MacOS/Dodex"))
}

fn entry_fingerprint(app: &Path) -> Result<(String, String), String> {
    let plist = read_plist(app)?;
    Ok((
        digest(&app.join("Contents/Info.plist"))?,
        digest(&app.join("Contents/MacOS").join(executable_name(&plist)?))?,
    ))
}

fn preserve_backup(
    app: &Path,
    existing: Option<&MirrorManifest>,
    ops: &dyn MirrorOperations,
) -> Result<Option<PathBuf>, String> {
    if !exists(app) {
        return Ok(None);
    }
    let backup = app.with_file_name(".Dodex-mirror-backup.app");
    if exists(&backup) {
        no_symlinks(&backup)?;
        if existing.and_then(|manifest| manifest.backup_app.as_ref()) != Some(&backup)
            && entry_fingerprint(&backup)? != entry_fingerprint(app)?
        {
            return Err("Dodex App 备份目录已存在且不匹配；未覆盖备份或现有 App。".into());
        }
        return Ok(Some(backup));
    }
    let stage = app.with_file_name(format!(".Dodex-backup-stage-{}.app", transaction_id()));
    let cleanup = OwnedArtifacts {
        paths: vec![stage.clone()],
    };
    ops.copy(app, &stage)?;
    if entry_fingerprint(app)? != entry_fingerprint(&stage)? {
        return Err("备份期间 Dodex App 发生变化；未替换现有 App。".into());
    }
    rename_exclusive(&stage, &backup)?;
    drop(cleanup);
    Ok(Some(backup))
}

fn customize_app(app: &Path, manifest: &MirrorManifest) -> Result<(), String> {
    let plist = customize_plist(
        read_plist(app)?,
        &manifest.profile,
        &manifest.app,
        &manifest.source.executable,
        manifest.source.cli_sha256.is_some(),
        manifest.updater_executable.as_deref(),
    )?;
    let path = app.join("Contents/Info.plist");
    let bytes = serde_json::to_vec(&plist).map_err(|_| "无法生成 Dodex App 属性。")?;
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(no_follow_flag())
        .open(&path)
        .map_err(|_| "无法写入 Dodex App 属性。")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "无法写入 Dodex App 属性。")?;
    command_ok(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "xml1"])
            .arg(path),
    )?;
    write_new(
        &app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE),
        LAUNCHER,
        0o755,
    )?;
    write_new(&app.join("Contents/Resources/Dodex.icns"), ICON, 0o644)?;
    write_new(
        &app.join(MIRROR_MANIFEST),
        &serde_json::to_vec(manifest).map_err(|_| "无法保存 Dodex App 记录。")?,
        0o644,
    )?;
    let icon = app.join("Icon\r");
    if exists(&icon) {
        fs::remove_file(icon).map_err(|_| "无法清理副本的旧 Finder 图标。")?;
    }
    for attribute in ["com.apple.FinderInfo", "com.apple.ResourceFork"] {
        let _ = Command::new("/usr/bin/xattr")
            .args(["-d", attribute])
            .arg(app)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

fn sync_with(
    layout: &Layout,
    profile: &MirrorProfile,
    source: &Path,
    ops: &dyn MirrorOperations,
    notify: impl FnMut(&str, &str),
) -> Result<MirrorStatus, String> {
    sync_with_caller(layout, profile, source, ops, None, notify)
}

fn sync_with_caller(
    layout: &Layout,
    profile: &MirrorProfile,
    source: &Path,
    ops: &dyn MirrorOperations,
    launch_parent: Option<u32>,
    mut notify: impl FnMut(&str, &str),
) -> Result<MirrorStatus, String> {
    let app = target(layout)?;
    no_symlinks(&app)?;
    validate_profile(layout, profile)?;
    require_stopped(layout, &app, ops, launch_parent)?;
    let updater = updater_executable()?;
    notify("verifying", "正在验证本机官方 Codex App…");
    let fingerprint = read_source(source, ops)?;
    let existing = if exists(&app.join(MIRROR_MANIFEST)) {
        Some(validate_app(layout, &app, &app, ops)?)
    } else {
        if exists(&app) {
            validate_previous(&app)?;
        }
        None
    };
    if let Some(existing) = &existing
        && existing.source == fingerprint
        && &existing.profile == profile
        && current_packaging(existing, &updater)
    {
        return Ok(status(existing, true));
    }
    let parent = app.parent().ok_or("Dodex App 路径无效。")?;
    private_directory(parent)?;
    let stage = parent.join(format!(".Dodex-stage-{}.app", transaction_id()));
    let cleanup = OwnedArtifacts {
        paths: vec![stage.clone()],
    };
    notify("copying", "正在复制官方 App，准备独立桌面入口…");
    ops.copy(source, &stage)?;
    if read_source(&stage, ops)? != fingerprint || read_source(source, ops)? != fingerprint {
        return Err("复制期间官方 Codex 已更新；现有 Dodex 未替换，请重新同步。".into());
    }
    notify("configuring", "正在设置 Dodex 图标与现有数据目录…");
    prepare_profile(layout, profile, &app)?;
    let backup_app = preserve_backup(&app, existing.as_ref(), ops)?;
    let manifest = MirrorManifest {
        schema: MIRROR_SCHEMA,
        packaging_revision: PACKAGING_REVISION,
        updater_executable: Some(updater),
        app: app.clone(),
        profile: profile.clone(),
        source_app: source.to_path_buf(),
        source: fingerprint,
        backup_app,
    };
    customize_app(&stage, &manifest)?;
    notify("verifying", "正在验证本地 ad-hoc 签名与独立入口…");
    ops.sign(&stage)?;
    validate_app(layout, &stage, &app, ops)?;
    if read_source(source, ops)? != manifest.source {
        return Err("同步期间官方 Codex 已更新；现有 Dodex 未替换，请重新同步。".into());
    }
    require_stopped(layout, &app, ops, launch_parent)?;
    notify("finishing", "正在发布 Dodex App…");
    if exists(&app) {
        exchange(&stage, &app)?;
    } else {
        rename_exclusive(&stage, &app)?;
    }
    // After an atomic exchange the old app is at our private stage path. The
    // original public app is already preserved in the single fixed backup.
    drop(cleanup);
    ops.register(&app);
    Ok(status(&manifest, true))
}

fn exchange(left: &Path, right: &Path) -> Result<(), String> {
    no_symlinks(left)?;
    no_symlinks(right)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let left =
            std::ffi::CString::new(left.as_os_str().as_bytes()).map_err(|_| "App 路径无效。")?;
        let right =
            std::ffi::CString::new(right.as_os_str().as_bytes()).map_err(|_| "App 路径无效。")?;
        unsafe extern "C" {
            fn renamex_np(
                left: *const std::ffi::c_char,
                right: *const std::ffi::c_char,
                flags: u32,
            ) -> std::ffi::c_int;
        }
        if unsafe { renamex_np(left.as_ptr(), right.as_ptr(), 2) } != 0 {
            return Err("无法原子替换 Dodex App；现有应用未改变。".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (left, right);
        Err("Dodex App 同步仅支持 macOS。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeOps {
        copies: AtomicUsize,
        registrations: AtomicUsize,
        source_verifications: AtomicUsize,
        local_verifications: AtomicUsize,
        signatures: AtomicUsize,
        process_list: Vec<u8>,
        reject_source: bool,
        reject_signature: bool,
        reject_copy: bool,
        reject_signing: bool,
        change_source_during_copy: bool,
    }
    impl MirrorOperations for FakeOps {
        fn verify_official(&self, _: &Path) -> Result<(), String> {
            self.source_verifications.fetch_add(1, Ordering::SeqCst);
            if self.reject_source {
                Err("synthetic invalid official signature".into())
            } else {
                Ok(())
            }
        }
        fn copy(&self, source: &Path, destination: &Path) -> Result<(), String> {
            self.copies.fetch_add(1, Ordering::SeqCst);
            if self.reject_copy {
                return Err("synthetic copy failure".into());
            }
            SystemMirrorOps.copy(source, destination)?;
            if self.change_source_during_copy {
                fs::write(
                    source.join("Contents/Resources/app.asar"),
                    b"changed official release",
                )
                .unwrap();
            }
            Ok(())
        }
        fn sign(&self, _: &Path) -> Result<(), String> {
            self.signatures.fetch_add(1, Ordering::SeqCst);
            if self.reject_signing {
                return Err("synthetic signing failure".into());
            }
            Ok(())
        }
        fn verify_local(&self, _: &Path) -> Result<(), String> {
            self.local_verifications.fetch_add(1, Ordering::SeqCst);
            if self.reject_signature {
                Err("synthetic invalid local signature".into())
            } else {
                Ok(())
            }
        }
        fn processes(&self) -> Result<Vec<u8>, String> {
            Ok(self.process_list.clone())
        }
        fn register(&self, _: &Path) {
            self.registrations.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fixture {
        _temporary: tempfile::TempDir,
        layout: Layout,
        source: PathBuf,
    }
    impl Fixture {
        fn new(bundled_cli: bool) -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let mut layout = Layout::for_home(root.join("user"));
            layout.system_applications = root.join("Applications");
            let source = layout.system_applications.join("Codex.app");
            for relative in [
                "Contents/MacOS",
                "Contents/Resources",
                "Contents/_CodeSignature",
            ] {
                fs::create_dir_all(source.join(relative)).unwrap();
            }
            fs::write(
                source.join("Contents/Info.plist"),
                serde_json::to_vec(&json!({
                    "CFBundleIdentifier": "com.openai.codex",
                    "CFBundleExecutable": "ChatGPT",
                    "CFBundleShortVersionString": "26.924.22138",
                    "CFBundleVersion": "11645",
                    "CFBundleName": "ChatGPT",
                    "CFBundleIconName": "Icon",
                    "NSDockTilePlugIn": "CodexDockTilePlugin.docktileplugin",
                    "CFBundleURLTypes": [{"CFBundleURLSchemes": ["codex"]}],
                    "LSEnvironment": {"MallocNanoZone": "0"}
                }))
                .unwrap(),
            )
            .unwrap();
            command_ok(
                Command::new("/usr/bin/plutil")
                    .args(["-convert", "xml1"])
                    .arg(source.join("Contents/Info.plist")),
            )
            .unwrap();
            let executable = source.join("Contents/MacOS/ChatGPT");
            fs::write(&executable, b"synthetic executable, never run").unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(
                source.join("Contents/Resources/app.asar"),
                REQUIRED_FEATURES.concat(),
            )
            .unwrap();
            fs::write(
                source.join("Contents/_CodeSignature/CodeResources"),
                b"synthetic resource seal",
            )
            .unwrap();
            if bundled_cli {
                fs::write(
                    source.join("Contents/Resources/codex"),
                    b"synthetic cli, never run",
                )
                .unwrap();
            }
            Self {
                _temporary: temporary,
                layout,
                source,
            }
        }

        fn change_release(&self, key: &str, value: &str) {
            let mut plist = read_plist(&self.source).unwrap();
            plist[key] = json!(value);
            let path = self.source.join("Contents/Info.plist");
            fs::write(&path, serde_json::to_vec(&plist).unwrap()).unwrap();
            command_ok(
                Command::new("/usr/bin/plutil")
                    .args(["-convert", "xml1"])
                    .arg(path),
            )
            .unwrap();
        }

        fn legacy(&self) -> MirrorProfile {
            let app = self.layout.system_applications.join("Dodex.app");
            fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
            fs::write(
                app.join("Contents/Info.plist"),
                serde_json::to_vec(&json!({
                    "CFBundleIdentifier": BUNDLE_ID, "CFBundleExecutable": "Dodex"
                }))
                .unwrap(),
            )
            .unwrap();
            fs::write(
                app.join("Contents/MacOS/Dodex"),
                b"original launcher, never run",
            )
            .unwrap();
            let profile = MirrorProfile {
                codex_home: self.layout.user_home.join(".codex-second"),
                desktop_user_data: self
                    .layout
                    .user_home
                    .join("Library/Application Support/Codex-B"),
                database_dir: self.layout.user_home.join(".codex-second/sqlite"),
            };
            for path in [
                &profile.codex_home,
                &profile.desktop_user_data,
                &profile.database_dir,
            ] {
                fs::create_dir_all(path).unwrap();
            }
            profile
        }
    }

    #[test]
    fn settings_observe_mirror_metadata_but_explicit_operations_check_app_contents() {
        let fixture = Fixture::new(true);
        let profile = MirrorProfile::fresh(&fixture.layout);
        let ops = FakeOps::default();
        let installed =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        let instance = installed.instance;
        save_record(&fixture.layout, true, &instance).unwrap();
        fs::write(
            instance.runtime_app.join("Contents/Resources/app.asar"),
            b"changed synthetic App contents",
        )
        .unwrap();
        let preference = fs::read(fixture.layout.settings()).unwrap();
        let config = fs::read(instance.codex_home.join("config.toml")).unwrap();
        let runtime_ops = super::super::tests::FakeOps::default();

        // Opening/polling settings and selecting its saved profile still work.
        // Calling mirror::check on any of these paths would reject this App.
        let current = State::default();
        for _ in 0..3 {
            let observed = settings_state_for(&fixture.layout, &current);
            assert!(observed.status.enabled && observed.status.deployed);
            assert!(!observed.status.busy);
            let presentation =
                settings_presentation_for(&fixture.layout, observed.instance.as_ref());
            assert_eq!(presentation.app_path, Some(instance.launcher_app.clone()));
            assert_eq!(presentation.app_version, "26.924.22138 (11645)");
            assert_eq!(
                saved_sync_instance(&fixture.layout, &runtime_ops, false).unwrap(),
                instance
            );
            assert_eq!(
                settings_config_path_for(&fixture.layout, &runtime_ops).unwrap(),
                Some(instance.codex_home.join("config.toml"))
            );
        }
        assert!(!current.initialized && !current.status.enabled);
        assert_eq!(runtime_ops.verified.load(Ordering::SeqCst), 0);

        // Monitor/enable validation, explicit profile sync, and App sync still
        // reject changed App contents before writing profile or App files.
        assert!(validate_existing(&fixture.layout, &runtime_ops, &instance).is_err());
        assert!(
            sync_profile_file_under_lock(
                &fixture.layout,
                &runtime_ops,
                &fixture.layout.user_home.join(".codex"),
                FileKind::Config,
                Direction::ToSecondary,
            )
            .is_err()
        );
        assert!(sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).is_err());
        assert_eq!(fs::read(fixture.layout.settings()).unwrap(), preference);
        assert_eq!(
            fs::read(instance.codex_home.join("config.toml")).unwrap(),
            config
        );
    }

    #[test]
    fn missing_bundled_cli_uses_the_official_apps_own_runtime_selection() {
        let fixture = Fixture::new(false);
        let profile = MirrorProfile::fresh(&fixture.layout);
        let ops = FakeOps::default();
        let source_before = fs::read(fixture.source.join("Contents/Info.plist")).unwrap();
        let result =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        let app = &result.instance.runtime_app;
        assert_eq!(app, &result.instance.launcher_app);
        assert!(!result.instance.cli_path.exists());
        let plist = read_plist(app).unwrap();
        assert_eq!(plist["CFBundleExecutable"], LAUNCHER_EXECUTABLE);
        assert_eq!(plist["DodexNativeExecutable"], "ChatGPT");
        assert_eq!(
            fs::read(app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE)).unwrap(),
            LAUNCHER
        );
        assert_eq!(
            fs::read(app.join("Contents/MacOS/ChatGPT")).unwrap(),
            fs::read(fixture.source.join("Contents/MacOS/ChatGPT")).unwrap()
        );
        assert_eq!(plist["CFBundleIdentifier"], BUNDLE_ID);
        assert_eq!(plist["CFBundleIconFile"], "Dodex.icns");
        assert!(plist.get("CFBundleIconName").is_none());
        assert!(plist.get("NSDockTilePlugIn").is_none());
        assert!(plist.get("CFBundleURLTypes").is_none());
        assert_eq!(plist["LSEnvironment"]["MallocNanoZone"], "0");
        assert_eq!(plist["LSEnvironment"]["CODEX_CLI_PATH"], "");
        assert_eq!(plist["LSEnvironment"]["CODEX_APP_SERVER_FORCE_CLI"], "0");
        assert_eq!(
            plist["LSEnvironment"]["CODEX_APP_SERVER_USE_LOCAL_DAEMON"],
            "0"
        );
        assert_eq!(plist["LSEnvironment"]["CODEX_SPARKLE_ENABLED"], "false");
        assert_eq!(
            fs::read(fixture.source.join("Contents/Info.plist")).unwrap(),
            source_before
        );
        assert!(profile.codex_home.join("config.toml").is_file());
        assert!(is_deployed(&fixture.layout));
        assert_eq!(deployed_app(&fixture.layout), Some(app.clone()));
        validate_app(&fixture.layout, app, app, &ops).unwrap();
        let again = sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        assert!(again.up_to_date);
        assert_eq!(ops.copies.load(Ordering::SeqCst), 1);
        assert_eq!(ops.registrations.load(Ordering::SeqCst), 1);

        let record = monitor_after_app_sync(&fixture.layout, None, &result.instance).unwrap();
        assert!(!record.enabled);
        let settings = fs::read(fixture.layout.settings()).unwrap();
        let config = fs::read(profile.codex_home.join("config.toml")).unwrap();
        let presentation = settings_presentation_for(&fixture.layout, Some(&record.instance));
        assert_eq!(presentation.app_path, Some(app.clone()));
        assert_eq!(presentation.app_version, "26.924.22138 (11645)");
        assert_eq!(presentation.profile_home, Some(profile.codex_home.clone()));
        assert!(!presentation.tui_available);
        assert_eq!(fs::read(fixture.layout.settings()).unwrap(), settings);
        assert_eq!(
            fs::read(profile.codex_home.join("config.toml")).unwrap(),
            config
        );

        // A retained preference/manifest is not evidence of an installed App
        // when its executable has disappeared. The saved home stays visible.
        fs::remove_file(app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE)).unwrap();
        let presentation = settings_presentation_for(&fixture.layout, Some(&record.instance));
        assert!(presentation.app_path.is_none() && presentation.app_version.is_empty());
        assert_eq!(presentation.profile_home, Some(profile.codex_home));
        assert!(!presentation.tui_available);
    }

    #[test]
    fn bootstrap_execs_same_bundle_with_only_the_selected_browser_profile() {
        let fixture = Fixture::new(false);
        let mut profile = MirrorProfile::fresh(&fixture.layout);
        profile.desktop_user_data = fixture
            .layout
            .user_home
            .join("Library/Application Support/Dodex's $HOME `data` $(data)");
        let native_name = "ChatGPT's executable";
        let mut plist = read_plist(&fixture.source).unwrap();
        plist["CFBundleExecutable"] = json!(native_name);
        fs::write(
            fixture.source.join("Contents/Info.plist"),
            serde_json::to_vec(&plist).unwrap(),
        )
        .unwrap();
        let native = fixture.source.join("Contents/MacOS").join(native_name);
        fs::rename(fixture.source.join("Contents/MacOS/ChatGPT"), &native).unwrap();
        fs::write(
            &native,
            b"#!/bin/sh\nprintf '%s\\0' \"$$\" \"$0\" \"$@\"\nexit 37\n",
        )
        .unwrap();
        let result = sync_with(
            &fixture.layout,
            &profile,
            &fixture.source,
            &FakeOps::default(),
            |_, _| {},
        )
        .unwrap();
        let app = result.instance.runtime_app;
        command_ok(
            Command::new("/usr/bin/plutil")
                .args([
                    "-replace",
                    "DodexUpdaterExecutable",
                    "-string",
                    "/usr/bin/true",
                ])
                .arg(app.join("Contents/Info.plist")),
        )
        .unwrap();
        let child = Command::new(app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE))
            .args([
                "--user-data-dir=/wrong-profile",
                "--",
                "a caller-supplied argument",
            ])
            .env("CODEX_ELECTRON_USER_DATA_PATH", "/wrong-inherited-profile")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            output.stdout,
            format!(
                "{pid}\0{}\0--user-data-dir={}\0",
                app.join("Contents/MacOS").join(native_name).display(),
                profile.desktop_user_data.display(),
            )
            .as_bytes()
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn updating_shared_profile_preserves_tui_and_one_original_public_backup() {
        let fixture = Fixture::new(true);
        let profile = fixture.legacy();
        let config = profile.codex_home.join("config.toml");
        let auth = profile.codex_home.join("auth.json");
        let manager = profile.desktop_user_data.join("tools/manager.py");
        let tui = fixture.layout.user_home.join(".local/bin/dodex");
        let hidden = fixture
            .layout
            .system_applications
            .join(".Dodex/Dodex.app/Contents/Resources/codex");
        for path in [&config, &auth, &manager, &tui, &hidden] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"must remain byte-for-byte unchanged").unwrap();
        }
        let ops = FakeOps {
            process_list: format!("100 {}\n101 {}\n", tui.display(), hidden.display()).into_bytes(),
            ..FakeOps::default()
        };
        let result =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        let app = &result.instance.runtime_app;
        let backup = result.backup_app.unwrap();
        assert_eq!(
            fs::read(backup.join("Contents/MacOS/Dodex")).unwrap(),
            b"original launcher, never run"
        );
        let env = read_plist(app).unwrap()["LSEnvironment"].clone();
        assert_eq!(
            env["CODEX_HOME"],
            profile.codex_home.to_string_lossy().as_ref()
        );
        assert_eq!(
            env["CODEX_CLI_PATH"],
            app.join("Contents/Resources/codex")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(env["CODEX_APP_SERVER_FORCE_CLI"], "1");
        let mut source_plist = read_plist(&fixture.source).unwrap();
        source_plist["CFBundleVersion"] = json!("11646");
        fs::write(
            fixture.source.join("Contents/Info.plist"),
            serde_json::to_vec(&source_plist).unwrap(),
        )
        .unwrap();
        let updated =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        assert_eq!(updated.source_build, "11646");
        assert_eq!(updated.backup_app, Some(backup.clone()));
        assert_eq!(
            fs::read(backup.join("Contents/MacOS/Dodex")).unwrap(),
            b"original launcher, never run"
        );
        for path in [&config, &auth, &manager, &tui, &hidden] {
            assert_eq!(
                fs::read(path).unwrap(),
                b"must remain byte-for-byte unchanged"
            );
        }
        assert_eq!(ops.copies.load(Ordering::SeqCst), 3); // two releases, one backup
    }

    #[test]
    fn source_change_or_invalid_signatures_never_publish_the_stage() {
        for ops in [
            FakeOps {
                change_source_during_copy: true,
                ..FakeOps::default()
            },
            FakeOps {
                reject_source: true,
                ..FakeOps::default()
            },
            FakeOps {
                reject_signature: true,
                ..FakeOps::default()
            },
        ] {
            let fixture = Fixture::new(false);
            let profile = MirrorProfile::fresh(&fixture.layout);
            assert!(
                sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).is_err()
            );
            assert!(!target(&fixture.layout).unwrap().exists());
            assert_eq!(ops.registrations.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn running_desktop_blocks_update_and_cli_paths_do_not() {
        let fixture = Fixture::new(false);
        let profile = fixture.legacy();
        for path in [
            fixture
                .layout
                .system_applications
                .join("Dodex.app/Contents/MacOS/DodexLauncher"),
            fixture
                .layout
                .system_applications
                .join("Dodex.app/Contents/MacOS/ChatGPT"),
            fixture
                .layout
                .system_applications
                .join(".Dodex/Dodex.app/Contents/MacOS/ChatGPT"),
            fixture
                .layout
                .system_applications
                .join("Dodex.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper"),
        ] {
            let ops = FakeOps {
                process_list: format!("100 {}\n", path.display()).into_bytes(),
                ..FakeOps::default()
            };
            assert!(
                sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {})
                    .unwrap_err()
                    .contains("退出 Dodex")
            );
            assert_eq!(ops.copies.load(Ordering::SeqCst), 0);
        }
    }

    fn launch_ops(app: &Path) -> FakeOps {
        FakeOps {
            process_list: format!(
                "  123 {}\n",
                app.join("Contents/MacOS")
                    .join(LAUNCHER_EXECUTABLE)
                    .display()
            )
            .into_bytes(),
            ..FakeOps::default()
        }
    }

    #[test]
    fn launch_with_matching_release_only_reads_metadata() {
        let fixture = Fixture::new(false);
        let profile = MirrorProfile::fresh(&fixture.layout);
        let installed = sync_with(
            &fixture.layout,
            &profile,
            &fixture.source,
            &FakeOps::default(),
            |_, _| {},
        )
        .unwrap();
        let app = installed.instance.runtime_app;
        let manifest_before = fs::read(app.join(MIRROR_MANIFEST)).unwrap();
        // A same-release launch must not read the archive or run either verifier.
        fs::remove_file(fixture.source.join("Contents/Resources/app.asar")).unwrap();
        let ops = FakeOps {
            reject_source: true,
            reject_signature: true,
            ..launch_ops(&app)
        };
        let result = sync_on_launch_with(&fixture.layout, &app, 123, &ops).unwrap();
        assert!(result.up_to_date);
        assert_eq!(ops.copies.load(Ordering::SeqCst), 0);
        assert_eq!(ops.signatures.load(Ordering::SeqCst), 0);
        assert_eq!(ops.source_verifications.load(Ordering::SeqCst), 0);
        assert_eq!(ops.local_verifications.load(Ordering::SeqCst), 0);
        assert_eq!(ops.registrations.load(Ordering::SeqCst), 0);
        assert_eq!(
            fs::read(app.join(MIRROR_MANIFEST)).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn launch_updates_a_changed_version_or_build_and_preserves_the_shared_profile() {
        for (key, value) in [
            ("CFBundleShortVersionString", "26.925.1"),
            ("CFBundleVersion", "11646"),
        ] {
            let fixture = Fixture::new(false);
            let profile = fixture.legacy();
            let config = profile.codex_home.join("config.toml");
            let tui = fixture.layout.user_home.join(".local/bin/dodex");
            for path in [&config, &tui] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"existing shared profile and TUI").unwrap();
            }
            let installed = sync_with(
                &fixture.layout,
                &profile,
                &fixture.source,
                &FakeOps::default(),
                |_, _| {},
            )
            .unwrap();
            let app = installed.instance.runtime_app;
            fixture.change_release(key, value);
            let ops = launch_ops(&app);
            let updated = sync_on_launch_with(&fixture.layout, &app, 123, &ops).unwrap();
            let plist = read_plist(&app).unwrap();
            assert_eq!(plist[key], value);
            assert_eq!(updated.backup_app, installed.backup_app);
            assert_eq!(read_manifest(&app).unwrap().profile, profile);
            assert_eq!(ops.copies.load(Ordering::SeqCst), 1);
            assert_eq!(ops.signatures.load(Ordering::SeqCst), 1);
            assert_eq!(ops.registrations.load(Ordering::SeqCst), 1);
            for path in [&config, &tui] {
                assert_eq!(fs::read(path).unwrap(), b"existing shared profile and TUI");
            }
        }
    }

    #[test]
    fn launch_errors_leave_the_installed_app_intact() {
        for failure in ["missing", "source", "local", "copy", "sign", "changed"] {
            let fixture = Fixture::new(false);
            let profile = MirrorProfile::fresh(&fixture.layout);
            let installed = sync_with(
                &fixture.layout,
                &profile,
                &fixture.source,
                &FakeOps::default(),
                |_, _| {},
            )
            .unwrap();
            let app = installed.instance.runtime_app;
            let before = [
                MIRROR_MANIFEST,
                "Contents/Info.plist",
                "Contents/MacOS/ChatGPT",
            ]
            .map(|path| (path, fs::read(app.join(path)).unwrap()));
            fixture.change_release("CFBundleVersion", "11646");
            if failure == "missing" {
                fs::remove_dir_all(&fixture.source).unwrap();
            }
            let ops = FakeOps {
                reject_source: failure == "source",
                reject_signature: failure == "local",
                reject_copy: failure == "copy",
                reject_signing: failure == "sign",
                change_source_during_copy: failure == "changed",
                ..launch_ops(&app)
            };
            assert!(sync_on_launch_with(&fixture.layout, &app, 123, &ops).is_err());
            for (path, bytes) in before {
                assert_eq!(
                    fs::read(app.join(path)).unwrap(),
                    bytes,
                    "{failure}: {path}"
                );
            }
            assert_eq!(ops.registrations.load(Ordering::SeqCst), 0);
            assert!(fs::read_dir(app.parent().unwrap()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".Dodex-stage-")
            }));
        }
    }

    #[test]
    fn launch_rejects_other_bootstraps_native_processes_and_unverified_callers() {
        let fixture = Fixture::new(false);
        let profile = MirrorProfile::fresh(&fixture.layout);
        let installed = sync_with(
            &fixture.layout,
            &profile,
            &fixture.source,
            &FakeOps::default(),
            |_, _| {},
        )
        .unwrap();
        let app = installed.instance.runtime_app;
        fixture.change_release("CFBundleVersion", "11646");
        for relative in [
            "Contents/MacOS/DodexLauncher",
            "Contents/MacOS/ChatGPT",
            "Contents/Frameworks/Helper.app/Contents/MacOS/Helper",
        ] {
            let mut ops = launch_ops(&app);
            ops.process_list
                .extend_from_slice(format!("124 {}\n", app.join(relative).display()).as_bytes());
            assert!(
                sync_on_launch_with(&fixture.layout, &app, 123, &ops)
                    .unwrap_err()
                    .contains("退出 Dodex")
            );
            assert_eq!(ops.copies.load(Ordering::SeqCst), 0);
        }
        for (path, pid) in [(app.clone(), 124), (app.with_file_name("Other.app"), 123)] {
            let ops = launch_ops(&app);
            assert!(sync_on_launch_with(&fixture.layout, &path, pid, &ops).is_err());
            assert_eq!(ops.source_verifications.load(Ordering::SeqCst), 0);
        }
        let ops = FakeOps {
            process_list: format!("123 {}\n", app.join("Contents/MacOS/ChatGPT").display())
                .into_bytes(),
            ..FakeOps::default()
        };
        assert!(sync_on_launch_with(&fixture.layout, &app, 123, &ops).is_err());
        assert_eq!(ops.source_verifications.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn legacy_packaging_and_a_moved_updater_are_rebuilt_once() {
        for legacy in [true, false] {
            let fixture = Fixture::new(false);
            let profile = MirrorProfile::fresh(&fixture.layout);
            let installed = sync_with(
                &fixture.layout,
                &profile,
                &fixture.source,
                &FakeOps::default(),
                |_, _| {},
            )
            .unwrap();
            let app = installed.instance.runtime_app;
            let mut manifest: Value = read_json(&app.join(MIRROR_MANIFEST)).unwrap();
            let mut plist = read_plist(&app).unwrap();
            if legacy {
                manifest
                    .as_object_mut()
                    .unwrap()
                    .remove("packaging_revision");
                manifest
                    .as_object_mut()
                    .unwrap()
                    .remove("updater_executable");
                plist
                    .as_object_mut()
                    .unwrap()
                    .remove("DodexUpdaterExecutable");
            } else {
                manifest["updater_executable"] = json!("/previous/agent-companion");
                plist["DodexUpdaterExecutable"] = json!("/previous/agent-companion");
            }
            fs::write(
                app.join(MIRROR_MANIFEST),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::write(
                app.join("Contents/Info.plist"),
                serde_json::to_vec(&plist).unwrap(),
            )
            .unwrap();
            let ops = FakeOps::default();
            let old = validate_app(&fixture.layout, &app, &app, &ops).unwrap();
            assert_eq!(old.schema, MIRROR_SCHEMA);
            assert_eq!(
                old.packaging_revision,
                if legacy { 0 } else { PACKAGING_REVISION }
            );
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
            let updated = validate_app(&fixture.layout, &app, &app, &ops).unwrap();
            assert!(current_packaging(
                &updated,
                &std::env::current_exe().unwrap()
            ));
            assert_eq!(updated.profile, profile);
            assert_eq!(ops.copies.load(Ordering::SeqCst), 2); // refreshed app and one backup
            assert_eq!(ops.signatures.load(Ordering::SeqCst), 1);
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
            assert_eq!(ops.copies.load(Ordering::SeqCst), 2);
            assert_eq!(ops.signatures.load(Ordering::SeqCst), 1);
            assert_eq!(ops.registrations.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn private_profile_and_icon_tampering_fail_validation() {
        let fixture = Fixture::new(false);
        let mut profile = MirrorProfile::fresh(&fixture.layout);
        profile.codex_home = fixture.layout.user_home.join(".codex");
        assert!(validate_profile(&fixture.layout, &profile).is_err());
        let profile = MirrorProfile::fresh(&fixture.layout);
        let ops = FakeOps::default();
        let result =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        let app = &result.instance.runtime_app;
        fs::write(app.join("Contents/Resources/Dodex.icns"), b"changed icon").unwrap();
        assert!(validate_app(&fixture.layout, app, app, &ops).is_err());
    }

    #[test]
    fn nonexecutable_bootstrap_or_changed_native_target_fails_validation() {
        let fixture = Fixture::new(false);
        let profile = MirrorProfile::fresh(&fixture.layout);
        let ops = FakeOps::default();
        let result =
            sync_with(&fixture.layout, &profile, &fixture.source, &ops, |_, _| {}).unwrap();
        let app = &result.instance.runtime_app;
        let launcher = app.join("Contents/MacOS").join(LAUNCHER_EXECUTABLE);
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(validate_app(&fixture.layout, app, app, &ops).is_err());
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        let mut plist = read_plist(app).unwrap();
        plist["DodexNativeExecutable"] = json!("wrong-target");
        fs::write(
            app.join("Contents/Info.plist"),
            serde_json::to_vec(&plist).unwrap(),
        )
        .unwrap();
        assert!(validate_app(&fixture.layout, app, app, &ops).is_err());
    }

    #[test]
    fn local_signature_drops_official_identity_and_shared_access_entitlements() {
        let value = sanitized_entitlements(json!({
            "com.apple.application-identifier": "official.app",
            "com.apple.developer.team-identifier": "official-team",
            "com.apple.developer.aps-environment": "production",
            "keychain-access-groups": ["official.*"],
            "com.apple.security.application-groups": ["official.group"],
            "com.apple.security.cs.allow-jit": true,
            "com.apple.security.automation.apple-events": true,
            "com.apple.security.app-sandbox": false
        }))
        .unwrap();
        assert_eq!(
            value,
            json!({
                "com.apple.security.cs.allow-jit": true,
                "com.apple.security.automation.apple-events": true,
                "com.apple.security.app-sandbox": false
            })
        );
    }
}
