//! Managed native launchers: a GUI copy for Start and a console copy for PATH.
//! Both are self-contained and can repair each other without a build directory.
use super::{no_links, no_redirects, read_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub(super) enum Kind {
    Desktop = 2,
    Console = 3,
}

pub(super) fn kind(path: &Path) -> std::io::Result<Kind> {
    read_subsystem(&mut File::open(path)?).map(|(_, kind)| kind)
}

pub(super) const MARKER: &str = "dodex.agent-companion.json";
pub(super) const OWNER: &str = "agent-companion/dodex";

#[derive(Serialize, Deserialize)]
pub(super) struct Ownership {
    pub(super) schema: u32,
    owner: String,
    // During replacement both hashes are accepted, so interruption between the
    // two atomic writes can be repaired without claiming an unrelated file.
    pub(super) hashes: Vec<String>,
    #[serde(default)]
    pub(super) entry_revision: u32,
    #[serde(default)]
    pub(super) update_route: String,
}

pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    key(a) == key(b)
}

pub(super) fn conflict(path: &Path) -> String {
    format!(
        "发现已有或被修改的 dodex 命令：{}。未覆盖；请先移动或重命名该命令，再重试。",
        path.display()
    )
}

pub(super) fn ownership(directory: &Path) -> Result<Option<Ownership>, String> {
    let marker = directory.join(MARKER);
    no_redirects(&marker)?;
    if !marker.exists() {
        return Ok(None);
    }
    let owner: Ownership = read_json(&marker).map_err(|_| conflict(&marker))?;
    if !matches!(owner.schema, 1 | 2)
        || owner.schema == 2 && (owner.entry_revision != 2 || owner.update_route != "companion")
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

pub(super) fn file_hash(path: &Path) -> Result<String, String> {
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

pub(super) fn save_ownership(
    directory: &Path,
    hashes: Vec<String>,
    existing: bool,
) -> Result<(), String> {
    let mut stage =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法保存 dodex 命令登记。")?;
    serde_json::to_writer(
        &mut stage,
        &Ownership {
            schema: 2,
            owner: OWNER.into(),
            hashes,
            entry_revision: 2,
            update_route: "companion".into(),
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

pub(super) fn install_command(source: &Path, directory: &Path, kind: Kind) -> Result<(), String> {
    let target = directory.join("dodex.exe");
    let owner = ownership(directory)?;
    // Cargo and some package managers hard-link immutable source executables.
    // Copy their bytes, then require the installed command to be independent.
    let source_hash = file_hash_with(source, true)?;
    let previous_hash = if target.exists() {
        Some(file_hash(&target)?)
    } else {
        None
    };
    if let Some(hash) = &previous_hash
        && !owner
            .as_ref()
            .is_some_and(|owner| owner.hashes.contains(hash))
    {
        return Err(conflict(&target));
    }
    let mut stage =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法写入 dodex 命令。")?;
    fs::copy(source, stage.path()).map_err(|_| "无法复制 dodex 命令。")?;
    if file_hash(stage.path())? != source_hash {
        return Err("Companion 程序在复制时改变，请重试。".into());
    }
    set_subsystem(stage.as_file_mut(), kind).map_err(|_| "无法生成 dodex 启动器。")?;
    let new_hash = file_hash(stage.path())?;
    if previous_hash.as_ref() == Some(&new_hash) {
        // Subsystem conversion is idempotent. In particular, dodex --deploy
        // must not replace its running self or rewrite its ownership marker.
        if owner.as_ref().is_some_and(|owner| {
            owner.schema != 2 || owner.entry_revision != 2 || owner.update_route != "companion"
        }) {
            save_ownership(directory, vec![new_hash], true)?;
        }
        return Ok(());
    }
    let mut hashes = vec![new_hash.clone()];
    if let Some(hash) = &previous_hash {
        hashes.push(hash.clone());
    }
    save_ownership(directory, hashes, owner.is_some())?;
    let result = if previous_hash.is_some() {
        stage.persist(&target)
    } else {
        stage.persist_noclobber(&target)
    };
    result.map_err(|_| "无法更新 dodex 命令；请等待正在执行的 dodex 退出后重试。")?;
    save_ownership(directory, vec![new_hash], true)
}

/// PE32 and PE32+ share these header offsets. Read the launch mode explicitly;
/// it must not depend on inherited consoles, redirected IO, or the parent process.
fn read_subsystem(file: &mut File) -> std::io::Result<(u64, Kind)> {
    let invalid = || std::io::Error::other("invalid Companion PE executable");
    let mut dos = [0; 64];
    file.rewind()?;
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Err(invalid());
    }
    let pe_offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if pe_offset < dos.len() as u64 {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(pe_offset))?;
    let mut pe = [0; 24];
    file.read_exact(&mut pe)?;
    let optional_size = u16::from_le_bytes(pe[20..22].try_into().unwrap()) as u64;
    let characteristics = u16::from_le_bytes(pe[22..24].try_into().unwrap());
    let optional_offset = pe_offset + pe.len() as u64;
    if &pe[..4] != b"PE\0\0"
        || characteristics & 0x0002 == 0 // IMAGE_FILE_EXECUTABLE_IMAGE
        || characteristics & 0x2000 != 0 // IMAGE_FILE_DLL
        || optional_size < 70
        || optional_offset + optional_size > file.metadata()?.len()
    {
        return Err(invalid());
    }
    let mut optional = [0; 70];
    file.read_exact(&mut optional)?;
    let minimum_size = match u16::from_le_bytes(optional[..2].try_into().unwrap()) {
        0x10b => 96,  // PE32
        0x20b => 112, // PE32+
        _ => return Err(invalid()),
    };
    let subsystem = u16::from_le_bytes(optional[68..70].try_into().unwrap());
    if optional_size < minimum_size || !matches!(subsystem, 2 | 3) {
        return Err(invalid());
    }
    Ok((
        optional_offset,
        if subsystem == 2 {
            Kind::Desktop
        } else {
            Kind::Console
        },
    ))
}

pub(super) fn set_subsystem(file: &mut File, kind: Kind) -> std::io::Result<()> {
    let (optional_offset, _) = read_subsystem(file)?;
    file.seek(SeekFrom::Start(optional_offset + 64))?;
    // Windows accepts a zero checksum for ordinary user-mode executables.
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&(kind as u16).to_le_bytes())?;
    file.flush()
}
