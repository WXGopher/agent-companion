//! The release ZIP is an explicit allowlist, never an arbitrary directory tree.
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

use serde::{Deserialize, Serialize};
use zip::{CompressionMethod, ZipArchive};

use super::{copy_bounded, hash_file};

pub(super) const FILES: &[&str] = &[
    "agent-companion.exe",
    "acomp.exe",
    "dodex.exe",
    "agent-companion-hook.exe",
    "agent-companion-codex.exe",
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "docs/RESUME.md",
    "docs/terminal-dependency-licenses.md",
    "docs/macos-dual-instance.md",
    "docs/panel.png",
    "docs/windows-usage.png",
    "docs/codex-tui.png",
    "docs/windows-dual.png",
    "docs/macos-menu-bar.png",
    "docs/macos-panel.png",
    "docs/macos-panel-light.png",
    "docs/macos-settings.png",
];
const MAX_UNPACKED: u64 = 768 * 1024 * 1024;
const MAX_EXECUTABLE: u64 = 256 * 1024 * 1024;
const MAX_DOCUMENT: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

pub(super) fn entry_limit(name: &str) -> u64 {
    if name.ends_with(".exe") {
        MAX_EXECUTABLE
    } else {
        MAX_DOCUMENT
    }
}

pub(super) fn validate_manifest(entries: &[Entry]) -> io::Result<()> {
    if entries.len() != FILES.len() {
        return Err(io::Error::other("Package manifest is incomplete"));
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for entry in entries {
        if !FILES.contains(&entry.name.as_str())
            || !seen.insert(&entry.name)
            || entry.size == 0
            || entry.size > entry_limit(&entry.name)
            || super::parse_digest(&entry.sha256).as_deref() != Some(&entry.sha256)
        {
            return Err(io::Error::other(
                "Package manifest contains an invalid entry",
            ));
        }
        total = total
            .checked_add(entry.size)
            .filter(|total| *total <= MAX_UNPACKED)
            .ok_or_else(|| io::Error::other("Unpacked package exceeds the size limit"))?;
    }
    Ok(())
}

pub(super) fn extract(archive: &Path, destination: &Path) -> io::Result<Vec<Entry>> {
    let _pin = super::transaction::pin_directory(destination)?;
    let mut zip = ZipArchive::new(File::open(archive)?).map_err(io::Error::other)?;
    if zip.len() > FILES.len() + 1 {
        return Err(io::Error::other("Package ZIP has unexpected entries"));
    }
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(io::Error::other)?;
        // Reject the raw spelling before a ZIP or Windows path normalizer can
        // fold traversal, backslashes, ADS, device names or trailing dots.
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(io::Error::other)?
            .to_owned();
        if !seen.insert(name.to_ascii_lowercase()) || entry.encrypted() || entry.is_symlink() {
            return Err(io::Error::other(
                "Package ZIP contains duplicate, encrypted or linked entries",
            ));
        }
        if name == "docs/" && entry.is_dir() {
            if entry.size() != 0 {
                return Err(io::Error::other("Package directory contains data"));
            }
            continue;
        }
        if !FILES.contains(&name.as_str())
            || !entry.is_file()
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
            || entry.size() == 0
            || entry.size() > entry_limit(&name)
        {
            return Err(io::Error::other(format!(
                "Unsupported package ZIP entry: {name}"
            )));
        }
        let path = destination.join(&name);
        if name.starts_with("docs/") {
            match fs::create_dir(destination.join("docs")) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let _parent_pin = super::transaction::pin_directory(path.parent().unwrap())?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let declared = entry.size();
        let count = copy_bounded(
            &mut entry,
            &mut output,
            declared.min(MAX_UNPACKED - total),
            |_| {},
        )?;
        if count != declared {
            return Err(io::Error::other(
                "ZIP entry length does not match its header",
            ));
        }
        output.sync_all()?;
        drop(output);
        total += count;
        if name.ends_with(".exe") {
            validate_executable(&path)?;
        }
        entries.push(Entry {
            name,
            size: count,
            sha256: hash_file(&path)?,
        });
    }
    validate_manifest(&entries)?;
    // Keep the GUI last so the gap between retiring it and publishing its
    // replacement is as small as possible during the transaction.
    entries.sort_by_key(|entry| (entry.name == "agent-companion.exe", entry.name.clone()));
    Ok(entries)
}

pub(super) fn validate_executable(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut dos = [0; 64];
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Err(io::Error::other("Package executable is not a PE image"));
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if !(64..=1024 * 1024).contains(&offset) {
        return Err(io::Error::other(
            "Package executable has an invalid PE header",
        ));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut pe = [0; 26];
    file.read_exact(&mut pe)?;
    if &pe[..4] != b"PE\0\0"
        || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664
        || u16::from_le_bytes([pe[24], pe[25]]) != 0x20b
    {
        return Err(io::Error::other(
            "Package executable is not a Windows x86_64 image",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    pub(crate) fn pe(marker: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        bytes.extend_from_slice(marker);
        bytes
    }

    fn make_zip(path: &Path, names: &[&str], directory: bool, method: CompressionMethod) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        let options = SimpleFileOptions::default().compression_method(method);
        if directory {
            writer.add_directory("docs/", options).unwrap();
        }
        for name in names {
            writer.start_file(*name, options).unwrap();
            writer
                .write_all(&if name.ends_with(".exe") {
                    pe(name.as_bytes())
                } else {
                    name.as_bytes().to_vec()
                })
                .unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extracts_stored_and_compress_archive_style_deflated_packages_with_optional_directory() {
        for (method, directory) in [
            (CompressionMethod::Stored, false),
            (CompressionMethod::Deflated, true),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let archive = temporary.path().join("release.zip");
            let output = temporary.path().join("payload");
            fs::create_dir(&output).unwrap();
            make_zip(&archive, FILES, directory, method);
            let entries = extract(&archive, &output).unwrap();
            assert_eq!(entries.len(), FILES.len());
            assert_eq!(entries.last().unwrap().name, "agent-companion.exe");
            for entry in entries {
                assert_eq!(hash_file(&output.join(&entry.name)).unwrap(), entry.sha256);
            }
        }
    }

    #[test]
    fn extracts_a_package_produced_by_native_windows_compress_archive() {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
        };
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("release files 中文");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("docs")).unwrap();
        for name in FILES {
            fs::write(
                source.join(name),
                if name.ends_with(".exe") {
                    pe(name.as_bytes())
                } else {
                    name.as_bytes().to_vec()
                },
            )
            .unwrap();
        }
        let archive = temporary.path().join("release.zip");
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let output = Command::new(powershell).args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $paths=Get-ChildItem -LiteralPath $env:AC_UPDATE_TEST_SOURCE | Select-Object -ExpandProperty FullName; Compress-Archive -LiteralPath $paths -DestinationPath $env:AC_UPDATE_TEST_ARCHIVE -CompressionLevel Fastest"])
            .env("AC_UPDATE_TEST_SOURCE", &source).env("AC_UPDATE_TEST_ARCHIVE", &archive)
            .stdin(Stdio::null()).creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0).output().unwrap();
        assert!(
            output.status.success(),
            "Compress-Archive: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let destination = temporary.path().join("payload");
        fs::create_dir(&destination).unwrap();
        assert_eq!(extract(&archive, &destination).unwrap().len(), FILES.len());
    }

    #[test]
    fn rejects_traversal_windows_aliases_unknown_files_and_case_collisions() {
        for bad in [
            "../outside.exe",
            "/agent-companion.exe",
            "docs\\RESUME.md",
            "README.md:evil",
            "README.md.",
            "CON",
            "docs/../README.md",
            "docs/extra.txt",
            "ACOMP.EXE",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let archive = temporary.path().join("release.zip");
            let output = temporary.path().join("payload");
            fs::create_dir(&output).unwrap();
            let mut names = FILES.to_vec();
            names[1] = bad;
            make_zip(&archive, &names, false, CompressionMethod::Stored);
            assert!(extract(&archive, &output).is_err(), "accepted {bad}");
            assert!(!temporary.path().join("outside.exe").exists());
        }
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("release.zip");
        let output = temporary.path().join("payload");
        fs::create_dir(&output).unwrap();
        make_zip(
            &archive,
            &["README.md", "readme.md"],
            false,
            CompressionMethod::Stored,
        );
        assert!(extract(&archive, &output).is_err());
    }

    #[test]
    fn rejects_symlinks_missing_executables_and_oversized_manifest() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("linked.zip");
        let output = temporary.path().join("payload");
        fs::create_dir(&output).unwrap();
        let mut writer = ZipWriter::new(File::create(&archive).unwrap());
        writer
            .add_symlink(
                "agent-companion.exe",
                "../outside.exe",
                SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(extract(&archive, &output).is_err());
        make_zip(&archive, &["README.md"], false, CompressionMethod::Stored);
        assert!(extract(&archive, &output).is_err());
        let mut entries = FILES
            .iter()
            .map(|name| Entry {
                name: (*name).into(),
                size: 1,
                sha256: "a".repeat(64),
            })
            .collect::<Vec<_>>();
        entries[0].size = MAX_EXECUTABLE + 1;
        assert!(validate_manifest(&entries).is_err());
        entries[0].size = MAX_EXECUTABLE;
        entries[1].size = MAX_EXECUTABLE;
        entries[2].size = MAX_EXECUTABLE;
        assert!(
            validate_manifest(&entries).is_err(),
            "total quota must include documents too"
        );
    }

    #[test]
    fn extraction_never_follows_existing_payload_redirect_or_overwrites_existing_file() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("release.zip");
        let output = temporary.path().join("payload");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("agent-companion.exe"), b"unrelated").unwrap();
        make_zip(&archive, FILES, false, CompressionMethod::Deflated);
        assert!(extract(&archive, &output).is_err());
        assert_eq!(
            fs::read(output.join("agent-companion.exe")).unwrap(),
            b"unrelated"
        );
        assert!(super::super::native::pin_directory(&output.join("../payload")).is_err());
    }

    #[test]
    fn executable_validation_rejects_arm64_and_non_pe_images() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("program.exe");
        let mut bytes = pe(b"ok");
        fs::write(&path, &bytes).unwrap();
        validate_executable(&path).unwrap();
        bytes[68..70].copy_from_slice(&0xaa64u16.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        assert!(validate_executable(&path).is_err());
        fs::write(&path, b"not an executable").unwrap();
        assert!(validate_executable(&path).is_err());
    }
}
