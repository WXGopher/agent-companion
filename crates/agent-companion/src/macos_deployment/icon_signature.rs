//! Finder custom icons live outside the signed Contents tree, but their resource
//! forks fail codesign's strict detritus check. Preserve strict verification by
//! removing only those two known presentation items from a private APFS clone.
//! The installed app, its signed resources, and its signature are never changed.
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
};

const ICON_FILE: &str = "Icon\r";
const FINDER_INFO: &str = "com.apple.FinderInfo";
const RESOURCE_FORK: &str = "com.apple.ResourceFork";

pub(super) fn verify(app: &Path) -> Result<bool, String> {
    if signature_valid(app, true)? {
        return Ok(true);
    }
    // Ordinary validation still authenticates every sealed resource and the
    // official identity before we make a copy. This is not an unsigned fallback.
    if !standard_custom_icon(app) || !signature_valid(app, false)? {
        return Ok(false);
    }
    let temporary = tempfile::Builder::new()
        .prefix("agent-companion-icon-verification-")
        .tempdir()
        .map_err(|_| "无法创建自定义图标签名校验副本。")?;
    let copy = temporary.path().join("Runtime.app");
    // -c requires copy-on-write support; fail closed instead of modifying the
    // source or silently copying gigabytes on an unsupported filesystem.
    if !quiet_status(Command::new("/bin/cp").arg("-cR").arg(app).arg(&copy))? {
        return Err("无法创建自定义图标签名校验副本；现有应用未修改。".into());
    }
    // Recheck copied metadata before deleting anything, including link counts.
    if !standard_custom_icon(&copy) {
        return Ok(false);
    }
    fs::remove_file(copy.join(ICON_FILE)).map_err(|_| "无法检查自定义应用图标。")?;
    if !quiet_status(
        Command::new("/usr/bin/xattr")
            .args(["-d", FINDER_INFO])
            .arg(&copy),
    )? {
        return Ok(false);
    }
    // Any nested FinderInfo/resource fork, changed executable, or modified
    // sealed resource remains present and must still fail strict validation.
    signature_valid(&copy, true)
}

fn signature_valid(app: &Path, strict: bool) -> Result<bool, String> {
    let mut command = Command::new("/usr/bin/codesign");
    command.args(["--verify", "--deep"]);
    if strict {
        command.arg("--strict");
    }
    quiet_status(command.args(["-R", super::OFFICIAL_REQUIREMENT]).arg(app))
}

fn quiet_status(command: &mut Command) -> Result<bool, String> {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|_| "无法执行应用签名或自定义图标校验。".into())
}

fn standard_custom_icon(app: &Path) -> bool {
    let icon = app.join(ICON_FILE);
    if !fs::symlink_metadata(&icon)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 0 && metadata.nlink() == 1)
    {
        return false;
    }
    let Ok(attributes) = Command::new("/usr/bin/xattr").arg(&icon).output() else {
        return false;
    };
    if !attributes.status.success()
        || !attributes
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|name| name == RESOURCE_FORK.as_bytes())
    {
        return false;
    }
    let Ok(info) = Command::new("/usr/bin/xattr")
        .args(["-px", FINDER_INFO])
        .arg(app)
        .output()
    else {
        return false;
    };
    info.status.success() && standard_finder_info(&info.stdout)
}

fn standard_finder_info(hex: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(hex) else {
        return false;
    };
    let Some(bytes) = text
        .split_ascii_whitespace()
        .map(|value| {
            (value.len() == 2)
                .then(|| u8::from_str_radix(value, 16).ok())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    // Finder's flags are big-endian at byte 8; kHasCustomIcon is 0x0400.
    bytes.len() == 32 && bytes[8] & 0x04 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const CUSTOM_INFO: &str = "00 00 00 00 00 00 00 00 04 00 00 00 00 00 00 00\n00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00\n";

    #[test]
    fn finder_info_requires_complete_metadata_and_custom_icon_flag() {
        assert!(standard_finder_info(CUSTOM_INFO.as_bytes()));
        assert!(!standard_finder_info(
            CUSTOM_INFO.replace("04", "00").as_bytes()
        ));
        assert!(!standard_finder_info(&CUSTOM_INFO.as_bytes()[3..]));
        assert!(!standard_finder_info(format!("{CUSTOM_INFO}00").as_bytes()));
        assert!(!standard_finder_info(
            CUSTOM_INFO.replace("04", "zz").as_bytes()
        ));
    }

    #[test]
    fn icon_requires_resource_fork_and_root_finder_info() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path();
        let icon = app.join(ICON_FILE);
        fs::write(&icon, []).unwrap();
        assert!(!standard_custom_icon(app));
        assert!(
            quiet_status(
                Command::new("/usr/bin/xattr")
                    .args(["-wx", RESOURCE_FORK, "00000000"])
                    .arg(&icon)
            )
            .unwrap()
        );
        assert!(!standard_custom_icon(app));
        assert!(
            quiet_status(
                Command::new("/usr/bin/xattr")
                    .args(["-wx", FINDER_INFO, CUSTOM_INFO])
                    .arg(app)
            )
            .unwrap()
        );
        assert!(standard_custom_icon(app));
        fs::write(&icon, b"nonempty data fork").unwrap();
        assert!(!standard_custom_icon(app));
    }

    #[test]
    fn icon_rejects_symbolic_and_hard_links_and_directories() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path();
        let icon = app.join(ICON_FILE);
        let target = app.join("target");
        fs::write(&target, []).unwrap();
        symlink(&target, &icon).unwrap();
        assert!(!standard_custom_icon(app));
        fs::remove_file(&icon).unwrap();
        fs::hard_link(&target, &icon).unwrap();
        assert!(!standard_custom_icon(app));
        fs::remove_file(&icon).unwrap();
        fs::create_dir(&icon).unwrap();
        assert!(!standard_custom_icon(app));
    }
}
