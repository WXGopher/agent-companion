//! Explicit, one-file profile copies. Credential files and keychains are never opened.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const ISOLATION_KEYS: [&str; 3] = ["cli_auth_credentials_store", "sqlite_home", "log_dir"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Config,
    Instructions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToSecondary,
    ToPrimary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfilePaths {
    pub config: PathBuf,
    pub instructions: PathBuf,
    /// An override takes precedence over AGENTS.md. Only its presence is inspected.
    pub instructions_override: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsolationPaths {
    pub sqlite_home: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfilePair {
    pub primary: ProfilePaths,
    pub secondary: ProfilePaths,
    pub secondary_isolation: IsolationPaths,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncOutcome {
    pub changed: bool,
    pub backup_path: Option<PathBuf>,
}

/// Resolve the two supported names without opening configuration or credentials.
pub fn profile_paths(home: &Path) -> io::Result<ProfilePaths> {
    validate_path(home)?;
    let override_path = home.join("AGENTS.override.md");
    let instructions_override = match fs::symlink_metadata(&override_path) {
        Ok(_) => Some(override_path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(ProfilePaths {
        config: home.join("config.toml"),
        instructions: home.join("AGENTS.md"),
        instructions_override,
    })
}

/// Copy exactly one selected file. The deployment caller must hold its operation
/// lock and revalidate the secondary deployment before entering this function.
pub fn sync_file(
    pair: &ProfilePair,
    kind: FileKind,
    direction: Direction,
) -> io::Result<SyncOutcome> {
    validate_pair(pair)?;
    let path = |profile: &ProfilePaths| match kind {
        FileKind::Config => profile.config.clone(),
        FileKind::Instructions => profile.instructions.clone(),
    };
    let (source, destination) = match direction {
        Direction::ToSecondary => (path(&pair.primary), path(&pair.secondary)),
        Direction::ToPrimary => (path(&pair.secondary), path(&pair.primary)),
    };
    let original_source = read_optional(&source)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "The selected source file does not exist.",
        )
    })?;
    let original_destination = read_optional(&destination)?;
    let replacement = match kind {
        FileKind::Config => sync_configuration(
            &original_source,
            original_destination.as_deref(),
            direction,
            &pair.secondary_isolation,
        )?,
        FileKind::Instructions => original_source.clone(),
    };
    if original_destination.as_deref() == Some(replacement.as_slice()) {
        return Ok(SyncOutcome {
            changed: false,
            backup_path: None,
        });
    }
    let revalidate = || {
        validate_pair(pair)?;
        if read_optional(&source)?.as_deref() != Some(original_source.as_slice())
            || read_optional(&destination)? != original_destination
        {
            return Err(io::Error::other(
                "A profile file changed during sync. Try again.",
            ));
        }
        Ok(())
    };
    let backup_path = save_atomic(
        &destination,
        original_destination.as_deref(),
        &replacement,
        revalidate,
    )?;
    Ok(SyncOutcome {
        changed: true,
        backup_path,
    })
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_config(bytes: &[u8]) -> io::Result<DocumentMut> {
    // TOML parser diagnostics quote input. Never include them in a UI error.
    std::str::from_utf8(bytes)
        .map_err(|_| invalid("A configuration file is not valid UTF-8."))?
        .parse()
        .map_err(|_| invalid("A configuration file contains invalid TOML. No file was changed."))
}

fn sync_configuration(
    source: &[u8],
    destination: Option<&[u8]>,
    direction: Direction,
    isolation: &IsolationPaths,
) -> io::Result<Vec<u8>> {
    let source_bytes = source;
    let mut source = parse_config(source_bytes)?;
    let destination = destination
        .map(parse_config)
        .transpose()?
        .unwrap_or_default();
    if destination.to_string().as_bytes() == source_bytes
        && (direction == Direction::ToPrimary || matches_secondary_isolation(&source, isolation))
    {
        return Ok(source_bytes.to_vec());
    }
    // Remove every source override first, including inline tables and arrays.
    // Reverse sync must also remove the secondary's defaults when the primary
    // previously had no explicit isolation settings.
    strip_isolation(source.as_item_mut());
    overlay_isolation(source.as_item_mut(), destination.as_item())?;
    if direction == Direction::ToSecondary {
        // Restore isolation even for a missing destination or a new source profile.
        enforce_isolation(source.as_item_mut(), isolation);
        for key in ISOLATION_KEYS {
            source[key] = toml_edit::value(isolation_value(key, isolation));
        }
    }
    let bytes = source.to_string().into_bytes();
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(invalid(
            "The resulting configuration exceeds the sync size limit.",
        ));
    }
    Ok(bytes)
}

fn matches_secondary_isolation(document: &DocumentMut, paths: &IsolationPaths) -> bool {
    fn item_matches(item: &Item, paths: &IsolationPaths) -> bool {
        if let Some(table) = item.as_table_like() {
            table.iter().all(|(key, child)| {
                (!is_isolation(key) || child.as_str() == Some(isolation_value(key, paths).as_str()))
                    && item_matches(child, paths)
            })
        } else if let Some(tables) = item.as_array_of_tables() {
            tables
                .iter()
                .all(|table| item_matches(&Item::Table(table.clone()), paths))
        } else {
            item.as_value()
                .is_none_or(|value| value_matches(value, paths))
        }
    }
    fn value_matches(value: &Value, paths: &IsolationPaths) -> bool {
        if let Some(table) = value.as_inline_table() {
            table.iter().all(|(key, child)| {
                (!is_isolation(key) || child.as_str() == Some(isolation_value(key, paths).as_str()))
                    && value_matches(child, paths)
            })
        } else {
            value
                .as_array()
                .is_none_or(|array| array.iter().all(|child| value_matches(child, paths)))
        }
    }
    ISOLATION_KEYS.iter().all(|key| {
        document.get(key).and_then(Item::as_str) == Some(isolation_value(key, paths).as_str())
    }) && item_matches(document.as_item(), paths)
}

fn is_isolation(key: &str) -> bool {
    ISOLATION_KEYS.contains(&key)
}

fn strip_isolation(item: &mut Item) {
    if let Some(table) = item.as_table_like_mut() {
        for key in ISOLATION_KEYS {
            table.remove(key);
        }
        for (_, child) in table.iter_mut() {
            strip_isolation(child);
        }
    } else if let Some(tables) = item.as_array_of_tables_mut() {
        for table in tables.iter_mut() {
            strip_table(table);
        }
    } else if let Some(value) = item.as_value_mut() {
        strip_value(value);
    }
}

fn strip_table(table: &mut dyn TableLike) {
    for key in ISOLATION_KEYS {
        table.remove(key);
    }
    for (_, child) in table.iter_mut() {
        strip_isolation(child);
    }
}

fn strip_value(value: &mut Value) {
    if let Some(table) = value.as_inline_table_mut() {
        strip_table(table);
    } else if let Some(array) = value.as_array_mut() {
        for child in array.iter_mut() {
            strip_value(child);
        }
    }
}

fn has_isolation(item: &Item) -> bool {
    if let Some(table) = item.as_table_like() {
        table
            .iter()
            .any(|(key, child)| is_isolation(key) || has_isolation(child))
    } else if let Some(tables) = item.as_array_of_tables() {
        tables.iter().any(|table| {
            table
                .iter()
                .any(|(key, child)| is_isolation(key) || has_isolation(child))
        })
    } else {
        item.as_value().is_some_and(value_has_isolation)
    }
}

fn value_has_isolation(value: &Value) -> bool {
    if let Some(table) = value.as_inline_table() {
        table
            .iter()
            .any(|(key, child)| is_isolation(key) || value_has_isolation(child))
    } else {
        value
            .as_array()
            .is_some_and(|array| array.iter().any(value_has_isolation))
    }
}

fn incompatible() -> io::Error {
    invalid("The configurations use incompatible structures for instance isolation settings.")
}

fn overlay_table(source: &mut dyn TableLike, target: &dyn TableLike) -> io::Result<()> {
    for (key, child) in target.iter() {
        if is_isolation(key) {
            source.insert(key, child.clone());
        } else if has_isolation(child) {
            if !source.contains_key(key) {
                source.insert(key, empty_item(child));
            }
            overlay_isolation(source.get_mut(key).ok_or_else(incompatible)?, child)?;
        }
    }
    Ok(())
}

fn empty_item(item: &Item) -> Item {
    match item {
        Item::Table(_) => Item::Table(Table::new()),
        Item::ArrayOfTables(_) => Item::ArrayOfTables(ArrayOfTables::new()),
        Item::Value(value) => Item::Value(empty_value(value)),
        Item::None => Item::None,
    }
}

fn empty_value(value: &Value) -> Value {
    if value.is_array() {
        Value::Array(Array::new())
    } else {
        Value::InlineTable(InlineTable::new())
    }
}

fn overlay_isolation(source: &mut Item, target: &Item) -> io::Result<()> {
    if let Some(target) = target.as_table_like() {
        overlay_table(source.as_table_like_mut().ok_or_else(incompatible)?, target)
    } else if let Some(target) = target.as_array_of_tables() {
        let source = source.as_array_of_tables_mut().ok_or_else(incompatible)?;
        for (index, table) in target.iter().enumerate() {
            if !table
                .iter()
                .any(|(key, child)| is_isolation(key) || has_isolation(child))
            {
                continue;
            }
            while source.len() <= index {
                source.push(Table::new());
            }
            overlay_table(source.get_mut(index).ok_or_else(incompatible)?, table)?;
        }
        Ok(())
    } else if let Some(target) = target.as_value() {
        overlay_value(source.as_value_mut().ok_or_else(incompatible)?, target)
    } else {
        Ok(())
    }
}

fn overlay_value(source: &mut Value, target: &Value) -> io::Result<()> {
    if let Some(target) = target.as_inline_table() {
        overlay_table(
            source.as_inline_table_mut().ok_or_else(incompatible)?,
            target,
        )
    } else if let Some(target) = target.as_array() {
        let source = source.as_array_mut().ok_or_else(incompatible)?;
        for (index, value) in target.iter().enumerate() {
            if !value_has_isolation(value) {
                continue;
            }
            while source.len() <= index {
                let placeholder = target.get(source.len()).ok_or_else(incompatible)?;
                if !placeholder.is_array() && !placeholder.is_inline_table() {
                    return Err(incompatible());
                }
                source.push(empty_value(placeholder));
            }
            overlay_value(source.get_mut(index).ok_or_else(incompatible)?, value)?;
        }
        Ok(())
    } else {
        Ok(())
    }
}

fn isolation_value(key: &str, paths: &IsolationPaths) -> String {
    match key {
        "cli_auth_credentials_store" => "file".into(),
        "sqlite_home" => paths.sqlite_home.to_string_lossy().into_owned(),
        "log_dir" => paths.log_dir.to_string_lossy().into_owned(),
        _ => unreachable!(),
    }
}

fn enforce_isolation(item: &mut Item, paths: &IsolationPaths) {
    if let Some(table) = item.as_table_like_mut() {
        enforce_table(table, paths);
    } else if let Some(tables) = item.as_array_of_tables_mut() {
        for table in tables.iter_mut() {
            enforce_table(table, paths);
        }
    } else if let Some(value) = item.as_value_mut() {
        enforce_value(value, paths);
    }
}

fn enforce_table(table: &mut dyn TableLike, paths: &IsolationPaths) {
    for (key, child) in table.iter_mut() {
        if is_isolation(key.get()) {
            *child = toml_edit::value(isolation_value(key.get(), paths));
        } else {
            enforce_isolation(child, paths);
        }
    }
}

fn enforce_value(value: &mut Value, paths: &IsolationPaths) {
    if let Some(table) = value.as_inline_table_mut() {
        enforce_table(table, paths);
    } else if let Some(array) = value.as_array_mut() {
        for child in array.iter_mut() {
            enforce_value(child, paths);
        }
    }
}

fn validate_pair(pair: &ProfilePair) -> io::Result<()> {
    for profile in [&pair.primary, &pair.secondary] {
        if profile.config.file_name() != Some("config.toml".as_ref())
            || profile.instructions.file_name() != Some("AGENTS.md".as_ref())
            || profile.config.parent() != profile.instructions.parent()
        {
            return Err(invalid(
                "Only config.toml and global AGENTS.md can be synced.",
            ));
        }
    }
    let primary = pair.primary.config.parent().ok_or_else(incompatible)?;
    let secondary = pair.secondary.config.parent().ok_or_else(incompatible)?;
    let comparable = |path: &Path| {
        // Case aliases and Windows short names can name the same directory
        // without being symlinks or adding a hard link to either file.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        #[cfg(windows)]
        {
            PathBuf::from(path.to_string_lossy().to_lowercase())
        }
        #[cfg(not(windows))]
        {
            path
        }
    };
    let (primary, secondary) = (comparable(primary), comparable(secondary));
    if primary.starts_with(&secondary) || secondary.starts_with(&primary) {
        return Err(invalid(
            "Codex and Dodex profile directories must not overlap.",
        ));
    }
    for path in [
        &pair.primary.config,
        &pair.primary.instructions,
        &pair.secondary.config,
        &pair.secondary.instructions,
    ] {
        validate_path(path)?;
    }
    Ok(())
}

fn validate_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid(
            "Profile paths must be absolute and cannot contain parent components.",
        ));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if redirected(&metadata) {
                    return Err(invalid("Profile paths cannot contain links or redirects."));
                }
                if !metadata.is_dir() && !metadata.is_file() {
                    return Err(invalid(
                        "Profile paths must name regular files or directories.",
                    ));
                }
                if metadata.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if metadata.nlink() > 1 {
                            return Err(invalid("Profile files cannot use hard links."));
                        }
                    }
                    #[cfg(windows)]
                    validate_handle(&open_file(ancestor, false)?)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn redirected(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn open_file(path: &Path, write: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(not(target_os = "macos"))]
        options.custom_flags(0x20000 | 0x800); // O_NOFOLLOW | O_NONBLOCK on Linux
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    options.open(path)
}

fn validate_handle(file: &File) -> io::Result<fs::Metadata> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || redirected(&metadata) {
        return Err(invalid("A profile file is not a regular, isolated file."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(invalid("Profile files cannot use hard links."));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // Stable std does not expose the link count on Windows. This fixed ABI
        // structure contains DWORDs only, including the three FILETIME pairs.
        #[repr(C)]
        #[derive(Default)]
        struct FileInformation {
            words: [u32; 13],
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                information: *mut FileInformation,
            ) -> i32;
        }
        let mut information = FileInformation::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if information.words[10] > 1 {
            return Err(invalid("Profile files cannot use hard links."));
        }
    }
    Ok(metadata)
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    validate_path(path)?;
    let mut file = match open_file(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = validate_handle(&file)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(invalid("A profile file exceeds the sync size limit."));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(invalid("A profile file exceeds the sync size limit."));
    }
    Ok(Some(bytes))
}

fn save_atomic(
    path: &Path,
    original: Option<&[u8]>,
    replacement: &[u8],
    revalidate: impl Fn() -> io::Result<()>,
) -> io::Result<Option<PathBuf>> {
    revalidate()?;
    let permissions = if original.is_some() {
        let file = open_file(path, true)?;
        let permissions = validate_handle(&file)?.permissions();
        if permissions.readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "The destination file is read-only.",
            ));
        }
        Some(permissions)
    } else {
        None
    };
    let parent = path.parent().ok_or_else(incompatible)?;
    validate_path(parent)?;
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o700);
    }
    directories.create(parent)?;
    validate_path(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(replacement)?;
    if let Some(permissions) = &permissions {
        temporary.as_file().set_permissions(permissions.clone())?;
    }
    temporary.as_file().sync_all()?;
    revalidate()?;
    let backup = if let Some(original) = original {
        let backup_path = super::backup_path_for(path);
        let mut backup = tempfile::NamedTempFile::new_in(parent)?;
        backup.write_all(original)?;
        if let Some(permissions) = &permissions {
            backup.as_file().set_permissions(permissions.clone())?;
        }
        backup.as_file().sync_all()?;
        backup
            .persist_noclobber(&backup_path)
            .map_err(|error| error.error)?;
        Some(backup_path)
    } else {
        None
    };
    revalidate()?;
    if original.is_some() {
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    }
    Ok(backup)
}

#[cfg(test)]
mod tests;
