//! Read-only ownership checks. Credentials and ownership identifiers never
//! leave this module except as the redacted, non-serializable identity type.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use agent_companion_core::usage_service::{AccountIdentity, Source};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

const LIMIT: u64 = 1024 * 1024;
const UNKNOWN: &str =
    "Could not verify the signed-in account. Check Codex authentication settings and refresh.";
const SIGNED_OUT: &str =
    "Sign in to this Codex instance with a ChatGPT subscription, then refresh.";
const LOCKED: &str = "Could not read system credentials without interaction. Unlock or authorize Codex credentials, then refresh.";

#[derive(Clone)]
pub(super) struct Config {
    pub(super) store: String,
    pub(super) service: String,
}

pub(super) fn read(source: &Source) -> Result<AccountIdentity, String> {
    let config = configuration(source)?;
    read_with(source, &config, keyring)
}

/// A final file-backed check at the publication boundary closes the interval
/// between a worker's result and the UI draining its mailbox. System credential
/// lookups remain exclusively on the worker thread.
pub(super) fn local_publication_check(
    source: &Source,
    expected: &AccountIdentity,
) -> Option<Result<AccountIdentity, String>> {
    let config = match configuration(source) {
        Ok(config) => config,
        Err(error) => return Some(Err(error)),
    };
    if config.store == "file" {
        return Some(read_with(source, &config, |_| unreachable!()));
    }
    if !expected.storage.starts_with(&format!("{}:", config.store))
        || expected.service != config.service
    {
        return Some(Err(
            "Authentication settings changed. Waiting to verify the active account.".into(),
        ));
    }
    None
}

pub(super) fn configuration_matches(source: &Source, native: &Value) -> bool {
    let Ok(expected) = configuration(source) else {
        return false;
    };
    native["cli_auth_credentials_store"].as_str() == Some(expected.store.as_str())
        && native["chatgpt_base_url"]
            .as_str()
            .map(|v| v.trim_end_matches('/'))
            == Some(expected.service.as_str())
        && native["sqlite_home"].as_str() == Some(source.database_path.to_string_lossy().as_ref())
        && native["features"]["secret_auth_storage"].as_bool() != Some(true)
}

fn read_with(
    source: &Source,
    config: &Config,
    read_keyring: impl FnOnce(&str) -> Result<Option<Vec<u8>>, String>,
) -> Result<AccountIdentity, String> {
    let home = source
        .codex_home
        .canonicalize()
        .unwrap_or_else(|_| source.codex_home.clone());
    let (bytes, location) = match config.store.as_str() {
        "file" => (read_optional(&home.join("auth.json"))?, "file"),
        "keyring" | "auto" => {
            let digest = format!("{:x}", Sha256::digest(home.to_string_lossy().as_bytes()));
            let key = format!("cli|{}", &digest[..16]);
            match read_keyring(&key) {
                Ok(Some(bytes)) => (Some(bytes), "keyring"),
                Ok(None) if config.store == "auto" => (read_optional(&home.join("auth.json"))?, "file"),
                Ok(None) => (None, "keyring"),
                // A blocked noninteractive lookup does not prove which account
                // native interactive keyring access would return. Fail closed.
                Err(_) => return Err(LOCKED.into()),
            }
        }
        "ephemeral" => return Err("This instance uses process-only authentication. Its account cannot be verified by the usage reader.".into()),
        _ => return Err(UNKNOWN.into()),
    };
    let bytes = bytes.ok_or_else(|| SIGNED_OUT.to_owned())?;
    let storage_path = if location == "file" {
        home.join("auth.json")
            .canonicalize()
            .unwrap_or_else(|_| home.join("auth.json"))
    } else {
        home
    };
    let storage = format!(
        "{}:{location}:{}",
        config.store,
        storage_path.to_string_lossy()
    );
    parse(&bytes, storage, config.service.clone())
}

fn parse(bytes: &[u8], storage: String, service: String) -> Result<AccountIdentity, String> {
    if bytes.len() > LIMIT as usize {
        return Err(UNKNOWN.into());
    }
    let auth: Value = serde_json::from_slice(bytes).map_err(|_| UNKNOWN.to_owned())?;
    if auth.get("OPENAI_API_KEY").is_some_and(|v| !v.is_null())
        || auth["auth_mode"].as_str().is_some_and(|v| v != "chatgpt")
    {
        return Err("Subscription usage requires a ChatGPT login; this instance uses a different authentication method.".into());
    }
    let tokens = &auth["tokens"];
    let workspace = identifier(&tokens["account_id"])?;
    let jwt = tokens["id_token"].as_str().ok_or(UNKNOWN)?;
    if tokens["access_token"].as_str().is_none_or(str::is_empty) {
        return Err(UNKNOWN.into());
    }
    let claims = jwt_claims(jwt)?;
    let account = &claims["https://api.openai.com/auth"];
    if account["chatgpt_account_id"]
        .as_str()
        .is_some_and(|v| v != workspace)
    {
        return Err(UNKNOWN.into());
    }
    let user = if account["chatgpt_user_id"].is_string() {
        identifier(&account["chatgpt_user_id"])?
    } else {
        identifier(&claims["sub"])?
    };
    Ok(AccountIdentity {
        user: user.into(),
        workspace: workspace.into(),
        storage,
        service,
    })
}

fn identifier(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 512 && !v.chars().any(char::is_control))
        .ok_or_else(|| UNKNOWN.into())
}

fn jwt_claims(jwt: &str) -> Result<Value, String> {
    let payload = jwt.split('.').nth(1).ok_or(UNKNOWN)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| UNKNOWN.to_owned())?;
    serde_json::from_slice(&bytes).map_err(|_| UNKNOWN.into())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNKNOWN.into()),
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() <= LIMIT)
    {
        return Err(UNKNOWN.into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNKNOWN.to_owned())?;
    if bytes.len() > LIMIT as usize {
        return Err(UNKNOWN.into());
    }
    Ok(Some(bytes))
}

pub(super) fn configuration(source: &Source) -> Result<Config, String> {
    #[cfg(windows)]
    let system = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("OpenAI/Codex");
    #[cfg(not(windows))]
    let system = PathBuf::from("/etc/codex");
    #[allow(unused_mut, clippy::useless_vec)] // macOS adds managed preference files below.
    let mut managed = vec![
        system.join("managed_config.toml"),
        system.join("requirements.toml"),
    ];
    #[cfg(target_os = "macos")]
    {
        managed.push("/Library/Managed Preferences/com.openai.codex.plist".into());
        if let Some(home) = super::user_home() {
            managed.push(home.join("Library/Managed Preferences/com.openai.codex.plist"));
        }
    }
    if managed.iter().any(|path| path.exists()) {
        return Err("Managed Codex authentication settings could not be verified. Usage is unavailable until their effective configuration is supported.".into());
    }
    let mut config = Config {
        store: "file".into(),
        service: "https://chatgpt.com/backend-api".into(),
    };
    let mut effective = DocumentMut::new();
    for path in [
        system.join("config.toml"),
        source.codex_home.join("config.toml"),
    ] {
        if let Some(doc) = document(&path)? {
            merge_tables(effective.as_table_mut(), doc.as_table());
        }
    }
    apply_config(&effective, &mut config)?;
    if source.instance_id != "codex" {
        // This is the same explicit override passed to the native worker.
        config.store = "file".into();
    }
    Ok(config)
}

fn document(path: &Path) -> Result<Option<DocumentMut>, String> {
    read_optional(path)?
        .map(|bytes| {
            let text = String::from_utf8(bytes).map_err(|_| UNKNOWN.to_owned())?;
            text.parse::<DocumentMut>().map_err(|_| UNKNOWN.into())
        })
        .transpose()
}

fn apply_config(doc: &DocumentMut, config: &mut Config) -> Result<(), String> {
    if doc.contains_key("auth_keyring_backend") {
        return Err("The configured Codex profile or encrypted authentication backend cannot be verified by this usage reader.".into());
    }
    for (key, target) in [
        ("cli_auth_credentials_store", &mut config.store),
        ("chatgpt_base_url", &mut config.service),
    ] {
        if let Some(item) = doc.get(key) {
            *target = item
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or(UNKNOWN)?
                .trim_end_matches('/')
                .to_owned();
        }
    }
    // Native 0.159.3 rejects the old root selector instead of applying
    // [profiles.name]. Unselected legacy tables may remain in a valid config.
    if doc.contains_key("profile") {
        return Err("Legacy profile selection is not supported by current Codex. Use a separate native named profile; its account is not inherited by this usage reader.".into());
    }
    let encrypted = doc
        .get("features")
        .and_then(|v| v.get("secret_auth_storage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if encrypted {
        return Err(
            "The encrypted Codex authentication backend cannot be verified by this usage reader."
                .into(),
        );
    }
    Ok(())
}

fn merge_tables(target: &mut dyn toml_edit::TableLike, source: &dyn toml_edit::TableLike) {
    for (key, value) in source.iter() {
        if let Some(existing) = target.get_mut(key).and_then(|v| v.as_table_like_mut())
            && let Some(incoming) = value.as_table_like()
        {
            merge_tables(existing, incoming);
        } else {
            target.insert(key, value.clone());
        }
    }
}

/// Direct Security.framework query with UI explicitly forbidden. Never call
/// `security find-generic-password`: it may show an authorization prompt.
#[cfg(target_os = "macos")]
fn keyring(account: &str) -> Result<Option<Vec<u8>>, String> {
    use core_foundation::{
        base::{CFType, CFTypeRef, TCFType},
        boolean::CFBoolean,
        data::CFData,
        dictionary::{CFDictionary, CFDictionaryRef},
        string::{CFString, CFStringRef},
    };
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecClass: CFStringRef;
        static kSecClassGenericPassword: CFStringRef;
        static kSecAttrService: CFStringRef;
        static kSecAttrAccount: CFStringRef;
        static kSecReturnData: CFStringRef;
        static kSecMatchLimit: CFStringRef;
        static kSecMatchLimitOne: CFStringRef;
        static kSecUseAuthenticationUI: CFStringRef;
        static kSecUseAuthenticationUIFail: CFStringRef;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    }
    unsafe {
        let s = |reference| CFString::wrap_under_get_rule(reference).as_CFType();
        let pairs = [
            (s(kSecClass), s(kSecClassGenericPassword)),
            (s(kSecAttrService), CFString::new("Codex Auth").as_CFType()),
            (s(kSecAttrAccount), CFString::new(account).as_CFType()),
            (s(kSecReturnData), CFBoolean::true_value().as_CFType()),
            (s(kSecMatchLimit), s(kSecMatchLimitOne)),
            (s(kSecUseAuthenticationUI), s(kSecUseAuthenticationUIFail)),
        ];
        let query = CFDictionary::from_CFType_pairs(&pairs);
        let mut result = std::ptr::null();
        match SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) {
            -25300 => Ok(None), // errSecItemNotFound
            0 if !result.is_null() => {
                let value = CFType::wrap_under_create_rule(result);
                value
                    .downcast::<CFData>()
                    .filter(|data| data.len() <= LIMIT as isize)
                    .map(|data| Some(data.bytes().to_vec()))
                    .ok_or_else(|| UNKNOWN.into())
            }
            _ => Err(LOCKED.into()),
        }
    }
}

#[cfg(windows)]
fn keyring(account: &str) -> Result<Option<Vec<u8>>, String> {
    use windows::{
        Win32::{
            Foundation::ERROR_NOT_FOUND,
            Security::Credentials::{CRED_TYPE_GENERIC, CredFree, CredReadW},
        },
        core::PCWSTR,
    };
    let name: Vec<u16> = format!("{account}.Codex Auth")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut credential = std::ptr::null_mut();
    unsafe {
        if let Err(error) = CredReadW(
            PCWSTR(name.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut credential,
        ) {
            return if error.code() == windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0) {
                Ok(None)
            } else {
                Err(LOCKED.into())
            };
        }
        let data = &*credential;
        let result = if data.CredentialBlobSize != 0
            && !data.CredentialBlob.is_null()
            && data.CredentialBlobSize as u64 <= LIMIT
            && data.CredentialBlobSize % 2 == 0
        {
            let bytes =
                std::slice::from_raw_parts(data.CredentialBlob, data.CredentialBlobSize as usize);
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            String::from_utf16(&units)
                .map(|s| Some(s.into_bytes()))
                .map_err(|_| UNKNOWN.to_owned())
        } else {
            Err(UNKNOWN.into())
        };
        CredFree(credential.cast());
        result
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn keyring(_account: &str) -> Result<Option<Vec<u8>>, String> {
    Err(LOCKED.into())
}

pub(super) fn response_matches(value: &Value, identity: &AccountIdentity) -> bool {
    let own = [
        "workspaceId",
        "workspace_id",
        "accountId",
        "account_id",
        "chatgptAccountId",
        "chatgpt_account_id",
    ]
    .iter()
    .all(|key| {
        value
            .get(key)
            .is_none_or(|v| v.is_null() || v.as_str() == Some(identity.workspace.as_str()))
    }) && ["userId", "user_id", "chatgptUserId", "chatgpt_user_id"]
        .iter()
        .all(|key| {
            value
                .get(key)
                .is_none_or(|v| v.is_null() || v.as_str() == Some(identity.user.as_str()))
        });
    own && match value {
        Value::Object(fields) => fields.values().all(|v| response_matches(v, identity)),
        Value::Array(values) => values.iter().all(|v| response_matches(v, identity)),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn auth(user: &str, workspace: &str) -> Vec<u8> {
        let claims = json!({"sub":"auth0|subject", "https://api.openai.com/auth": {"chatgpt_user_id":user,"chatgpt_account_id":workspace}});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        json!({"tokens":{"account_id":workspace,"id_token":format!("header.{payload}.signature"),"access_token":"synthetic-access"}}).to_string().into_bytes()
    }

    fn source(home: &Path) -> Source {
        Source {
            instance_id: "codex".into(),
            codex_home: home.into(),
            database_path: home.join("db"),
            executable_path: None,
        }
    }

    #[test]
    fn keyring_auto_order_missing_fallback_and_locked_store_fail_closed() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("auth.json"),
            auth("file-user", "shared-workspace"),
        )
        .unwrap();
        let source = source(home.path());
        let config = Config {
            store: "auto".into(),
            service: "https://fixture.invalid".into(),
        };
        let from_keyring = read_with(&source, &config, |key| {
            assert!(key.starts_with("cli|") && key.len() == 20);
            Ok(Some(auth("keyring-user", "shared-workspace")))
        })
        .unwrap();
        assert_eq!(from_keyring.user, "keyring-user");
        assert!(from_keyring.storage.starts_with("auto:keyring:"));
        let from_file = read_with(&source, &config, |_| Ok(None)).unwrap();
        assert_eq!(from_file.user, "file-user");
        assert_ne!(from_file, from_keyring);
        assert_eq!(
            read_with(&source, &config, |_| Err("private-system-error".into())).unwrap_err(),
            LOCKED
        );
        let explicit = Config {
            store: "keyring".into(),
            ..config
        };
        assert_eq!(
            read_with(&source, &explicit, |_| Ok(None)).unwrap_err(),
            SIGNED_OUT
        );
    }

    #[test]
    fn same_workspace_users_token_rotation_and_nested_response_ownership() {
        let a = parse(
            &auth("user-a", "workspace"),
            "file:fixture".into(),
            "service".into(),
        )
        .unwrap();
        let b = parse(
            &auth("user-b", "workspace"),
            "file:fixture".into(),
            "service".into(),
        )
        .unwrap();
        assert_ne!(a, b);
        let mut rotated: Value = serde_json::from_slice(&auth("user-a", "workspace")).unwrap();
        rotated["tokens"]["access_token"] = json!("rotated-access");
        rotated["tokens"]["refresh_token"] = json!("rotated-refresh");
        assert_eq!(
            parse(
                rotated.to_string().as_bytes(),
                "file:fixture".into(),
                "service".into()
            )
            .unwrap(),
            a
        );
        assert!(response_matches(
            &json!({"accountId":"workspace", "rateLimits":{"workspace_id":"workspace","chatgptUserId":"user-a"}}),
            &a
        ));
        assert!(!response_matches(
            &json!({"rateLimitsByLimitId":{"codex":{"workspaceId":"wrong"}}}),
            &a
        ));
        assert!(!response_matches(
            &json!({"dailyUsageBuckets":[{"accountId":"wrong"}]}),
            &a
        ));
        assert!(!format!("{a:?}").contains("user-a"));
    }

    #[test]
    fn invalid_or_unverifiable_login_is_explicit_and_never_echoes_credentials() {
        for bytes in [
            b"secret-invalid-json".to_vec(),
            json!({"OPENAI_API_KEY":"secret-key"})
                .to_string()
                .into_bytes(),
            json!({"tokens":{"id_token":"secret-token","account_id":"workspace"}})
                .to_string()
                .into_bytes(),
        ] {
            let error = parse(&bytes, "file:fixture".into(), "service".into()).unwrap_err();
            assert!(!error.contains("secret"));
        }
        for config in [
            "profile='work'",
            "features.secret_auth_storage=true",
            "auth_keyring_backend='secrets'",
            "chatgpt_base_url=123",
        ] {
            let mut resolved = Config {
                store: "file".into(),
                service: "service".into(),
            };
            assert!(apply_config(&config.parse().unwrap(), &mut resolved).is_err());
        }
    }

    #[test]
    fn unselected_legacy_profiles_do_not_override_actual_global_authentication() {
        let mut effective: DocumentMut = "cli_auth_credentials_store='auto'\nchatgpt_base_url='https://root.invalid'\n[profiles.work]\nchatgpt_base_url='https://inactive-profile.invalid/'\nmodel='fixture-model'\n".parse().unwrap();
        let user: DocumentMut =
            "cli_auth_credentials_store='file'\n[profiles.work]\nmodel_reasoning_effort='high'\n"
                .parse()
                .unwrap();
        merge_tables(effective.as_table_mut(), user.as_table());
        let mut config = Config {
            store: "file".into(),
            service: "default".into(),
        };
        apply_config(&effective, &mut config).unwrap();
        assert_eq!(config.store, "file");
        assert_eq!(config.service, "https://root.invalid");
        effective["profile"] = toml_edit::value("work");
        assert!(
            apply_config(&effective, &mut config)
                .unwrap_err()
                .contains("Legacy profile")
        );
    }
}
