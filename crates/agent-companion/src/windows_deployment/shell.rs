//! Register a native, self-contained Dodex command. Never edit shell profiles or
//! execution policy, and never depend on the download/build directory surviving.
use super::{no_links, no_redirects, read_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Write},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, LPARAM, WPARAM},
        System::Registry::{
            HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND,
            RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
        },
        UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        },
    },
    core::w,
};

const MARKER: &str = "dodex.agent-companion.json";
const OWNER: &str = "agent-companion/dodex";

#[derive(Serialize, Deserialize)]
struct Ownership {
    schema: u32,
    owner: String,
    // During replacement both hashes are accepted, so interruption between the
    // two atomic writes can be repaired without claiming an unrelated file.
    hashes: Vec<String>,
}

struct Environment {
    local: PathBuf,
    home: PathBuf,
    roaming: Option<PathBuf>,
    current_directory: PathBuf,
    path: OsString,
    extensions: OsString,
    variables: Vec<(OsString, OsString)>,
}

impl Environment {
    fn read() -> Result<Self, String> {
        Ok(Self {
            local: std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .ok_or("无法定位本地应用目录。")?,
            home: crate::util::home_dir().ok_or("无法定位用户目录。")?,
            roaming: std::env::var_os("APPDATA").map(PathBuf::from),
            current_directory: std::env::current_dir().map_err(|_| "无法检查当前 shell 目录。")?,
            path: std::env::var_os("PATH").unwrap_or_default(),
            extensions: std::env::var_os("PATHEXT").unwrap_or_default(),
            variables: std::env::vars_os().collect(),
        })
    }

    fn candidates(&self) -> Vec<(PathBuf, &'static str)> {
        let mut paths = vec![
            (self.home.join(".local/bin"), "%USERPROFILE%\\.local\\bin"),
            (self.home.join(".cargo/bin"), "%USERPROFILE%\\.cargo\\bin"),
        ];
        if let Some(roaming) = &self.roaming {
            paths.push((roaming.join("npm"), "%APPDATA%\\npm"));
        }
        paths.extend([
            (self.stable_bin(), "%LOCALAPPDATA%\\AgentCompanion\\bin"),
            (
                self.local.join("Microsoft/WindowsApps"),
                "%LOCALAPPDATA%\\Microsoft\\WindowsApps",
            ),
        ]);
        paths
    }

    fn stable_bin(&self) -> PathBuf {
        self.local.join("AgentCompanion/bin")
    }

    fn expand(&self, entry: &Path) -> PathBuf {
        let value = entry.to_string_lossy();
        let mut result = String::new();
        let mut rest = value.as_ref();
        while let Some(start) = rest.find('%') {
            result.push_str(&rest[..start]);
            rest = &rest[start..];
            let Some(end) = rest[1..].find('%').map(|end| end + 1) else {
                break;
            };
            let name = &rest[1..end];
            if let Some((_, value)) = self
                .variables
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
            {
                result.push_str(&value.to_string_lossy());
            } else {
                result.push_str(&rest[..=end]);
            }
            rest = &rest[end + 1..];
        }
        result.push_str(rest);
        PathBuf::from(result)
    }

    fn entries(&self, path: &OsStr) -> Vec<PathBuf> {
        std::env::split_paths(path)
            .map(|entry| self.current_directory.join(entry))
            .collect()
    }

    fn contains(&self, path: &OsStr, directory: &Path) -> bool {
        self.entries(path)
            .iter()
            .any(|entry| same_path(entry, directory))
    }
}

pub(super) struct Registration {
    directory: PathBuf,
    in_current_path: bool,
}

impl Registration {
    pub(super) fn message(&self) -> String {
        let refresh = if self.in_current_path {
            "已有此 PATH 的 PowerShell、cmd 和 Git Bash 可直接运行 dodex；Git Bash 如有缓存请运行 hash -r。"
        } else {
            "已加入用户 PATH；请完全退出并重开终端后运行 dodex，旧终端不会自动更新 PATH。"
        };
        format!("dodex 命令目录：{}。{refresh}", self.directory.display())
    }
}

pub(super) fn ensure() -> Result<Registration, String> {
    let environment = Environment::read()?;
    let source = std::env::current_exe().map_err(|_| "无法定位 Companion 程序。")?;
    let registration = register(&source, &environment)?;
    let current = read_user_path()?;
    if let Some(updated) = append_user_path(&current, &registration.directory, &environment)? {
        write_user_path(&updated)?;
        // Explorer propagates this to future processes. An existing terminal's
        // environment belongs to that process and cannot be changed from here.
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("Environment").0 as isize),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            );
        }
    }
    Ok(registration)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    key(a) == key(b)
}

fn conflict(path: &Path) -> String {
    format!(
        "发现已有或被修改的 dodex 命令：{}。未覆盖；请先移动或重命名该命令，再重试。",
        path.display()
    )
}

fn ownership(directory: &Path) -> Result<Option<Ownership>, String> {
    let marker = directory.join(MARKER);
    no_redirects(&marker)?;
    if !marker.exists() {
        return Ok(None);
    }
    let owner: Ownership = read_json(&marker).map_err(|_| conflict(&marker))?;
    if owner.schema != 1
        || owner.owner != OWNER
        || owner.hashes.is_empty()
        || owner.hashes.len() > 2
        || owner
            .hashes
            .iter()
            .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(conflict(&marker));
    }
    Ok(Some(owner))
}

fn file_hash(path: &Path) -> Result<String, String> {
    file_hash_with(path, false)
}

fn file_hash_with(path: &Path, allow_hardlinks: bool) -> Result<String, String> {
    no_links(path, allow_hardlinks)?;
    let mut file =
        File::open(path).map_err(|_| format!("无法读取命令文件：{}。", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = file
            .read(&mut buffer)
            .map_err(|_| "无法校验 dodex 命令。")?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn command_names(extensions: &OsStr) -> Vec<String> {
    let mut names: Vec<String> = [
        "dodex",
        "dodex.exe",
        "dodex.com",
        "dodex.cmd",
        "dodex.bat",
        "dodex.ps1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for extension in extensions.to_string_lossy().split(';') {
        let extension = extension.trim().to_ascii_lowercase();
        if extension.starts_with('.')
            && extension
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.')
        {
            let name = format!("dodex{extension}");
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn check_directory(directory: &Path, names: &[String]) -> Result<(), String> {
    for name in names {
        let path = directory.join(name);
        if fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            continue;
        }
        if name != "dodex.exe" {
            return Err(conflict(&path));
        }
        no_redirects(&path).map_err(|_| conflict(&path))?;
        let owner = ownership(directory)?.ok_or_else(|| conflict(&path))?;
        if !owner.hashes.contains(&file_hash(&path)?) {
            return Err(conflict(&path));
        }
    }
    Ok(())
}

fn register(source: &Path, environment: &Environment) -> Result<Registration, String> {
    // Check the entire visible search path, including cmd's current directory.
    // Installing elsewhere must not silently shadow or be shadowed by another tool.
    let candidates = environment.candidates();
    let names = command_names(&environment.extensions);
    let search = std::iter::once(environment.current_directory.clone())
        .chain(environment.entries(&environment.path));
    let mut existing = None;
    for directory in search {
        check_directory(&directory, &names)?;
        if directory.join("dodex.exe").is_file() {
            let supported = candidates
                .iter()
                .find(|(path, _)| same_path(path, &directory))
                .map(|(path, _)| path.clone())
                .ok_or_else(|| {
                    format!(
                        "已有 dodex 命令位于非受支持的命令目录：{}。请移走该命令后重试。",
                        directory.display()
                    )
                })?;
            if existing.is_none() {
                existing = Some(supported);
            }
        }
    }
    // Repair the command this shell actually finds before considering another
    // directory; otherwise a managed older copy could keep shadowing the update.
    let directory = existing
        .or_else(|| {
            candidates.into_iter().map(|(path, _)| path).find(|path| {
                environment.contains(&environment.path, path)
                    && path.is_dir()
                    && no_redirects(path).is_ok()
                    && tempfile::NamedTempFile::new_in(path).is_ok()
            })
        })
        .unwrap_or_else(|| environment.stable_bin());
    no_redirects(&directory)?;
    if directory.to_string_lossy().contains([';', '"']) {
        return Err("dodex 命令目录不能包含分号或双引号。".into());
    }
    fs::create_dir_all(&directory).map_err(|_| "无法创建 dodex 命令目录。")?;
    check_directory(&directory, &names)?;
    install_command(source, &directory)?;
    Ok(Registration {
        in_current_path: environment.contains(&environment.path, &directory),
        directory,
    })
}

fn save_ownership(directory: &Path, hashes: Vec<String>, existing: bool) -> Result<(), String> {
    let mut stage =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法保存 dodex 命令登记。")?;
    serde_json::to_writer(
        &mut stage,
        &Ownership {
            schema: 1,
            owner: OWNER.into(),
            hashes,
        },
    )
    .map_err(|_| "无法保存 dodex 命令登记。")?;
    stage.flush().map_err(|_| "无法保存 dodex 命令登记。")?;
    let target = directory.join(MARKER);
    let result = if existing {
        stage.persist(target)
    } else {
        stage.persist_noclobber(target)
    };
    result.map_err(|_| "无法保存 dodex 命令登记。")?;
    Ok(())
}

fn install_command(source: &Path, directory: &Path) -> Result<(), String> {
    let target = directory.join("dodex.exe");
    let owner = ownership(directory)?;
    // Cargo and some package managers hard-link immutable source executables.
    // Copy their bytes, then require the installed command to be independent.
    let new_hash = file_hash_with(source, true)?;
    let previous_hash = if target.exists() {
        Some(file_hash(&target)?)
    } else {
        None
    };
    if let Some(hash) = &previous_hash {
        if !owner
            .as_ref()
            .is_some_and(|owner| owner.hashes.contains(hash))
        {
            return Err(conflict(&target));
        }
        if *hash == new_hash {
            // In particular, dodex --deploy must not replace its running self.
            return Ok(());
        }
    }
    let mut hashes = vec![new_hash.clone()];
    if let Some(hash) = &previous_hash {
        hashes.push(hash.clone());
    }
    save_ownership(directory, hashes, owner.is_some())?;
    let stage = tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法写入 dodex 命令。")?;
    fs::copy(source, stage.path()).map_err(|_| "无法复制 dodex 命令。")?;
    if file_hash(stage.path())? != new_hash {
        return Err("Companion 程序在复制时改变，请重试。".into());
    }
    let result = if previous_hash.is_some() {
        stage.persist(&target)
    } else {
        stage.persist_noclobber(&target)
    };
    result.map_err(|_| "无法更新 dodex 命令；请等待正在执行的 dodex 退出后重试。")?;
    save_ownership(directory, vec![new_hash], true)
}

struct UserPath {
    value: OsString,
    kind: REG_VALUE_TYPE,
}

fn read_user_path() -> Result<UserPath, String> {
    for _ in 0..3 {
        let mut size = 0;
        let mut kind = REG_SZ;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(UserPath {
                value: OsString::new(),
                kind: REG_EXPAND_SZ,
            });
        }
        if result.is_err() || size > 128 * 1024 || size % 2 != 0 {
            return Err("无法读取用户 PATH；未修改其内容。".into());
        }
        let mut data = vec![0u16; size as usize / 2];
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                w!("Path"),
                flags,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if result == ERROR_MORE_DATA {
            continue;
        }
        if result.is_err() {
            return Err("无法读取用户 PATH；未修改其内容。".into());
        }
        data.truncate(size as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        if data.contains(&0) {
            return Err("用户 PATH 含无效字符；未修改其内容。".into());
        }
        return Ok(UserPath {
            value: OsString::from_wide(&data),
            kind,
        });
    }
    Err("用户 PATH 正在被修改，请稍后重试。".into())
}

fn append_user_path(
    current: &UserPath,
    directory: &Path,
    environment: &Environment,
) -> Result<Option<UserPath>, String> {
    let already_present = std::env::split_paths(&current.value).any(|entry| {
        let entry = if current.kind == REG_EXPAND_SZ {
            environment.expand(&entry)
        } else {
            entry
        };
        same_path(&environment.current_directory.join(entry), directory)
    });
    if already_present {
        return Ok(None);
    }
    let mut value = current.value.clone();
    if !value.is_empty() && value.encode_wide().last() != Some(b';' as u16) {
        value.push(";");
    }
    if current.kind == REG_EXPAND_SZ {
        let expression = environment
            .candidates()
            .into_iter()
            .find(|(path, _)| same_path(path, directory))
            .map(|(_, expression)| expression)
            .ok_or("无法保存 dodex 命令目录。")?;
        value.push(expression);
    } else {
        value.push(directory);
    }
    if value.encode_wide().count() >= 32767 {
        return Err("用户 PATH 已过长，无法追加 dodex 命令目录。".into());
    }
    Ok(Some(UserPath {
        value,
        kind: current.kind,
    }))
}

fn write_user_path(path: &UserPath) -> Result<(), String> {
    let data: Vec<u16> = path.value.encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Environment"),
            w!("Path"),
            path.kind.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if result.is_err() {
        return Err("无法保存用户 PATH；请检查当前用户权限后重试。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
