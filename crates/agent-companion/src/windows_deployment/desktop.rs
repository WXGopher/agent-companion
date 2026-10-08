//! A persistent GUI launcher and a Start menu shortcut, separate from PATH.
use super::{InstanceConfig, launcher, no_redirects};
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ,
        },
        UI::{
            Shell::{
                FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
                SLGP_RAWPATH, ShellLink,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{Interface, PCWSTR, w},
};

pub(super) fn ensure(instance: &InstanceConfig) -> Result<String, String> {
    let source = std::env::current_exe().map_err(|_| "无法定位 Companion 程序。")?;
    let instance = instance.clone();
    // Deployment also runs on GUI workers. Use our own apartment so callers'
    // COM initialization cannot change shell-link behavior.
    std::thread::spawn(move || {
        let _com = ComApartment::new().map_err(|_| "无法初始化开始菜单。")?;
        let programs = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
            .map_err(|_| "无法定位开始菜单。")?;
        let path = unsafe { programs.to_string() };
        unsafe { CoTaskMemFree(Some(programs.0.cast())) };
        let shortcut = PathBuf::from(path.map_err(|_| "开始菜单路径无效。")?).join("Dodex.lnk");
        register(&source, &instance, &shortcut)?;
        Ok("已添加开始菜单 Dodex（App）；命令行 dodex 打开第二个 profile 的 Codex CLI。".into())
    })
    .join()
    .map_err(|_| "无法配置 Dodex 开始菜单入口。")?
}

struct ComApartment;

impl ComApartment {
    fn new() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    // Use native separators when passing filesystem paths to shell APIs.
    value
        .encode_wide()
        .map(|ch| if ch == b'/' as u16 { b'\\' as u16 } else { ch })
        .chain(Some(0))
        .collect()
}

fn text(buffer: &[u16]) -> OsString {
    let end = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..end])
}

fn load_shortcut(path: &Path) -> windows::core::Result<IShellLinkW> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        file.Load(PCWSTR(wide(path.as_os_str()).as_ptr()), STGM_READ)?;
        Ok(link)
    }
}

fn shortcut_target(link: &IShellLinkW) -> windows::core::Result<PathBuf> {
    let mut buffer = vec![0; 32768];
    unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)? };
    Ok(PathBuf::from(text(&buffer)))
}

fn same_shortcut_path(left: &Path, right: &Path) -> bool {
    // The shell expands 8.3 parent names when saving/loading a link. Comparing
    // only its spelling rejects the same file when TEMP or a user directory
    // has a short name. Keep this equivalence local to shell-link inspection:
    // redirects and hard links remain invalid deployment paths.
    if no_redirects(left).is_err() || no_redirects(right).is_err() {
        return false;
    }
    if launcher::same_path(left, right) {
        return true;
    }
    let resolved = |path: &Path| {
        path.canonicalize().ok().or_else(|| {
            // An owned shortcut may outlive its launcher. Resolve its existing
            // parent so repair still works without accepting unknown parents.
            Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?))
        })
    };
    match (resolved(left), resolved(right)) {
        (Some(left), Some(right)) => launcher::same_path(&left, &right),
        _ => false,
    }
}

fn check_shortcut(path: &Path, target: &Path) -> Result<(), String> {
    no_redirects(path)?;
    if !path.exists() {
        return Ok(());
    }
    let conflict = || {
        format!(
            "发现无关的开始菜单入口：{}。未覆盖，请先重命名后重试。",
            path.display()
        )
    };
    let link = load_shortcut(path).map_err(|_| conflict())?;
    let existing = shortcut_target(&link).map_err(|_| conflict())?;
    no_redirects(&existing).map_err(|_| conflict())?;
    if same_shortcut_path(&existing, target) {
        return Ok(());
    }
    // Repair a shortcut to a managed older CLI copy, but never claim another
    // app just because its executable happens to have the same name.
    if existing
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("dodex.exe"))
        && let Some(directory) = existing.parent()
        && let Ok(Some(owner)) = launcher::ownership(directory)
        && let Ok(hash) = launcher::file_hash(&existing)
        && owner.hashes.contains(&hash)
    {
        let mut arguments = vec![0; 32768];
        unsafe { link.GetArguments(&mut arguments) }.map_err(|_| conflict())?;
        if text(&arguments).is_empty() {
            return Ok(());
        }
    }
    Err(conflict())
}

fn register(source: &Path, instance: &InstanceConfig, shortcut: &Path) -> Result<(), String> {
    let root = instance.codex_home.parent().ok_or("无效的 Dodex 目录。")?;
    let executable = root.join("dodex.exe");
    no_redirects(root)?;
    check_shortcut(shortcut, &executable)?;
    launcher::install_command(source, root, launcher::Kind::Desktop)?;
    let directory = shortcut.parent().ok_or("无效的开始菜单目录。")?;
    fs::create_dir_all(directory).map_err(|_| "无法创建开始菜单目录。")?;
    let mut stage = tempfile::Builder::new()
        .prefix(".dodex-")
        .suffix(".lnk")
        .tempfile_in(directory)
        .map_err(|_| "无法写入 Dodex 开始菜单入口。")?
        .into_temp_path();
    write_shortcut(&stage, &executable, instance)
        .map_err(|error| format!("无法保存 Dodex 开始菜单入口：{error}"))?;
    // Shell inspection and indexing can briefly keep a mapped view of a .lnk
    // after its COM object is released. Retry the same atomic replacement;
    // never delete the destination or truncate a shortcut being read.
    for attempt in 0..6 {
        match stage.persist(shortcut) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == 5 || !matches!(error.error.raw_os_error(), Some(5 | 32 | 33 | 1224)) {
                    return Err(format!("无法更新 Dodex 开始菜单入口：{error}"));
                }
                stage = error.path;
                std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
            }
        }
    }
    unreachable!()
}

fn write_shortcut(
    path: &Path,
    executable: &Path,
    instance: &InstanceConfig,
) -> windows::core::Result<()> {
    let context = |operation: &str, error: windows::core::Error| {
        windows::core::Error::new(error.code(), format!("{operation}: {error}"))
    };
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(wide(executable.as_os_str()).as_ptr()))
            .map_err(|error| context("SetPath", error))?;
        link.SetWorkingDirectory(PCWSTR(
            wide(instance.desktop_user_data.as_os_str()).as_ptr(),
        ))
        .map_err(|error| context("SetWorkingDirectory", error))?;
        link.SetDescription(w!("Dodex - Codex desktop app (second profile)"))
            .map_err(|error| context("SetDescription", error))?;
        link.SetIconLocation(PCWSTR(wide(instance.runtime_app.as_os_str()).as_ptr()), 0)
            .map_err(|error| context("SetIconLocation", error))?;
        link.SetShowCmd(SW_SHOWNORMAL)
            .map_err(|error| context("SetShowCmd", error))?;
        let file: IPersistFile = link.cast()?;
        file.Save(PCWSTR(wide(path.as_os_str()).as_ptr()), true)
            .map_err(|error| context(&format!("Save {}", path.display()), error))?;
        file.SaveCompleted(PCWSTR::null())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
