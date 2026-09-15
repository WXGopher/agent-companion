//! Compatibility with existing Atoll installations. New settings always win.
use std::{ffi::OsString, fs, io, path::Path};

/// Read a new environment variable, falling back only when it is absent.
/// An explicitly empty/false new value must not reactivate a legacy setting.
pub fn var_os(name: &str) -> Option<OsString> {
    resolve(name, |key| std::env::var_os(key))
}

fn resolve(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(name).or_else(|| {
        name.strip_prefix("AGENT_COMPANION_")
            .and_then(|suffix| get(&format!("ATOLL_{suffix}")))
    })
}

pub fn var(name: &str) -> Result<String, std::env::VarError> {
    match var_os(name) {
        Some(value) => value.into_string().map_err(std::env::VarError::NotUnicode),
        None => Err(std::env::VarError::NotPresent),
    }
}

/// Copy missing state files only. Leave legacy binaries and originals in place.
/// create_new also protects a new file created by another starting instance.
pub fn migrate_files(legacy: &Path, current: &Path, files: &[&str]) -> io::Result<()> {
    for name in files {
        let source = legacy.join(name);
        let target = current.join(name);
        if target.try_exists()? || !source.try_exists()? {
            continue;
        }
        let mut input = fs::File::open(source)?;
        fs::create_dir_all(current)?;
        let mut output = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = io::copy(&mut input, &mut output).and_then(|_| output.sync_all()) {
            drop(output);
            let _ = fs::remove_file(target);
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_environment_wins_including_empty_and_false_values() {
        let get = |key: &str| match key {
            "AGENT_COMPANION_SKIP_HOOKS" => Some("false".into()),
            "ATOLL_SKIP_HOOKS" => Some("true".into()),
            "AGENT_COMPANION_CONFIG_DIR" => Some("".into()),
            "ATOLL_CONFIG_DIR" => Some("legacy".into()),
            "ATOLL_PIPE_NAME" => Some("legacy-pipe".into()),
            _ => None,
        };
        assert_eq!(
            resolve("AGENT_COMPANION_SKIP_HOOKS", get),
            Some("false".into())
        );
        assert_eq!(resolve("AGENT_COMPANION_CONFIG_DIR", get), Some("".into()));
        assert_eq!(
            resolve("AGENT_COMPANION_PIPE_NAME", get),
            Some("legacy-pipe".into())
        );
    }
    #[test]
    fn migration_copies_without_replacing_new_settings_or_deleting_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("Atoll");
        let new = dir.path().join("AgentCompanion");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("config.json"), r#"{"unknown":true}"#).unwrap();
        migrate_files(&old, &new, &["config.json", "missing.json"]).unwrap();
        assert_eq!(
            fs::read(new.join("config.json")).unwrap(),
            fs::read(old.join("config.json")).unwrap()
        );
        fs::write(new.join("config.json"), "new").unwrap();
        migrate_files(&old, &new, &["config.json"]).unwrap();
        assert_eq!(fs::read_to_string(new.join("config.json")).unwrap(), "new");
        assert!(old.join("config.json").is_file());
        assert!(!new.join("missing.json").exists());
    }
}
