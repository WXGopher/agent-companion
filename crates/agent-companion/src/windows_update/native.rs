//! Held Windows identities: no path-based replacement of an image in use.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path},
};

use windows::{
    Win32::{
        Foundation::{FILETIME, GENERIC_READ, HANDLE},
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo,
            SetFileInformationByHandle,
        },
        System::Threading::{GetProcessTimes, PROCESS_NAME_WIN32, QueryFullProcessImageNameW},
    },
    core::PWSTR,
};

pub(super) struct DirectoryPins {
    _files: Vec<File>,
}

pub(super) fn plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
        && !path.as_os_str().encode_wide().any(|ch| ch == 0)
}

pub(super) fn pin_directory(path: &Path) -> io::Result<DirectoryPins> {
    if !plain_absolute(path) {
        return Err(io::Error::other(
            "Update directory is not an absolute, plain path",
        ));
    }
    let mut files = Vec::new();
    for directory in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(directory)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::other(
                "Update directory contains a link or junction",
            ));
        }
        files.push(file);
    }
    Ok(DirectoryPins { _files: files })
}

pub(super) fn open_held(path: &Path, rename: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ.0 | if rename { DELETE.0 } else { 0 })
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(io::Error::other(
            "Update entry is not a regular, unredirected file",
        ));
    }
    Ok(file)
}

pub(super) fn fingerprint(file: &mut File) -> io::Result<super::transaction::Fingerprint> {
    let size = file.metadata()?.len();
    // Every held entry comes from the bounded package allowlist. This also
    // prevents an unrelated enormous local file from stalling initialization.
    if size > 256 * 1024 * 1024 {
        return Err(io::Error::other("Installed package entry is too large"));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(super::transaction::Fingerprint {
        size,
        sha256: super::hash_reader(file)?,
    })
}

pub(super) fn rename(file: &File, target: &Path) -> io::Result<()> {
    if !plain_absolute(target) {
        return Err(io::Error::other("Invalid update rename destination"));
    }
    let name: Vec<u16> = target.as_os_str().encode_wide().collect();
    let filename_bytes = u32::try_from(name.len() * size_of::<u16>()).map_err(io::Error::other)?;
    let length =
        offset_of!(FILE_RENAME_INFO, FileName) + filename_bytes as usize + size_of::<u16>();
    let mut buffer = vec![0usize; length.div_ceil(size_of::<usize>())];
    unsafe {
        let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = HANDLE::default();
        (*information).FileNameLength = filename_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfo,
            information.cast(),
            length as u32,
        )
        .map_err(win_error)
    }
}

pub(super) fn rename_bounded(file: &File, target: &Path) -> io::Result<()> {
    for attempt in 0..10 {
        match rename(file, target) {
            Err(error) if attempt < 9 && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
    unreachable!()
}

pub(super) fn process_created(process: HANDLE) -> io::Result<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(win_error)?;
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

pub(super) fn process_image(process: HANDLE) -> io::Result<std::path::PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(win_error)?;
    Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length as usize],
    )))
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    // QueryFullProcessImageName returns an ordinary DOS spelling, whereas
    // canonicalize/current_exe can use the verbatim DOS prefix.
    let normalize = |path: &Path| {
        let value = path.to_string_lossy().replace('/', "\\");
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_lowercase()
    };
    normalize(left) == normalize(right)
}

pub(super) fn regular_or_absent(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 =>
        {
            Ok(true)
        }
        Ok(_) => Err(io::Error::other(
            "Update destination is a link, junction or directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn win_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
