//! Register only the user-level command directory. Shell files are edited as
//! plain text, never sourced, and existing content and permissions are retained.
fn atomic_write(path: &std::path::Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    super::atomic_write(path, bytes, mode).map_err(|e| e.to_string())
}
fn no_redirects(path: &std::path::Path) -> Result<(), String> {
    super::no_redirects(path).map_err(|e| e.to_string())
}
fn read_limited(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    super::read_limited(path, limit).map_err(|e| e.to_string())
}
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

const START: &str = "# >>> Agent Companion Dodex PATH >>>";
const END: &str = "# <<< Agent Companion Dodex PATH <<<";
const BLOCK: &str = "# >>> Agent Companion Dodex PATH >>>\ncase \":$PATH:\" in\n  *\":$HOME/.local/bin:\"*) ;;\n  *) export PATH=\"$HOME/.local/bin:$PATH\" ;;\nesac\n# <<< Agent Companion Dodex PATH <<<\n";

pub(super) fn configure(home: &Path, entry: &Path) -> Result<(), String> {
    let shell = std::env::var_os("SHELL").or_else(login_shell);
    configure_for(
        home,
        entry,
        &std::env::var_os("PATH").unwrap_or_default(),
        shell.as_deref(),
    )
}

fn login_shell() -> Option<OsString> {
    // Finder may omit SHELL. The reentrant account lookup is safe on workers
    // and never executes the user's login configuration.
    let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0_u8; 16 * 1024];
    let code = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            account.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if code != 0 || result.is_null() {
        return None;
    }
    let account = unsafe { account.assume_init() };
    if account.pw_shell.is_null() {
        return None;
    }
    Some(
        OsStr::from_bytes(unsafe { std::ffi::CStr::from_ptr(account.pw_shell) }.to_bytes())
            .to_owned(),
    )
}

fn configure_for(
    home: &Path,
    entry: &Path,
    path: &OsStr,
    shell: Option<&OsStr>,
) -> Result<(), String> {
    let directory = entry.parent().ok_or("dodex 命令目录无效。")?;
    if std::env::split_paths(path).any(|value| value == directory) {
        return Ok(());
    }
    let manual = "dodex 命令已保留，但终端 PATH 尚未配置。请将命令目录加入 PATH 后重新打开终端，再重试安装；Companion 监控尚未启用。";
    if directory != home.join(".local/bin") {
        return Err(manual.into());
    }
    let files = match shell
        .and_then(|value| Path::new(value).file_name())
        .and_then(OsStr::to_str)
    {
        Some("zsh") => vec![home.join(".zprofile"), home.join(".zshrc")],
        Some("bash") => {
            // Creating .bash_profile can suppress an existing .profile. Use
            // Bash's existing login file, or its last fallback on fresh homes.
            let login = [".bash_profile", ".bash_login", ".profile"]
                .into_iter()
                .map(|name| home.join(name))
                .find(|file| fs::symlink_metadata(file).is_ok())
                .unwrap_or_else(|| home.join(".profile"));
            vec![login, home.join(".bashrc")]
        }
        _ => return Err(manual.into()),
    };
    // Validate every file before publishing any edits. A failed later write is
    // resumable: the exact managed block is recognized on the next attempt.
    let edits = files
        .iter()
        .map(|file| prepare(file))
        .collect::<Result<Vec<_>, _>>()?;
    for (file, (content, mode)) in files.iter().zip(edits) {
        if let Some(content) = content {
            atomic_write(file, &content, mode)?;
        }
    }
    Ok(())
}

fn prepare(file: &Path) -> Result<(Option<Vec<u8>>, u32), String> {
    no_redirects(file)?;
    let (mut bytes, mode) = match fs::symlink_metadata(file) {
        Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => {
            (read_limited(file, 1024 * 1024)?, metadata.mode() & 0o777)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), 0o600),
        _ => return Err("终端配置不是独立普通文件；未自动配置 PATH，请检查后重试。".into()),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| "终端配置不是 UTF-8；未自动配置 PATH。")?;
    if text.contains(START) || text.contains(END) {
        if text.matches(START).count() == 1
            && text.matches(END).count() == 1
            && text.contains(BLOCK)
        {
            return Ok((None, mode));
        }
        return Err("Dodex PATH 配置段已被修改；未覆盖，请检查后重试。".into());
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(BLOCK.as_bytes());
    Ok((Some(bytes), mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn configured_path_changes_no_shell_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let entry = home.join(".local/bin/dodex");
        configure_for(&home, &entry, entry.parent().unwrap().as_os_str(), None).unwrap();
        assert_eq!(fs::read_dir(home).unwrap().count(), 0);
    }

    #[test]
    fn fresh_shell_setup_preserves_existing_text_and_is_idempotent() {
        for shell in ["/bin/zsh", "/bin/bash"] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().canonicalize().unwrap();
            let original = b"# User settings\nexport EXISTING=kept";
            let file = home.join(if shell.ends_with("zsh") {
                ".zshrc"
            } else {
                ".profile"
            });
            fs::write(&file, original).unwrap();
            let entry = home.join(".local/bin/dodex");
            configure_for(
                &home,
                &entry,
                OsStr::new("/usr/bin"),
                Some(OsStr::new(shell)),
            )
            .unwrap();
            let first = fs::read(&file).unwrap();
            assert!(first.starts_with(original));
            configure_for(
                &home,
                &entry,
                OsStr::new("/usr/bin"),
                Some(OsStr::new(shell)),
            )
            .unwrap();
            assert_eq!(fs::read(file).unwrap(), first);
            assert!(!home.join(".bash_profile").exists());
        }
    }

    #[test]
    fn unsafe_or_unsupported_shell_configuration_fails_without_editing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let entry = home.join(".local/bin/dodex");
        assert!(
            configure_for(
                &home,
                &entry,
                OsStr::new("/usr/bin"),
                Some(OsStr::new("fish"))
            )
            .is_err()
        );
        fs::write(home.join("original"), b"keep").unwrap();
        symlink(home.join("original"), home.join(".zshrc")).unwrap();
        assert!(
            configure_for(
                &home,
                &entry,
                OsStr::new("/usr/bin"),
                Some(OsStr::new("zsh"))
            )
            .is_err()
        );
        assert!(!home.join(".zprofile").exists());
        assert_eq!(fs::read(home.join("original")).unwrap(), b"keep");
    }
}
