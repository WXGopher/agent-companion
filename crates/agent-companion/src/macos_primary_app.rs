//! The native Foundation resolver is shared with the CLI fallback. Its only
//! candidates are Codex.app and ChatGPT.app in the two Applications directories.
use std::ffi::{CStr, CString, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn agent_companion_find_primary_codex_app(
        system_applications: *const c_char,
        user_applications: *const c_char,
    ) -> *mut c_char;
    fn agent_companion_release_primary_app_path(path: *mut c_char);
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
pub struct Application {
    pub app: PathBuf,
    pub executable: Option<PathBuf>,
}

pub fn discover(system_applications: &Path, user_applications: &Path) -> Option<Application> {
    let system = CString::new(system_applications.as_os_str().as_bytes()).ok()?;
    let user = CString::new(user_applications.as_os_str().as_bytes()).ok()?;
    // SAFETY: both inputs remain valid NUL-terminated strings during this call.
    // The native resolver returns a distinct allocation or null and has no UI.
    let pointer = unsafe { agent_companion_find_primary_codex_app(system.as_ptr(), user.as_ptr()) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the bridge returns a NUL-terminated string owned until its paired
    // release function is called. Copy before releasing with the same allocator.
    let result = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes()).ok();
    unsafe { agent_companion_release_primary_app_path(pointer) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fixture(app: &Path, identity: &str) {
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        fs::write(app.join("Contents/Info.plist"), format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{identity}</string></dict></plist>"
        )).unwrap();
        for relative in ["Contents/MacOS/ChatGPT", "Contents/Resources/codex"] {
            let path = app.join(relative);
            fs::write(&path, "synthetic executable, never run").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn renamed_primary_is_discovered_without_selecting_other_chatgpt_or_dodex() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let system = root.join("Applications");
        let user = root.join("user/Applications");
        let renamed = system.join("ChatGPT.app");
        fixture(&renamed, "com.openai.codex");
        let found = discover(&system, &user).unwrap();
        assert_eq!(found.app, renamed);
        assert_eq!(
            found.executable,
            Some(renamed.join("Contents/Resources/codex"))
        );
        let standard = user.join("Codex.app");
        fixture(&standard, "com.openai.codex");
        assert_eq!(discover(&system, &user).unwrap().app, standard);
        fixture(&standard, "different.application");
        assert_eq!(discover(&system, &user).unwrap().app, renamed);
        fixture(&renamed, "different.application");
        let hidden = system.join(".Dodex/Dodex.app");
        fixture(&hidden, "com.openai.codex");
        assert_eq!(discover(&system, &user), None);
        fs::remove_dir_all(&renamed).unwrap();
        symlink(&hidden, &renamed).unwrap();
        assert_eq!(discover(&system, &user), None);
    }
}
