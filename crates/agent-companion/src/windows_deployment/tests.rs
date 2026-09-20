use super::*;

#[test]
fn runtime_checks_preserve_powershell_quotes_and_path_arguments() {
    let result = powershell("if ('CN=\"OpenAI OpCo, LLC\", O=\"OpenAI OpCo, LLC\", C=US' -notmatch 'O=\"?OpenAI OpCo, LLC\"?,') { exit 1 }; Write-Output $env:COMPANION_RUNTIME_CHECK", Some(Path::new("C:\\example space\\a&b's"))).unwrap();
    assert_eq!(result, "C:\\example space\\a&b's");
}

#[test]
fn runtime_checks_can_load_windows_signature_tools() {
    assert_eq!(powershell("$ErrorActionPreference='Stop'; Import-Module Microsoft.PowerShell.Security; (Get-Command Get-AuthenticodeSignature).Name", None).unwrap(), "Get-AuthenticodeSignature");
}

#[test]
fn profile_overrides_cannot_escape_file_credentials_or_database() {
    let root = tempfile::tempdir().unwrap();
    let instance = InstanceConfig::at(root.path());
    let config = configuration(&instance);
    assert!(validate_config(&config, &instance).is_ok());
    for extra in [
        "\n[profiles.work]\ncli_auth_credentials_store = 'keyring'\n",
        "\n[profiles.work]\nsqlite_home = 'C:\\shared'\n",
        "\nprofiles = { work = { log_dir = 'shared' } }\n",
    ] {
        assert!(validate_config(&(config.clone() + extra), &instance).is_err());
    }
    assert!(validate_config(&config.replace("\"file\"", "\"auto\""), &instance).is_err());
}

#[test]
fn paths_are_compared_case_insensitively_with_component_boundaries() {
    assert!(paths_overlap(
        Path::new("C:\\Users\\Me\\Dodex"),
        Path::new("C:\\Users\\Me\\old\\..\\Dodex\\sqlite")
    ));
    assert!(paths_overlap(
        Path::new("C:\\Users\\Me\\Dodex"),
        Path::new("c:/users/me/dodex/sqlite")
    ));
    assert!(!paths_overlap(
        Path::new("C:\\Users\\Me\\Dodex"),
        Path::new("C:\\Users\\Me\\Dodex-old")
    ));
    assert!(no_redirects(Path::new("C:\\Users\\Me\\..\\Dodex")).is_err());
}

#[test]
fn usage_and_launcher_commands_bind_paths_and_do_not_inherit_overrides() {
    let command = isolated_command(
        Path::new("C:\\runtime\\codex.exe"),
        Path::new("C:\\Dodex\\home"),
        Path::new("C:\\Dodex\\sqlite"),
    );
    let env: std::collections::HashMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env.get(std::ffi::OsStr::new("CODEX_HOME"))
            .copied()
            .flatten(),
        Some(std::ffi::OsStr::new("C:\\Dodex\\home"))
    );
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "CODEX_THREAD_ID",
        "CODEX_CLI_PATH",
        "NODE_OPTIONS",
        "ELECTRON_RUN_AS_NODE",
    ] {
        assert!(!env.contains_key(std::ffi::OsStr::new(key)));
    }
}

#[test]
fn deployment_rejects_unknown_and_redirected_existing_profiles() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("config.toml"), "personal = true").unwrap();
    assert!(validate(temp.path(), false).is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("config.toml")).unwrap(),
        "personal = true"
    );
    let shared = temp.path().join("shared.toml");
    fs::hard_link(temp.path().join("config.toml"), &shared).unwrap();
    assert!(no_redirects(&shared).is_err());
}

#[test]
fn packaged_hardlinks_are_copied_as_independent_runtime_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("original.dll"), b"signed package contents").unwrap();
    fs::hard_link(source.join("original.dll"), source.join("linked.dll")).unwrap();
    let target = temp.path().join("copy");
    copy_tree(&source, &target).unwrap();
    assert!(no_redirects(&target.join("linked.dll")).is_ok());
    fs::write(target.join("linked.dll"), b"independent").unwrap();
    assert_eq!(
        fs::read(source.join("linked.dll")).unwrap(),
        b"signed package contents"
    );
    assert_eq!(
        fs::read(target.join("original.dll")).unwrap(),
        b"signed package contents"
    );
}
