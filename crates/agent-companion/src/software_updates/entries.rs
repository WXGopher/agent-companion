//! Audited historical command entries, used only by explicit maintenance.
use crate::managed_tui::{Binding, read_limited, render_wrapper};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
const LEGACY_ADAPTER: &str = include_str!("legacy-dodex-adapter-v0.py");
const PRIMARY_WRAPPER: &str = include_str!("primary-wrapper.py");
const LEGACY_PRIMARY_WRAPPER: &str = include_str!("primary-wrapper-v1.py");
/// Template revision, App route and maintenance route are part of alignment,
/// even when the complete native package already has the requested version.
pub(crate) fn needs_migration(binding: &Binding, app: &Path, companion: &Path) -> bool {
    binding.app != app
        || binding.companion != companion
        || read_limited(&binding.entry, 128 * 1024).ok() != render_wrapper(binding).ok()
}

pub(crate) fn legacy_adapter(home: &Path, bytes: &[u8]) -> Result<(), String> {
    let manager = home.join("Library/Application Support/Codex-B/tools/codex_b_manager.py");
    let expected = LEGACY_ADAPTER.replace(
        "__MANAGER_JSON__",
        &serde_json::to_string(&manager).map_err(|_| "Dodex 入口路径无效。")?,
    );
    if expected.as_bytes() != bytes {
        return Err("现有 dodex 入口不是已验证的历史模板；未替换任何入口。".into());
    }
    Ok(())
}

fn python_literal(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("安装路径包含控制字符。".into());
    }
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let escaped = value
        .replace('\\', "\\\\")
        .replace(quote, &format!("\\{quote}"));
    Ok(format!("{quote}{escaped}{quote}"))
}

pub(crate) fn primary_native(home: &Path, entry: &Path) -> Result<PathBuf, String> {
    let target = home.join(".codex/packages/standalone/current/bin/codex");
    let native =
        if fs::symlink_metadata(entry).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            entry
                .canonicalize()
                .map_err(|_| "官方 TUI 链接目标不可用。")?
        } else if is_native_package_entry(entry) {
            entry.canonicalize().map_err(|_| "官方 TUI 程序不可用。")?
        } else {
            let bytes = read_limited(entry, 128 * 1024)?;
            let rendered = render_primary_wrapper(home)?;
            let legacy = render_primary_template(home, LEGACY_PRIMARY_WRAPPER)?;
            if bytes != rendered && bytes != legacy {
                return Err(
                    "codex 入口不是官方链接或已验证的主账号 wrapper；未执行或替换未知入口。".into(),
                );
            } else {
                target
                    .canonicalize()
                    .map_err(|_| "主账号 wrapper 的官方程序不可用。")?
            }
        };
    if !is_native_package_entry(&native) {
        return Err("codex 入口没有指向完整的原生 TUI 包。".into());
    }
    Ok(native)
}

pub(crate) fn render_primary_wrapper(home: &Path) -> Result<Vec<u8>, String> {
    render_primary_template(home, PRIMARY_WRAPPER)
}

fn render_primary_template(home: &Path, template: &str) -> Result<Vec<u8>, String> {
    Ok(template
        .replace(
            "__PRIMARY_HOME__",
            &python_literal(&home.join(".codex").to_string_lossy())?,
        )
        .replace(
            "__TARGET__",
            &python_literal(
                &home
                    .join(".codex/packages/standalone/current/bin/codex")
                    .to_string_lossy(),
            )?,
        )
        .into_bytes())
}

fn is_native_package_entry(entry: &Path) -> bool {
    let mut magic = [0; 4];
    entry.file_name().is_some_and(|name| name == "codex")
        && entry
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| root.join("codex-package.json").is_file())
        && File::open(entry)
            .and_then(|mut file| file.read_exact(&mut magic))
            .is_ok()
        && matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xce]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_tui::{read_binding, render_legacy_wrapper};
    use std::os::unix::fs::symlink;

    fn binding(home: &Path) -> Binding {
        let support = home.join("Library/Application Support/AgentCompanion/Tui");
        Binding {
            package: support.join("packages/complete/package"),
            entry: home.join("dodex"),
            app: home.join("Applications/Dodex.app"),
            profile_home: home.join("second"),
            sqlite_home: home.join("second/sqlite"),
            desktop_data: home.join("desktop"),
            log_dir: home.join("desktop/logs"),
            original: support.join("original-dodex"),
            companion: home.join("Companion"),
            version: "0.160.0".into(),
        }
    }

    #[test]
    fn exact_previous_template_is_readable_but_requires_migration() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().canonicalize().unwrap();
        let binding = binding(&home);
        fs::write(&binding.entry, render_legacy_wrapper(&binding).unwrap()).unwrap();
        let old = read_binding(&home, &binding.entry).unwrap().unwrap();
        assert!(needs_migration(&old, &binding.app, &binding.companion));
        fs::write(&binding.entry, render_wrapper(&binding).unwrap()).unwrap();
        let current = read_binding(&home, &binding.entry).unwrap().unwrap();
        assert!(!needs_migration(&current, &binding.app, &binding.companion));
        assert!(needs_migration(
            &current,
            &home.join("new App"),
            &binding.companion
        ));
        for changed in [
            render_legacy_wrapper(&binding).unwrap(),
            render_wrapper(&binding).unwrap(),
        ] {
            let changed = [changed.as_slice(), b"\n# unknown modification\n"].concat();
            fs::write(&binding.entry, &changed).unwrap();
            assert!(read_binding(&home, &binding.entry).is_err());
            assert_eq!(fs::read(&binding.entry).unwrap(), changed);
        }
    }

    #[test]
    fn legacy_adapter_requires_the_entire_audited_template() {
        let home = Path::new("/fixture user");
        let manager = home.join("Library/Application Support/Codex-B/tools/codex_b_manager.py");
        let expected = LEGACY_ADAPTER.replace(
            "__MANAGER_JSON__",
            &serde_json::to_string(&manager).unwrap(),
        );
        legacy_adapter(home, expected.as_bytes()).unwrap();
        assert!(legacy_adapter(home, (expected + "\nprint('changed')\n").as_bytes()).is_err());
        assert!(legacy_adapter(home, b"# Dodex: B's native CLI by default; isolate the desktop and updater entry points.\nMANAGER_PATH = Path('/untrusted')\n").is_err());
    }

    #[test]
    fn native_resolution_supports_vendor_link_and_generated_primary_wrapper_without_execution() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user's workspace");
        let root = home.join(".codex/packages/standalone/releases/release");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("codex-package.json"), b"{}").unwrap();
        let native = root.join("bin/codex");
        fs::write(&native, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        // Exercise a real-sized native file without trying to load it as text.
        File::options()
            .write(true)
            .open(&native)
            .unwrap()
            .set_len(200 * 1024 * 1024)
            .unwrap();
        let current = home.join(".codex/packages/standalone/current");
        symlink(&root, &current).unwrap();
        let entry = home.join("codex");
        symlink(current.join("bin/codex"), &entry).unwrap();
        assert_eq!(primary_native(&home, &entry).unwrap(), native);
        assert_eq!(primary_native(&home, &native).unwrap(), native);
        fs::remove_file(&entry).unwrap();
        let wrapper = PRIMARY_WRAPPER
            .replace(
                "__PRIMARY_HOME__",
                &python_literal(&home.join(".codex").to_string_lossy()).unwrap(),
            )
            .replace(
                "__TARGET__",
                &python_literal(&current.join("bin/codex").to_string_lossy()).unwrap(),
            );
        fs::write(&entry, &wrapper).unwrap();
        assert_eq!(primary_native(&home, &entry).unwrap(), native);
        fs::write(&entry, wrapper + "\nprint('unknown')\n").unwrap();
        assert!(primary_native(&home, &entry).is_err());
        fs::write(&native, "#!/bin/sh\ntouch should-never-exist\n").unwrap();
        assert!(primary_native(&home, &native).is_err());
        assert!(!home.join("should-never-exist").exists());
    }
}
