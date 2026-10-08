use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, TableLike};

use super::{Environment, Session, display_path, safe_text};

pub(super) struct Settings {
    pub model: Option<String>,
    pub database_home: PathBuf,
    pub profile: Option<String>,
    pub details: Vec<(String, String)>,
    pub blockers: Vec<String>,
    pub forced_account: Option<String>,
    pub disabled_mcp: Vec<String>,
}

pub(super) fn inspect(
    source: &Environment,
    session: &Session,
    profile: Option<&str>,
) -> Result<Settings, String> {
    #[cfg(target_os = "macos")]
    let managed = vec![
        PathBuf::from("/Library/Managed Preferences/com.openai.codex.plist"),
        user_home().join("Library/Managed Preferences/com.openai.codex.plist"),
    ];
    #[cfg(not(target_os = "macos"))]
    let managed = Vec::new();
    inspect_with_policy(source, session, profile, &system_directory(), &managed)
}

fn inspect_with_policy(
    source: &Environment,
    session: &Session,
    profile: Option<&str>,
    system: &Path,
    managed: &[PathBuf],
) -> Result<Settings, String> {
    let mut settings = Settings {
        model: None,
        database_home: super::discovery::database_home(source),
        profile: profile.map(str::to_owned),
        details: Vec::new(),
        blockers: Vec::new(),
        forced_account: None,
        disabled_mcp: Vec::new(),
    };
    // Native 0.159.3 deliberately opens this OAuth fallback with O_NOFOLLOW;
    // a home overlay cannot promise refresh writeback through its link.
    if source.home.join(".credentials.json").exists() {
        settings
            .blockers
            .push("来源包含本地 MCP OAuth 凭据，其刷新写入不能保持原位置；当前禁用接力。".into());
    }
    if let Some(profile) = profile
        && (profile.is_empty()
            || !profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
    {
        return Err("Profile 名称仅支持字母、数字、下划线和连字符。".into());
    }
    for path in [
        system.join("managed_config.toml"),
        system.join("requirements.toml"),
    ] {
        if path.exists() {
            settings
                .details
                .push(("系统策略（原生执行）".into(), display_path(&path)));
            settings
                .blockers
                .push("存在系统或组织托管策略；其有效配置尚未验证，接力已禁用。".into());
        }
    }
    for path in managed {
        if path.exists() {
            settings
                .blockers
                .push("存在 macOS 托管配置，无法确认账号切换后的有效策略。".into());
        }
    }
    let mut layers = Vec::new();
    if system.join("config.toml").exists() {
        layers.push(("系统配置", system.join("config.toml"), false));
    }
    layers.push(("来源用户配置", source.home.join("config.toml"), true));
    if let Some(profile) = profile {
        settings.blockers.push(
            "命名 Profile 的原生认证与配置预检尚不支持；保留该 Profile，当前禁用接力。".into(),
        );
        let path = source.home.join(format!("{profile}.config.toml"));
        if !path.is_file() {
            settings
                .blockers
                .push("所选命名 Profile 文件不存在。".into());
        }
        layers.push(("来源 Profile（覆盖用户配置）", path, true));
    }
    let mut trusted = false;
    let user = parse(&source.home.join("config.toml"))?;
    if let Some(projects) = user
        .as_ref()
        .and_then(|doc| doc.get("projects"))
        .and_then(Item::as_table_like)
    {
        for (directory, value) in projects.iter() {
            if session.cwd.starts_with(directory)
                && value.get("trust_level").and_then(Item::as_str) == Some("trusted")
            {
                trusted = true;
            }
        }
    }
    // Match the pinned native runtime's default project boundary. An account's
    // .codex above the nearest Git root is not a project layer for this session.
    for directory in project_directories(&session.cwd) {
        let path = directory.join(".codex/config.toml");
        // The source home can also be an ancestor; do not apply its config twice.
        if path == source.home.join("config.toml") {
            continue;
        }
        if path.exists() {
            settings
                .blockers
                .push("存在项目配置覆盖，其原生加载与信任顺序尚未验证；当前禁用接力。".into());
            if !trusted {
                settings
                    .blockers
                    .push("发现项目配置，但无法确认其原生信任层级；接力已禁用。".into());
            }
            layers.push(("项目配置（按目录由外到内）", path, false));
        }
    }
    // Native 0.159.3 also considers a config.toml in the working directory.
    let cwd_config = session.cwd.join("config.toml");
    if cwd_config.is_file() {
        settings
            .blockers
            .push("当前目录存在 config.toml，其原生项目配置优先级尚未验证。".into());
        layers.push(("当前目录配置", cwd_config, false));
    }
    for (label, path, relocated) in &layers {
        settings
            .details
            .push((label.to_string(), display_path(path)));
        let Some(document) = parse(path)? else {
            continue;
        };
        if let Some(model) = document.get("model").and_then(Item::as_str) {
            if model.len() > 200 || model.chars().any(char::is_control) {
                settings.blockers.push("模型名称格式未验证。".into());
            } else {
                settings.model = Some(model.to_owned());
            }
        }
        if let Some(provider) = document.get("model_provider").and_then(Item::as_str)
            && provider != "openai"
        {
            settings
                .blockers
                .push("来源配置使用其他 API provider，不能标记为所选 ChatGPT 订阅额度。".into());
        }
        if document
            .get("model_providers")
            .and_then(|item| item.get("openai"))
            .is_some()
        {
            settings
                .blockers
                .push("来源重定义了 OpenAI provider；无法证明使用所选订阅额度。".into());
        }
        if let Some(mode) = document
            .get("cli_auth_credentials_store")
            .and_then(Item::as_str)
            && mode != "file"
        {
            settings
                .blockers
                .push("来源使用钥匙串、auto 或其他认证存储；首版只支持文件认证。".into());
        }
        if document
            .get("mcp_oauth_credentials_store")
            .and_then(Item::as_str)
            .is_some_and(|mode| mode != "file")
            || document.contains_key("auth_keyring_backend")
        {
            settings
                .blockers
                .push("来源使用尚未验证的 MCP 钥匙串或 OAuth 存储方式；当前禁用接力。".into());
        }
        if let Some(servers) = document.get("mcp_servers").and_then(Item::as_table_like) {
            for (name, server) in servers.iter() {
                if server.get("enabled").and_then(Item::as_bool) == Some(false) {
                    // Inactive definitions cannot launch a process or use OAuth.
                    // Native preflight must confirm that no effective layer has
                    // re-enabled a server whose fields we did not validate.
                    settings.disabled_mcp.push(name.to_owned());
                    continue;
                }
                let explicit_bearer = server
                    .get("bearer_token_env_var")
                    .and_then(Item::as_str)
                    .is_some();
                let explicit_header = server
                    .get("http_headers")
                    .and_then(Item::as_table_like)
                    .is_some_and(|headers| {
                        headers
                            .iter()
                            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    });
                if server.get("url").is_some() && !explicit_bearer && !explicit_header {
                    settings.blockers.push("HTTP MCP 的原环境 OAuth 身份无法确认；当前仅支持显式令牌或请求头认证的 HTTP MCP。".into());
                }
            }
        }
        if let Some(method) = document.get("forced_login_method").and_then(Item::as_str)
            && method != "chatgpt"
        {
            settings
                .blockers
                .push("来源强制使用非 ChatGPT 登录方式。".into());
        }
        if let Some(account) = document
            .get("forced_chatgpt_workspace_id")
            .or_else(|| document.get("forced_chatgpt_account_id"))
            .and_then(Item::as_str)
        {
            settings.forced_account = Some(account.to_owned());
        }
        for key in [
            "profile",
            "profiles",
            "project_root_markers",
            "project_doc_fallback_filenames",
            "include",
            "imports",
            "config_profile",
            "experimental_instructions_file",
            "openai_base_url",
            "chatgpt_base_url",
            "chatgpt_auth_tokens_refresh_url",
        ] {
            if document.contains_key(key) {
                settings
                    .blockers
                    .push(format!("配置项 {} 的接力语义尚未验证。", safe_text(key)));
            }
        }
        if let Some(sqlite) = document.get("sqlite_home").and_then(Item::as_str) {
            if *relocated && !Path::new(sqlite).is_absolute() {
                settings
                    .blockers
                    .push("来源 sqlite_home 使用相对路径，首版不改变其语义。".into());
            }
            let sqlite = PathBuf::from(sqlite);
            settings.database_home = if sqlite.is_absolute() {
                sqlite
            } else {
                path.parent().unwrap().join(sqlite)
            };
        }
        // Values that depend on CODEX_HOME cannot safely refer to the temporary
        // directory. Keep source config bytes untouched and fail closed instead.
        if *relocated {
            check_paths(document.as_table(), "", &mut settings.blockers);
        }
        if document
            .get("features")
            .and_then(|item| item.get("cloud_config"))
            .and_then(Item::as_bool)
            == Some(true)
        {
            settings
                .blockers
                .push("来源启用账号云端配置，无法声称其仍来自原账号。".into());
        }
    }
    // Requiring an explicit current model allows a CLI override to prevent
    // native resume from silently restoring a historical, different model.
    if settings.model.as_deref().is_none_or(str::is_empty) {
        settings
            .blockers
            .push("来源未明确设置模型；无法准确展示本次最终模型。".into());
    }
    let global = instruction_file(&source.home);
    settings.details.push((
        "全局个人指令".into(),
        global
            .as_deref()
            .map(display_path)
            .unwrap_or_else(|| "无本地 AGENTS.md / AGENTS.override.md".into()),
    ));
    for directory in project_directories(&session.cwd) {
        if let Some(path) = instruction_file(&directory) {
            settings.details.push((
                "项目指令（项目根目录 → 当前目录）".into(),
                display_path(&path),
            ));
        }
    }
    settings.details.push((
        "本地记忆".into(),
        display_path(&source.home.join("memories")),
    ));
    settings.details.push((
        "配置覆盖顺序".into(),
        "系统 → 来源用户 → 来源命名 Profile → 受信任项目 → 接力运行参数；本次明确固定显示的模型。"
            .into(),
    ));
    settings.details.push((
        if settings.blockers.is_empty() {
            "本次模型（原生启动前再验证）"
        } else {
            "候选模型（组合已禁用）"
        }
        .into(),
        settings.model.clone().unwrap_or_else(|| "无法确认".into()),
    ));
    settings
        .details
        .push(("数据库目录".into(), display_path(&settings.database_home)));
    settings.details.push((
        "本地个性化范围".into(),
        "本次磁盘上的指令、记忆和配置；不恢复已删除的历史配置，也不推断所选账号的服务端个性化。"
            .into(),
    ));
    settings.blockers.sort();
    settings.blockers.dedup();
    settings.disabled_mcp.sort();
    settings.disabled_mcp.dedup();
    Ok(settings)
}

fn check_paths(table: &dyn TableLike, prefix: &str, blockers: &mut Vec<String>) {
    for (key, item) in table.iter() {
        if prefix == "mcp_servers" && item.get("enabled").and_then(Item::as_bool) == Some(false) {
            continue;
        }
        let full = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(table) = item.as_table_like() {
            check_paths(table, &full, blockers);
        }
        if let Some(tables) = item.as_array_of_tables() {
            for table in tables {
                check_paths(table, &full, blockers);
            }
        }
        if let Some(value) = item.as_str() {
            let path_key = key.ends_with("_path")
                || key.ends_with("_file")
                || key.ends_with("_dir")
                || key.ends_with("_root")
                || key == "cwd"
                || key == "path"
                || (key == "command" && (value.contains('/') || value.contains('\\')));
            let relative = value.starts_with("./")
                || value.starts_with("../")
                || value.starts_with('~')
                || value.contains("$CODEX_HOME")
                || value.contains("${CODEX_HOME}")
                || value.contains("%CODEX_HOME%");
            if (path_key && !Path::new(value).is_absolute()) || relative {
                blockers.push(format!(
                    "来源配置含依赖相对位置的路径（{}）；首版不重写该路径。",
                    safe_text(&full)
                ));
            }
        }
        if let Some(array) = item.as_array() {
            check_array(array, &full, blockers);
            for value in array.iter().filter_map(toml_edit::Value::as_str) {
                let path_values =
                    key.ends_with("_paths") || key.ends_with("_roots") || key == "writable_roots";
                if value.starts_with("./")
                    || value.starts_with("../")
                    || value.contains("CODEX_HOME")
                    || (path_values && !Path::new(value).is_absolute())
                {
                    blockers.push(format!(
                        "来源配置数组 {} 含相对路径，无法保留其语义。",
                        safe_text(&full)
                    ));
                }
            }
        }
    }
}
fn check_array(array: &toml_edit::Array, prefix: &str, blockers: &mut Vec<String>) {
    for value in array {
        match value {
            toml_edit::Value::InlineTable(table) => check_paths(table, prefix, blockers),
            toml_edit::Value::Array(array) => check_array(array, prefix, blockers),
            _ => {}
        }
    }
}

pub(super) fn parse(path: &Path) -> Result<Option<DocumentMut>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法检查配置文件。".into()),
    };
    if metadata.len() > 4 * 1024 * 1024 {
        return Err("配置文件过大，接力已禁用。".into());
    }
    // Parser diagnostics can echo tokens and arbitrary config strings.
    let text = fs::read_to_string(path).map_err(|_| "无法读取配置文件。")?;
    text.parse()
        .map(Some)
        .map_err(|_| "配置 TOML 无效；未展示可能包含密钥的原文。".into())
}

pub(super) fn auth_store(home: &Path) -> Result<String, String> {
    Ok(parse(&home.join("config.toml"))?
        .as_ref()
        .and_then(|doc| doc.get("cli_auth_credentials_store"))
        .and_then(Item::as_str)
        .unwrap_or("file")
        .to_owned())
}
fn instruction_file(directory: &Path) -> Option<PathBuf> {
    ["AGENTS.override.md", "AGENTS.md"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}
fn project_directories(cwd: &Path) -> Vec<PathBuf> {
    let root = cwd
        .ancestors()
        .find(|directory| {
            let marker = directory.join(".git");
            fs::metadata(&marker).is_ok_and(|metadata| {
                // Native 0.159.3 skips incomplete Git directories, while a
                // file marker (for example a worktree) also defines the root.
                !metadata.is_dir() || fs::metadata(marker.join("HEAD")).is_ok()
            })
        })
        .unwrap_or(cwd);
    let mut result: Vec<_> = cwd
        .ancestors()
        .take_while(|directory| directory.starts_with(root))
        .map(Path::to_owned)
        .collect();
    result.reverse();
    result
}
fn system_directory() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
            .join("OpenAI/Codex")
    } else {
        PathBuf::from("/etc/codex")
    }
}
#[cfg(target_os = "macos")]
fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn inspect(
        source: &Environment,
        session: &Session,
        profile: Option<&str>,
    ) -> Result<Settings, String> {
        inspect_with_policy(
            source,
            session,
            profile,
            &source.home.join("fixture-system-policy"),
            &[],
        )
    }
    fn fixture(contents: &str) -> (TempDir, Environment, Session) {
        let root = TempDir::new().unwrap();
        let home = root.path().join("source");
        let cwd = root.path().join("repo/nested");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(home.join("config.toml"), contents).unwrap();
        let environment = Environment {
            id: "codex".into(),
            label: "Codex".into(),
            home,
            executable: root.path().join("codex"),
            database_home: None,
        };
        let session = Session {
            id: "01999999-0000-7000-8000-000000000001".into(),
            title: String::new(),
            cwd,
            rollout_path: root.path().join("rollout.jsonl"),
            environment_id: "codex".into(),
            modified_secs: 0,
            creation_source: None,
            busy: false,
            blockers: Vec::new(),
        };
        (root, environment, session)
    }
    #[test]
    fn disabled_mcp_relative_command_and_cwd_do_not_block_resume() {
        for syntax in [
            "[mcp_servers.computer-use]\nenabled=false\ncommand='./Missing App/Contents/MacOS/server'\ncwd='.'\n",
            "mcp_servers.computer-use={enabled=false,command='./Missing App/Contents/MacOS/server',cwd='.'}\n",
        ] {
            let (_root, environment, session) = fixture(&format!("model='gpt-5'\n{syntax}"));
            let settings = inspect(&environment, &session, None).unwrap();
            assert!(settings.blockers.is_empty(), "{:?}", settings.blockers);
            assert_eq!(settings.disabled_mcp, ["computer-use"]);
        }
    }
    #[test]
    fn disabled_http_mcp_does_not_require_unused_oauth_identity() {
        let (_root, environment, session) = fixture(
            "model='gpt-5'\n[mcp_servers.remote]\nenabled=false\nurl='https://mcp.invalid'\n",
        );
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(settings.blockers.is_empty(), "{:?}", settings.blockers);
        assert_eq!(settings.disabled_mcp, ["remote"]);
    }
    #[test]
    fn active_mcp_and_project_reactivation_keep_their_compatibility_checks() {
        for enabled in ["", "enabled=true\n"] {
            let (_root, environment, session) = fixture(&format!(
                "model='gpt-5'\n[mcp_servers.computer-use]\n{enabled}command='./missing-server'\ncwd='.'\n"
            ));
            let settings = inspect(&environment, &session, None).unwrap();
            assert!(
                settings
                    .blockers
                    .iter()
                    .any(|reason| reason.contains("mcp_servers.computer-use.command"))
            );
            assert!(
                settings
                    .blockers
                    .iter()
                    .any(|reason| reason.contains("mcp_servers.computer-use.cwd"))
            );
        }
        let (_root, environment, session) = fixture(
            "model='gpt-5'\n[mcp_servers.computer-use]\nenabled=false\ncommand='./missing-server'\ncwd='.'\n",
        );
        fs::create_dir(session.cwd.join(".git")).unwrap();
        fs::write(session.cwd.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir(session.cwd.join(".codex")).unwrap();
        fs::write(
            session.cwd.join(".codex/config.toml"),
            "[mcp_servers.computer-use]\nenabled=true\n",
        )
        .unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(
            settings
                .blockers
                .iter()
                .any(|reason| reason.contains("项目配置"))
        );
    }
    #[test]
    fn project_discovery_does_not_adopt_another_account_above_the_git_root() {
        let (root, environment, session) = fixture("model='source-model'\n");
        fs::create_dir(root.path().join("repo/.git")).unwrap();
        fs::write(root.path().join("repo/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir(root.path().join(".codex")).unwrap();
        fs::write(
            root.path().join(".codex/config.toml"),
            "model='other-account-model'\nmodel_provider='other-account-provider'\n",
        )
        .unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(settings.blockers.is_empty(), "{:?}", settings.blockers);
        assert_eq!(settings.model.as_deref(), Some("source-model"));
        assert!(
            !settings
                .details
                .iter()
                .any(|(_, path)| path == &display_path(&root.path().join(".codex/config.toml")))
        );
    }
    #[test]
    fn empty_nested_git_directory_does_not_hide_a_valid_parent_project() {
        let (root, environment, session) = fixture("model='source-model'\n");
        let project = root.path().join("repo");
        fs::create_dir(project.join(".git")).unwrap();
        fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir(session.cwd.join(".git")).unwrap();
        fs::create_dir(project.join(".codex")).unwrap();
        fs::write(
            project.join(".codex/config.toml"),
            "model='project-model'\n",
        )
        .unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(
            settings
                .blockers
                .iter()
                .any(|reason| reason.contains("项目配置")),
            "{:?}",
            settings.blockers
        );
        assert_eq!(settings.model.as_deref(), Some("project-model"));
        // File markers define a worktree boundary without a local HEAD.
        fs::remove_dir(session.cwd.join(".git")).unwrap();
        fs::write(
            session.cwd.join(".git"),
            "gitdir: ../.git/worktrees/nested\n",
        )
        .unwrap();
        let worktree = inspect(&environment, &session, None).unwrap();
        assert!(worktree.blockers.is_empty(), "{:?}", worktree.blockers);
        assert_eq!(worktree.model.as_deref(), Some("source-model"));
    }
    #[test]
    fn without_a_git_root_only_the_working_directory_is_a_project_layer() {
        let (root, environment, session) = fixture("model='source-model'\n");
        fs::create_dir(root.path().join("repo/.codex")).unwrap();
        fs::write(
            root.path().join("repo/.codex/config.toml"),
            "model='parent-model'\n",
        )
        .unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(settings.blockers.is_empty(), "{:?}", settings.blockers);
        assert_eq!(settings.model.as_deref(), Some("source-model"));
        fs::create_dir(session.cwd.join(".codex")).unwrap();
        fs::write(
            session.cwd.join(".codex/config.toml"),
            "model='project-model'\n",
        )
        .unwrap();
        assert!(
            inspect(&environment, &session, None)
                .unwrap()
                .blockers
                .iter()
                .any(|reason| reason.contains("项目配置"))
        );
    }
    #[test]
    fn rejects_relative_paths_in_nested_arrays_without_exposing_values() {
        let (_root, environment, session) =
            fixture("model='gpt-5'\n[[skills.config]]\npath='../private-secret'\n");
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(
            settings
                .blockers
                .iter()
                .any(|reason| reason.contains("skills.config.path"))
        );
        assert!(!format!("{:?}", settings.blockers).contains("private-secret"));
        let (_root, environment, session) =
            fixture("model='gpt-5'\nskills.config=[{path='relative-skill'}]\n");
        assert!(
            !inspect(&environment, &session, None)
                .unwrap()
                .blockers
                .is_empty()
        );
    }
    #[test]
    fn malformed_configs_never_echo_credentials() {
        let (_root, environment, session) = fixture("api_key = 'sk-secret-never-print\n");
        let error = inspect(&environment, &session, None).err().unwrap();
        assert!(!error.contains("sk-secret-never-print"));
    }
    #[test]
    fn openai_named_provider_cannot_route_subscription_auth_to_a_custom_endpoint() {
        for key in [
            "openai_base_url",
            "chatgpt_base_url",
            "chatgpt_auth_tokens_refresh_url",
        ] {
            let (_root, environment, session) = fixture(&format!(
                "model='gpt-5'\nmodel_provider='openai'\n{key}='https://private-endpoint.invalid/token-secret'\n"
            ));
            let settings = inspect(&environment, &session, None).unwrap();
            assert!(settings.blockers.iter().any(|reason| reason.contains(key)));
            assert!(
                !format!("{:?}{:?}", settings.blockers, settings.details).contains("token-secret")
            );
        }
    }
    #[test]
    fn source_oauth_stores_that_cannot_refresh_through_links_are_disabled() {
        let (_root, environment, session) =
            fixture("model='gpt-5'\n[mcp_servers.remote]\nurl='https://mcp.invalid'\n");
        assert!(
            inspect(&environment, &session, None)
                .unwrap()
                .blockers
                .iter()
                .any(|reason| reason.contains("HTTP MCP"))
        );
        fs::write(
            environment.home.join(".credentials.json"),
            "private OAuth credentials",
        )
        .unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        assert!(
            settings
                .blockers
                .iter()
                .any(|reason| reason.contains("刷新写入"))
        );
        assert!(!format!("{:?}", settings.blockers).contains("private OAuth credentials"));
    }
    #[test]
    fn project_overrides_profiles_and_other_providers_show_explicit_blockers() {
        let (_root, environment, session) = fixture("model='gpt-5'\nmodel_provider='custom'\n");
        fs::write(environment.home.join("work.config.toml"), "model='gpt-4'\n").unwrap();
        fs::create_dir_all(session.cwd.join(".codex")).unwrap();
        fs::write(session.cwd.join(".codex/config.toml"), "model='gpt-3'\n").unwrap();
        let settings = inspect(&environment, &session, Some("work")).unwrap();
        for term in ["Profile", "provider", "项目配置"] {
            assert!(
                settings.blockers.iter().any(|reason| reason.contains(term)),
                "missing {term}"
            );
        }
        assert!(
            settings
                .details
                .iter()
                .any(|(label, _)| label.contains("候选模型"))
        );
    }
    #[test]
    fn project_instructions_stop_at_git_root_and_prefer_override() {
        let (root, environment, session) = fixture("model='gpt-5'\n");
        fs::create_dir(root.path().join("repo/.git")).unwrap();
        fs::write(root.path().join("repo/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.path().join("AGENTS.md"), "OUTSIDE REPO").unwrap();
        fs::write(root.path().join("repo/AGENTS.md"), "ROOT").unwrap();
        fs::write(session.cwd.join("AGENTS.md"), "REGULAR").unwrap();
        fs::write(session.cwd.join("AGENTS.override.md"), "OVERRIDE").unwrap();
        let settings = inspect(&environment, &session, None).unwrap();
        let paths = settings
            .details
            .iter()
            .filter(|(label, _)| label.starts_with("项目指令"))
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                display_path(&root.path().join("repo").join("AGENTS.md")),
                display_path(&session.cwd.join("AGENTS.override.md"))
            ]
        );
    }
}
