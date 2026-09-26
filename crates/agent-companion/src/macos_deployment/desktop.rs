//! Explicit desktop-only repair of the original Codex-B installation.
//! The old runtime, manager, CLI wrappers and profile contents are never changed.
use super::*;

const PENDING: &str = "companion-desktop-repair-pending.json";
const STAGE: &str = "Staged Launcher.app";
const BACKUP: &str = "Original Launcher.app";

#[derive(Serialize, Deserialize)]
struct PendingRepair {
    schema: u32,
    instance: InstanceConfig,
    bundle: PathBuf,
}

pub(super) fn repair_root(layout: &Layout) -> PathBuf {
    layout.system_applications.join(".Dodex")
}
fn repaired_instance(layout: &Layout) -> InstanceConfig {
    let runtime = repair_root(layout).join("Dodex.app");
    InstanceConfig {
        id: "dodex".into(),
        label: "Dodex".into(),
        codex_home: layout.user_home.join(".codex-second"),
        desktop_user_data: layout.user_home.join("Library/Application Support/Codex-B"),
        database_dir: layout.user_home.join(".codex-second/sqlite"),
        cli_path: runtime.join("Contents/Resources/codex"),
        runtime_app: runtime,
        launcher_app: layout.system_applications.join("Dodex.app"),
    }
}
fn original_instance(layout: &Layout) -> InstanceConfig {
    let mut instance = repaired_instance(layout);
    instance.runtime_app = layout.system_applications.join("Codex B Runtime.app");
    instance.cli_path = instance.runtime_app.join("Contents/Resources/codex");
    instance
}

// Unlike canonicalize(), this also accepts the known alias while its target is
// temporarily in the backup location during an interrupted publication.
fn publication_bundle(layout: &Layout) -> Result<PathBuf, String> {
    no_symlinks(&layout.system_applications)?;
    let launcher = layout.system_applications.join("Dodex.app");
    if fs::symlink_metadata(&launcher).is_ok_and(|meta| meta.file_type().is_symlink()) {
        let target = fs::read_link(&launcher).map_err(|_| "无法读取 Dodex 启动别名。")?;
        let target = if target.is_absolute() {
            target
        } else {
            layout.system_applications.join(target)
        };
        if target != layout.system_applications.join("Codex B.app") {
            return Err("Dodex 启动别名发生变化；保留修复备份，未覆盖文件。".into());
        }
        no_symlinks(&target)?;
        Ok(target)
    } else {
        no_symlinks(&launcher)?;
        Ok(launcher)
    }
}

fn validate_backup(layout: &Layout, ops: &dyn Operations, backup: &Path) -> Result<(), String> {
    let manager = original_instance(layout)
        .desktop_user_data
        .join("tools/codex_b_manager.py");
    let executable = backup.join("Contents/MacOS/Dodex");
    if !ops.legacy_fingerprints_match(&executable, &manager) {
        return Err("Dodex 原启动器备份无法验证；未覆盖任何文件。".into());
    }
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
        return Err("Dodex 原启动器备份绑定了不同用户目录。".into());
    }
    Ok(())
}

fn publish_pending(layout: &Layout, ops: &dyn Operations) -> Result<InstanceConfig, String> {
    let root = repair_root(layout);
    let pending: PendingRepair = read_json(&root.join(PENDING))?;
    let instance = repaired_instance(layout);
    if pending.schema != SCHEMA
        || pending.instance != instance
        || pending.bundle != publication_bundle(layout)?
    {
        return Err("Dodex 中断修复清单不兼容；保留备份，未覆盖文件。".into());
    }
    validate_instance_paths(layout, &instance)?;
    validate_config(&instance.codex_home.join("config.toml"), &instance)?;
    ops.verify_runtime(&instance.runtime_app)?;
    let bundle = &pending.bundle;
    let stage = root.join(STAGE);
    let backup = root.join(BACKUP);
    if exists(&backup) {
        validate_backup(layout, ops, &backup)?;
    }
    if exists(bundle) && validate_entry(layout, &instance, bundle).is_ok() {
        if !exists(&backup) || exists(&stage) {
            return Err("Dodex 修复发布状态有冲突；保留备份，未覆盖文件。".into());
        }
    } else {
        validate_entry(layout, &instance, &stage)?;
        if exists(bundle) {
            if exists(&backup) {
                return Err("Dodex 启动目标与备份同时存在；未覆盖任何文件。".into());
            }
            validate_legacy(layout, ops)?;
            rename_exclusive(bundle, &backup)?;
        } else if !exists(&backup) {
            return Err("Dodex 原启动器和备份均缺失；未发布新启动器。".into());
        }
        if let Err(error) = rename_exclusive(&stage, bundle) {
            if rename_exclusive(&backup, bundle).is_err() {
                return Err(format!(
                    "{error} 原启动器保留在 {}，请恢复后重试。",
                    backup.display()
                ));
            }
            return Err(error);
        }
    }
    // Publish completion only after the real launcher is present. The pending
    // record and original backup survive every earlier error or process exit.
    validate_entry(layout, &instance, bundle)?;
    write_new_atomic(
        &root.join(MANIFEST),
        &serde_json::to_vec(&Manifest {
            schema: SCHEMA,
            instance: instance.clone(),
        })
        .map_err(|_| "无法保存 Dodex 桌面修复清单。")?,
        0o600,
    )?;
    let result = validate_repaired(layout, ops, true, true)?;
    fs::remove_file(root.join(PENDING)).map_err(|_| "修复已完成，但无法清理中断修复记录。")?;
    Ok(result)
}
fn desktop_plist() -> String {
    LAUNCHER_PLIST.replace(
        "<key>CFBundleVersion</key><string>1</string>",
        "<key>CFBundleVersion</key><string>2</string>\n<key>CFBundleIconFile</key><string>Dodex.icns</string>",
    )
}
fn validate_entry(layout: &Layout, instance: &InstanceConfig, bundle: &Path) -> Result<(), String> {
    if read_limited(&bundle.join("Contents/Info.plist"), 8192)? != desktop_plist().as_bytes()
        || read_limited(&bundle.join("Contents/MacOS/Dodex"), 32768)?
            != launcher_text(layout, instance).as_bytes()
    {
        return Err("Dodex 桌面启动器与独立目录不一致；未修改现有环境。".into());
    }
    if fs::metadata(bundle.join("Contents/MacOS/Dodex"))
        .map_err(|_| "无法检查 Dodex 桌面启动权限。")?
        .permissions()
        .mode()
        & 0o111
        == 0
    {
        return Err("Dodex 桌面启动器不可执行。".into());
    }
    Ok(())
}
pub(super) fn validate_repaired(
    layout: &Layout,
    ops: &dyn Operations,
    require_config: bool,
    runtime: bool,
) -> Result<InstanceConfig, String> {
    let instance = repaired_instance(layout);
    let manifest: Manifest = read_json(&repair_root(layout).join(MANIFEST))?;
    if manifest.schema != SCHEMA || manifest.instance != instance {
        return Err("Dodex 桌面修复清单不兼容；未修改现有环境。".into());
    }
    validate_instance_paths(layout, &instance)?;
    let config = instance.codex_home.join("config.toml");
    if require_config || exists(&config) {
        validate_config(&config, &instance)?;
    }
    validate_entry(layout, &instance, &legacy_launcher_bundle(layout)?)?;
    if runtime {
        ops.verify_runtime(&instance.runtime_app)?;
    }
    Ok(instance)
}
fn desktop_running(output: &[u8], runtimes: &[&Path]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        runtimes
            .iter()
            .any(|app| line.trim() == app.join("Contents/MacOS/ChatGPT").to_string_lossy())
    })
}
fn require_desktop_stopped(layout: &Layout) -> Result<(), String> {
    let result = Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
        .map_err(|_| "无法检查 Dodex 桌面进程。")?;
    if !result.status.success() {
        return Err("无法检查 Dodex 桌面进程；未修改启动器。".into());
    }
    if desktop_running(
        &result.stdout,
        &[
            &layout.system_applications.join("Codex B Runtime.app"),
            &repaired_instance(layout).runtime_app,
        ],
    ) {
        return Err("请先退出 Dodex 桌面 App，再修复启动器；TUI 可以继续运行。".into());
    }
    Ok(())
}
fn repair_with(layout: &Layout, ops: &dyn Operations) -> Result<InstanceConfig, String> {
    if exists(&repair_root(layout).join(MANIFEST)) {
        let instance = validate_repaired(layout, ops, true, true)?;
        let root = repair_root(layout);
        if exists(&root.join(PENDING)) {
            let pending: PendingRepair = read_json(&root.join(PENDING))?;
            if pending.schema != SCHEMA
                || pending.instance != instance
                || pending.bundle != publication_bundle(layout)?
                || exists(&root.join(STAGE))
            {
                return Err("Dodex 已完成修复与中断记录冲突；保留全部文件，未清理记录。".into());
            }
            validate_backup(layout, ops, &root.join(BACKUP))?;
            fs::remove_file(root.join(PENDING))
                .map_err(|_| "修复已完成，但无法清理中断修复记录。")?;
        }
        return Ok(instance);
    }
    if exists(&repair_root(layout).join(PENDING)) {
        return publish_pending(layout, ops);
    }
    let original = validate_legacy(layout, ops)?;
    let instance = repaired_instance(layout);
    let root = repair_root(layout);
    if exists(&root) {
        return Err("Dodex 桌面修复目录已存在但不完整；请保留备份并检查，未覆盖文件。".into());
    }
    let bundle = legacy_launcher_bundle(layout)?;
    let stage = root.join(STAGE);
    // The signed runtime is copied, not renamed: running TUI processes retain
    // their original executable, and existing CLI/update tools retain all paths.
    create_private(&root)?;
    let mut cleanup = OwnedArtifacts {
        paths: vec![root.clone()],
    };
    ops.copy_runtime(&original.runtime_app, &instance.runtime_app)?;
    ops.verify_runtime(&instance.runtime_app)?;
    create_private(&stage)?;
    cleanup.paths.push(stage.clone());
    private_directory(&stage.join("Contents/MacOS"))?;
    private_directory(&stage.join("Contents/Resources"))?;
    write_new(
        &stage.join("Contents/Info.plist"),
        desktop_plist().as_bytes(),
        0o644,
    )?;
    write_new(
        &stage.join("Contents/MacOS/Dodex"),
        launcher_text(layout, &instance).as_bytes(),
        0o755,
    )?;
    let icon = bundle.join("Contents/Resources/Dodex.icns");
    if exists(&icon) {
        let bytes = read_limited(&icon, 8 * 1024 * 1024)?;
        write_new(&stage.join("Contents/Resources/Dodex.icns"), &bytes, 0o644)?;
    }
    validate_entry(layout, &instance, &stage)?;
    write_new_atomic(
        &root.join(PENDING),
        &serde_json::to_vec(&PendingRepair {
            schema: SCHEMA,
            instance: instance.clone(),
            bundle,
        })
        .map_err(|_| "无法保存 Dodex 桌面修复清单。")?,
        0o600,
    )?;
    // From this point the exact pending record owns the staged files. Only the
    // publication phase supports automatic crash recovery. A hard termination
    // before this journal exists leaves files for inspection rather than guessing
    // their ownership on retry. Ordinary errors before this point clean our files.
    cleanup.paths.clear();
    publish_pending(layout, ops)
}

fn repair_saved_with(layout: &Layout, ops: &dyn Operations) -> Result<InstanceConfig, String> {
    let saved: Option<Record> = if exists(&layout.settings()) {
        Some(read_json(&layout.settings())?)
    } else {
        None
    };
    if let Some(record) = &saved
        && (record.schema != SCHEMA
            || (record.instance != original_instance(layout)
                && record.instance != repaired_instance(layout)))
    {
        return Err("已有 Companion 记录与 Dodex 不一致；未修改启动器。".into());
    }
    // An exact original record is also accepted after publication: its atomic
    // migration may have failed or been interrupted on the preceding attempt.
    let instance = repair_with(layout, ops)?;
    if let Some(record) = saved {
        save_record(layout, record.enabled, &instance)?;
    }
    Ok(instance)
}

fn managed_entry_message(
    layout: &Layout,
    ops: &dyn Operations,
    repair: bool,
) -> Result<Option<String>, String> {
    if !exists(&layout.settings()) {
        return Ok(None);
    }
    let record: Record = read_json(&layout.settings())?;
    if record.instance != layout.instance() {
        return Ok(None);
    }
    if record.schema != SCHEMA {
        return Err("已有 Companion 部署记录版本不兼容；未修改任何文件。".into());
    }
    validate_managed(layout, ops, &record.instance)?;
    Ok(Some(if repair {
        format!(
            "Companion 管理的 Dodex 桌面校验通过，无需修复：{}。现有程序、配置和数据保持不变。",
            record.instance.launcher_app.display()
        )
    } else {
        format!(
            "Dodex 桌面校验通过：{}",
            record.instance.runtime_app.display()
        )
    }))
}

/// `dodex-app` checks only; `dodex-app --repair` mutates only the legacy entry.
/// It neither opens an app nor changes the manager, shell command or credentials.
pub fn desktop_entry(repair: bool) -> Result<String, String> {
    let layout = Layout::current()?;
    if let Some(message) = managed_entry_message(&layout, &SystemOps, repair)? {
        return Ok(message);
    }
    if !repair {
        let instance = validate_legacy(&layout, &SystemOps)?;
        return Ok(format!(
            "Dodex 桌面校验通过：{}",
            instance.runtime_app.display()
        ));
    }
    require_desktop_stopped(&layout)?;
    private_directory(&layout.support)?;
    let _lock = DeploymentLock::acquire(&layout.support.join("deployment.lock"))?;
    let instance = repair_saved_with(&layout, &SystemOps)?;
    Ok(format!(
        "Dodex 桌面启动器已修复：{}。原启动器备份：{}。TUI 命令、管理器、原运行程序和账号目录保持不变。",
        instance.launcher_app.display(),
        repair_root(&layout).join("Original Launcher.app").display()
    ))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{FakeOps, legacy_fixture};
    use super::*;

    fn make_alias(layout: &Layout) {
        let entry = layout.system_applications.join("Dodex.app");
        let target = layout.system_applications.join("Codex B.app");
        fs::rename(&entry, &target).unwrap();
        std::os::unix::fs::symlink("Codex B.app", entry).unwrap();
    }

    // Reconstruct the three durable states around the two publication renames.
    fn interrupt_publication(layout: &Layout, ops: &dyn Operations, phase: u8) {
        let bundle = legacy_launcher_bundle(layout).unwrap();
        let instance = repair_with(layout, ops).unwrap();
        let root = repair_root(layout);
        fs::remove_file(root.join(MANIFEST)).unwrap();
        if phase < 2 {
            fs::rename(&bundle, root.join(STAGE)).unwrap();
        }
        if phase == 0 {
            fs::rename(root.join(BACKUP), &bundle).unwrap();
        }
        write_new(
            &root.join(PENDING),
            &serde_json::to_vec(&PendingRepair {
                schema: SCHEMA,
                instance,
                bundle,
            })
            .unwrap(),
            0o600,
        )
        .unwrap();
    }

    #[test]
    fn retries_each_publication_phase_for_direct_and_alias_launchers() {
        for alias in [false, true] {
            for phase in 0..3 {
                let (fixture, old) = legacy_fixture();
                if alias {
                    make_alias(&fixture.layout);
                }
                let ops = FakeOps {
                    legacy: true,
                    ..FakeOps::default()
                };
                let manager = old.desktop_user_data.join("tools/codex_b_manager.py");
                let manager_before = fs::read(&manager).unwrap();
                let config_before = fs::read(old.codex_home.join("config.toml")).unwrap();
                interrupt_publication(&fixture.layout, &ops, phase);
                let new = repair_with(&fixture.layout, &ops).unwrap();
                assert_eq!(new, validate_legacy(&fixture.layout, &ops).unwrap());
                assert!(repair_root(&fixture.layout).join(BACKUP).is_dir());
                assert!(!repair_root(&fixture.layout).join(PENDING).exists());
                assert_eq!(fs::read(manager).unwrap(), manager_before);
                assert_eq!(
                    fs::read(old.codex_home.join("config.toml")).unwrap(),
                    config_before
                );
                assert!(old.cli_path.is_file());
                assert_eq!(ops.copied.load(std::sync::atomic::Ordering::SeqCst), 1);
                if alias {
                    assert_eq!(
                        fs::read_link(new.launcher_app).unwrap(),
                        Path::new("Codex B.app")
                    );
                }
            }
        }
    }

    #[test]
    fn retries_saved_original_record_after_launcher_publication() {
        for enabled in [false, true] {
            let (fixture, old) = legacy_fixture();
            make_alias(&fixture.layout);
            let ops = FakeOps {
                legacy: true,
                ..FakeOps::default()
            };
            save_record(&fixture.layout, enabled, &old).unwrap();
            let new = repair_with(&fixture.layout, &ops).unwrap();
            let saved: Record = read_json(&fixture.layout.settings()).unwrap();
            assert_eq!(saved.instance, old);
            assert_eq!(repair_saved_with(&fixture.layout, &ops).unwrap(), new);
            let saved: Record = read_json(&fixture.layout.settings()).unwrap();
            assert_eq!(saved.instance, new);
            assert_eq!(saved.enabled, enabled);
        }
    }

    fn restore_completed_pending(layout: &Layout, instance: &InstanceConfig) {
        write_new(
            &repair_root(layout).join(PENDING),
            &serde_json::to_vec(&PendingRepair {
                schema: SCHEMA,
                instance: instance.clone(),
                bundle: publication_bundle(layout).unwrap(),
            })
            .unwrap(),
            0o600,
        )
        .unwrap();
    }

    #[test]
    fn completed_publication_cleans_pending_record_on_retry() {
        for alias in [false, true] {
            let (fixture, _) = legacy_fixture();
            if alias {
                make_alias(&fixture.layout);
            }
            let ops = FakeOps {
                legacy: true,
                ..FakeOps::default()
            };
            let instance = repair_with(&fixture.layout, &ops).unwrap();
            let manifest = fs::read(repair_root(&fixture.layout).join(MANIFEST)).unwrap();
            restore_completed_pending(&fixture.layout, &instance);
            assert_eq!(repair_with(&fixture.layout, &ops).unwrap(), instance);
            assert!(!repair_root(&fixture.layout).join(PENDING).exists());
            assert!(repair_root(&fixture.layout).join(BACKUP).is_dir());
            assert_eq!(
                fs::read(repair_root(&fixture.layout).join(MANIFEST)).unwrap(),
                manifest
            );
            assert_eq!(ops.copied.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn completed_publication_preserves_conflicting_pending_records_and_stages() {
        for conflict in ["schema", "instance", "bundle", "stage", "backup"] {
            let (fixture, _) = legacy_fixture();
            let ops = FakeOps {
                legacy: true,
                ..FakeOps::default()
            };
            let instance = repair_with(&fixture.layout, &ops).unwrap();
            restore_completed_pending(&fixture.layout, &instance);
            let root = repair_root(&fixture.layout);
            let path = root.join(PENDING);
            let mut pending: PendingRepair = read_json(&path).unwrap();
            match conflict {
                "schema" => pending.schema += 1,
                "instance" => pending.instance.codex_home = fixture.layout.user_home.join("other"),
                "bundle" => pending.bundle = fixture.layout.system_applications.join("Other.app"),
                "stage" => fs::create_dir(root.join(STAGE)).unwrap(),
                "backup" => {
                    fs::rename(root.join(BACKUP), root.join("Preserved Backup.app")).unwrap()
                }
                _ => unreachable!(),
            }
            fs::write(&path, serde_json::to_vec(&pending).unwrap()).unwrap();
            let before = fs::read(&path).unwrap();
            assert!(repair_with(&fixture.layout, &ops).is_err(), "{conflict}");
            assert_eq!(fs::read(&path).unwrap(), before);
            assert!(root.join(MANIFEST).is_file());
            assert!(validate_entry(&fixture.layout, &instance, &instance.launcher_app).is_ok());
            if conflict == "stage" {
                assert!(root.join(STAGE).is_dir());
            }
        }
    }

    #[test]
    fn managed_check_and_repair_are_read_only_for_enabled_and_disabled_records() {
        for enabled in [false, true] {
            let (fixture, legacy) = legacy_fixture();
            fs::remove_dir_all(legacy.launcher_app).unwrap();
            fs::rename(
                legacy.runtime_app,
                fixture.layout.system_applications.join("Codex.app"),
            )
            .unwrap();
            let ops = FakeOps::default();
            let instance = deploy_with(&fixture.layout, &ops, |_, _| {}).unwrap();
            save_record(&fixture.layout, enabled, &instance).unwrap();
            let settings = fs::read(fixture.layout.settings()).unwrap();
            let config = fs::read(instance.codex_home.join("config.toml")).unwrap();
            let launcher = fs::read(instance.launcher_app.join("Contents/MacOS/Dodex")).unwrap();
            for repair in [false, true] {
                let message = managed_entry_message(&fixture.layout, &ops, repair)
                    .unwrap()
                    .unwrap();
                assert!(message.contains("校验通过"));
                assert_eq!(message.contains("无需修复"), repair);
                assert_eq!(fs::read(fixture.layout.settings()).unwrap(), settings);
                assert_eq!(
                    fs::read(instance.codex_home.join("config.toml")).unwrap(),
                    config
                );
                assert_eq!(
                    fs::read(instance.launcher_app.join("Contents/MacOS/Dodex")).unwrap(),
                    launcher
                );
                assert!(!repair_root(&fixture.layout).exists());
                assert!(!instance.codex_home.join("synthetic-observed-env").exists());
                assert_eq!(ops.copied.load(std::sync::atomic::Ordering::SeqCst), 1);
            }
            fs::write(
                instance.launcher_app.join("Contents/MacOS/Dodex"),
                "modified",
            )
            .unwrap();
            assert!(managed_entry_message(&fixture.layout, &ops, true).is_err());
            assert_eq!(fs::read(fixture.layout.settings()).unwrap(), settings);
        }
    }

    #[test]
    fn mismatched_saved_record_is_rejected_before_repair() {
        let (fixture, mut old) = legacy_fixture();
        old.codex_home = fixture.layout.user_home.join("unrelated");
        save_record(&fixture.layout, true, &old).unwrap();
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        assert!(repair_saved_with(&fixture.layout, &ops).is_err());
        assert!(!repair_root(&fixture.layout).exists());
        let saved: Record = read_json(&fixture.layout.settings()).unwrap();
        assert_eq!(saved.instance, old);
    }

    #[test]
    fn recovery_preserves_backup_and_foreign_target_on_conflict() {
        let (fixture, _) = legacy_fixture();
        make_alias(&fixture.layout);
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        interrupt_publication(&fixture.layout, &ops, 1);
        let target = fixture.layout.system_applications.join("Codex B.app");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("foreign"), b"keep").unwrap();
        assert!(repair_with(&fixture.layout, &ops).is_err());
        assert_eq!(fs::read(target.join("foreign")).unwrap(), b"keep");
        assert!(repair_root(&fixture.layout).join(BACKUP).is_dir());
        assert!(repair_root(&fixture.layout).join(STAGE).is_dir());
    }

    #[test]
    fn recovery_rejects_redirected_manifest_without_touching_backup() {
        let (fixture, _) = legacy_fixture();
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        interrupt_publication(&fixture.layout, &ops, 1);
        let pending_path = repair_root(&fixture.layout).join(PENDING);
        let mut pending: PendingRepair = read_json(&pending_path).unwrap();
        pending.bundle = fixture.layout.system_applications.join("Other.app");
        fs::write(pending_path, serde_json::to_vec(&pending).unwrap()).unwrap();
        assert!(repair_with(&fixture.layout, &ops).is_err());
        assert!(repair_root(&fixture.layout).join(BACKUP).is_dir());
        assert!(!pending.bundle.exists());
    }

    #[test]
    fn tui_and_helpers_do_not_block_desktop_repair() {
        let runtime = Path::new("/Applications/Codex B Runtime.app");
        assert!(!desktop_running(b"/Applications/Codex B Runtime.app/Contents/Resources/codex\n/Applications/Codex B Runtime.app/Contents/Frameworks/Helper\n", &[runtime]));
        assert!(!desktop_running(
            b"/Applications/Other.app/Contents/MacOS/ChatGPT\n",
            &[runtime]
        ));
        assert!(desktop_running(
            b" /Applications/Codex B Runtime.app/Contents/MacOS/ChatGPT\n",
            &[runtime]
        ));
    }
    #[test]
    fn repair_keeps_cli_manager_profile_and_original_runtime_unchanged() {
        let (fixture, old) = legacy_fixture();
        let manager = old.desktop_user_data.join("tools/codex_b_manager.py");
        let manager_before = fs::read(&manager).unwrap();
        let launcher_before = fs::read(old.launcher_app.join("Contents/MacOS/Dodex")).unwrap();
        let config_before = fs::read(old.codex_home.join("config.toml")).unwrap();
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        let new = repair_with(&fixture.layout, &ops).unwrap();
        assert_eq!(new.runtime_app.file_name().unwrap(), "Dodex.app");
        assert_eq!(new.codex_home, old.codex_home);
        assert_eq!(fs::read(manager).unwrap(), manager_before);
        assert_eq!(
            fs::read(old.codex_home.join("config.toml")).unwrap(),
            config_before
        );
        assert!(old.cli_path.is_file());
        assert_eq!(
            fs::read(
                repair_root(&fixture.layout).join("Original Launcher.app/Contents/MacOS/Dodex")
            )
            .unwrap(),
            launcher_before
        );
        assert_eq!(validate_legacy(&fixture.layout, &ops).unwrap(), new);
        validate_existing(&fixture.layout, &ops, &new).unwrap();
        validate_existing_for_sync(&fixture.layout, &ops, &new, true).unwrap();
        assert_eq!(repair_with(&fixture.layout, &ops).unwrap(), new);
        assert_eq!(ops.copied.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Run only the synthetic runtime. A poisoned parent cannot switch accounts.
        let status = Command::new(new.launcher_app.join("Contents/MacOS/Dodex"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("CODEX_HOME", "/wrong")
            .env("OPENAI_API_KEY", "synthetic")
            .status()
            .unwrap();
        assert!(status.success());
        let env = fs::read_to_string(new.codex_home.join("synthetic-observed-env")).unwrap();
        assert!(!env.contains("/wrong") && !env.contains("OPENAI_API_KEY"));
        assert!(env.contains(&format!("CODEX_HOME={}", new.codex_home.display())));
    }
    #[test]
    fn failed_runtime_copy_preserves_launcher_and_allows_retry() {
        let (fixture, old) = legacy_fixture();
        let before = fs::read(old.launcher_app.join("Contents/MacOS/Dodex")).unwrap();
        assert!(
            repair_with(
                &fixture.layout,
                &FakeOps {
                    legacy: true,
                    fail_copy: true,
                    ..FakeOps::default()
                }
            )
            .is_err()
        );
        assert_eq!(
            fs::read(old.launcher_app.join("Contents/MacOS/Dodex")).unwrap(),
            before
        );
        assert!(!repair_root(&fixture.layout).exists());
        repair_with(
            &fixture.layout,
            &FakeOps {
                legacy: true,
                ..FakeOps::default()
            },
        )
        .unwrap();
    }
    #[test]
    fn repaired_launcher_tampering_is_rejected() {
        let (fixture, _) = legacy_fixture();
        let ops = FakeOps {
            legacy: true,
            ..FakeOps::default()
        };
        let instance = repair_with(&fixture.layout, &ops).unwrap();
        fs::write(
            instance.launcher_app.join("Contents/MacOS/Dodex"),
            "#!/bin/sh\nexit 0\n",
        )
        .unwrap();
        assert!(validate_legacy(&fixture.layout, &ops).is_err());
    }
}
