//! Read-only managed Dodex runtime metadata shared by the GUI and console entry.
//! Resolving a runtime must not load a desktop bridge or inspect credentials.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

const MARKER: &str = "# agent-companion-dodex: ";
const WRAPPER: &str = include_str!("software_updates/dodex-wrapper.py");
const LEGACY_WRAPPER: &str = include_str!("software_updates/dodex-wrapper-v1.py");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub package: PathBuf,
    pub entry: PathBuf,
    pub app: PathBuf,
    pub profile_home: PathBuf,
    pub sqlite_home: PathBuf,
    pub desktop_data: PathBuf,
    pub log_dir: PathBuf,
    pub original: PathBuf,
    pub companion: PathBuf,
    pub version: String,
}

impl Binding {
    pub fn validate_profile(
        &self,
        home: &Path,
        sqlite: &Path,
        desktop: &Path,
    ) -> Result<(), String> {
        if self.profile_home != home
            || self.sqlite_home != sqlite
            || self.desktop_data != desktop
            || self.log_dir != desktop.join("logs")
        {
            return Err("Dodex 托管入口与当前副账号目录不一致；未修改程序。".into());
        }
        Ok(())
    }

    pub fn executable(&self) -> Result<PathBuf, String> {
        let executable = self.package.join("bin/codex");
        no_redirects(&executable)?;
        if fs::metadata(&executable)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            Ok(executable)
        } else {
            Err("托管 Dodex TUI 程序缺失。".into())
        }
    }
}

pub(crate) fn command(home: &Path, name: &str) -> Option<PathBuf> {
    [
        home.join(".local/bin").join(name),
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
    ]
    .into_iter()
    .chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|path| path.join(name)),
    )
    .find(|path| {
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

pub(crate) fn read_binding(home: &Path, entry: &Path) -> Result<Option<Binding>, String> {
    if !entry.is_file() {
        return Ok(None);
    }
    let bytes = read_limited(entry, 128 * 1024)?;
    let text = String::from_utf8_lossy(&bytes);
    let Some(line) = text.lines().find_map(|line| line.strip_prefix(MARKER)) else {
        return Ok(None);
    };
    let binding: Binding = serde_json::from_str(line).map_err(|_| "Dodex 托管入口元数据无效。")?;
    let support = home.join("Library/Application Support/AgentCompanion/Tui");
    if binding.entry != entry
        || !binding.package.starts_with(support.join("packages"))
        || !binding.original.starts_with(&support)
        || (render_wrapper(&binding)? != bytes && render_legacy_wrapper(&binding)? != bytes)
    {
        return Err("Dodex 托管入口已发生变化，请检查后重试。".into());
    }
    for path in [
        &binding.entry,
        &binding.package,
        &binding.profile_home,
        &binding.sqlite_home,
        &binding.desktop_data,
        &binding.log_dir,
    ] {
        no_redirects(path)?;
    }
    Ok(Some(binding))
}

pub(crate) fn render_wrapper(binding: &Binding) -> Result<Vec<u8>, String> {
    let mut metadata = serde_json::to_value(binding).map_err(|_| "无法生成隔离入口。")?;
    metadata["schema"] = 2.into();
    metadata["entry_revision"] = 2.into();
    metadata["update_route"] = "companion".into();
    let json = serde_json::to_string(&metadata).map_err(|_| "无法生成隔离入口。")?;
    render(WRAPPER, &json)
}

pub(crate) fn render_legacy_wrapper(binding: &Binding) -> Result<Vec<u8>, String> {
    render(
        LEGACY_WRAPPER,
        &serde_json::to_string(binding).map_err(|_| "无法生成隔离入口。")?,
    )
}

fn render(template: &str, json: &str) -> Result<Vec<u8>, String> {
    let literal = serde_json::to_string(&json).map_err(|_| "无法编码隔离入口。")?;
    Ok(template
        .replace("# __BINDING_MARKER__", &format!("{MARKER}{json}"))
        .replace("__BINDING_JSON__", &literal)
        .into_bytes())
}

pub(crate) fn no_redirects(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("安装路径必须是规范的绝对路径。".into());
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("安装或账号目录包含重定向；未修改程序。".into());
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err("无法验证安装目录。".into());
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut data))
        .map_err(|_| "无法读取安装元数据。")?;
    if data.len() as u64 > limit {
        return Err("安装元数据过大。".into());
    }
    Ok(data)
}
