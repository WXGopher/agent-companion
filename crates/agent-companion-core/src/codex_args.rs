//! Option-aware dispatch for isolated native Codex command entries. Prompts and
//! option values are never inspected as management commands or config options.
use std::ffi::OsString;

fn value_option(name: &str) -> bool {
    matches!(
        name,
        "--config"
            | "--enable"
            | "--disable"
            | "--remote"
            | "--remote-auth-token-env"
            | "--model"
            | "--local-provider"
            | "--profile"
            | "--sandbox"
            | "--cd"
            | "--add-dir"
            | "--ask-for-approval"
            | "--image"
            | "--output-last-message"
            | "--output-schema"
            | "--color"
            | "--env"
            | "--url"
            | "--bearer-token-env-var"
            | "--listen"
            | "--host"
            | "--port"
            | "--issuer-base-url"
            | "--client-id"
            | "--title"
            | "--base"
            | "--commit"
            | "--permission-profile"
    )
}
fn short_value(name: char) -> Option<&'static str> {
    Some(match name {
        'c' => "--config",
        'i' => "--image",
        'm' => "--model",
        'p' => "--profile",
        's' => "--sandbox",
        'C' => "--cd",
        'a' => "--ask-for-approval",
        'o' => "--output-last-message",
        'P' => "--permission-profile",
        _ => return None,
    })
}
fn bool_option(name: &str) -> bool {
    matches!(
        name,
        "--help"
            | "--version"
            | "--yolo"
            | "--strict-config"
            | "--oss"
            | "--approve-for-me"
            | "--dangerously-bypass-approvals-and-sandbox"
            | "--dangerously-bypass-hook-trust"
            | "--worktree"
            | "--search"
            | "--no-alt-screen"
            | "--full-auto"
            | "--no-daemon"
            | "--ignore-user-config"
            | "--ignore-rules"
    )
}

type Options = Vec<(String, Option<String>)>;
fn option(args: &[OsString], index: usize) -> Option<(usize, Options)> {
    let token = args.get(index)?.to_str()?;
    let mut end = index + 1;
    let mut options = Vec::new();
    let (name, attached) = if token.starts_with("--") {
        let (name, attached) = token
            .split_once('=')
            .map_or((token, None), |(name, value)| (name, Some(value)));
        if attached.is_none() && bool_option(name) {
            return Some((end, vec![(name.into(), None)]));
        }
        if !value_option(name) {
            return None;
        }
        (name, attached)
    } else {
        let mut found = None;
        for (offset, letter) in token.strip_prefix('-')?.char_indices() {
            if let Some(name) = short_value(letter) {
                let value = &token[1 + offset + letter.len_utf8()..];
                found = Some((
                    name,
                    (!value.is_empty()).then(|| value.strip_prefix('=').unwrap_or(value)),
                ));
                break;
            }
            options.push((
                match letter {
                    'h' => "--help",
                    'V' => "--version",
                    _ => return None,
                }
                .into(),
                None,
            ));
        }
        let Some(found) = found else {
            return Some((end, options));
        };
        found
    };
    let value = match attached {
        Some(value) => value,
        None => {
            let value = args.get(end)?.to_str()?;
            if value == "--" {
                return None;
            }
            end += 1;
            value
        }
    };
    options.push((name.into(), Some(value.into())));
    if name == "--image" {
        while let Some(value) = args
            .get(end)
            .and_then(|value| value.to_str())
            .filter(|value| !value.starts_with('-'))
        {
            options.push((name.into(), Some(value.into())));
            end += 1;
        }
    }
    Some((end, options))
}

pub fn config_overrides(args: &[OsString]) -> Vec<String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" {
            break;
        }
        if args[index]
            .to_str()
            .is_some_and(|value| value.starts_with('-'))
            && let Some((end, options)) = option(args, index)
        {
            result.extend(
                options
                    .into_iter()
                    .filter_map(|(key, value)| (key == "--config").then_some(value).flatten()),
            );
            index = end;
        } else {
            index += 1;
        }
    }
    result
}

/// Inspect only the root command. Unknown option arity and option values stay
/// with the native parser; prompts containing "app"/"update" are never routed.
fn root_command(args: &[OsString]) -> Option<&str> {
    let mut index = 0;
    while index < args.len() {
        let token = args[index].to_str()?;
        if token == "--" {
            return None;
        }
        if token.starts_with('-') && token != "-" {
            let (end, options) = option(args, index)?;
            if options
                .iter()
                .any(|(key, _)| key == "--help" || key == "--version")
            {
                return None;
            }
            index = end;
        } else {
            return if token == "help" {
                args.get(index + 1)?.to_str()
            } else {
                Some(token)
            };
        }
    }
    None
}

pub fn is_app_command(args: &[OsString]) -> bool {
    root_command(args) == Some("app")
}
pub fn is_update_command(args: &[OsString]) -> bool {
    root_command(args) == Some("update") && !args.iter().any(|arg| arg == "--help" || arg == "-h")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn only_the_real_desktop_subcommand_is_rejected_and_updates_stay_native() {
        for values in [
            &["--", "app"][..],
            &["resume", "id", "app"],
            &["--image", "picture", "app"],
            &["--future", "value", "app"],
            &["--model", "app"],
            &["--help", "app"],
        ] {
            assert!(!is_app_command(&args(values)), "{values:?}");
        }
        for values in [
            &["app"][..],
            &["help", "app"],
            &["--no-daemon", "app"],
            &["-C", "/somewhere", "app", "--help"],
        ] {
            assert!(is_app_command(&args(values)), "{values:?}");
        }
        assert!(is_update_command(&args(&["update"])));
        assert!(is_update_command(&args(&["-C", "/work", "update"])));
        assert!(!is_update_command(&args(&["update", "--help"])));
        assert!(!is_update_command(&args(&["exec", "update"])));
    }
    #[test]
    fn config_scanner_stops_at_separator_and_consumes_attached_or_separate_values() {
        assert!(
            config_overrides(&args(&["resume", "id", "--", "-c", "sqlite_home=prompt"])).is_empty()
        );
        assert!(
            config_overrides(&args(&[
                "--model",
                "-clog_dir=prompt",
                "exec",
                "--output-last-message",
                "-clog_dir=file"
            ]))
            .is_empty()
        );
        assert_eq!(
            config_overrides(&args(&[
                "resume",
                "id",
                "-hcsqlite_home='/bad'",
                "--config=profiles.work.log_dir='/bad'"
            ])),
            ["sqlite_home='/bad'", "profiles.work.log_dir='/bad'"]
        );
    }
}
