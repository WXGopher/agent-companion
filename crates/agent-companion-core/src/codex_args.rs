//! Option-aware dispatch for isolated native Codex command entries. Prompts and
//! option values are never inspected as management commands or config options.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Native,
    App(Option<PathBuf>),
    Update,
    AppHelp,
    UpdateHelp,
}

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

pub fn route(args: &[OsString], cwd: &Path) -> Result<Action, String> {
    let (mut index, mut prefix) = (0, Vec::new());
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            return Ok(Action::Native);
        };
        if token == "--" {
            return Ok(Action::Native);
        }
        if token.starts_with('-') && token != "-" {
            let Some((end, options)) = option(args, index) else {
                return Ok(Action::Native);
            };
            prefix.extend(options);
            index = end;
            continue;
        }
        if prefix
            .iter()
            .any(|(name, _)| name == "--help" || name == "--version")
        {
            return Ok(Action::Native);
        }
        return match token {
            "app" => app_request(&args[index + 1..], prefix, cwd),
            "update" if index == 0 && args.len() == 1 => Ok(Action::Update),
            "update"
                if index == 0 && args.len() == 2 && (args[1] == "--help" || args[1] == "-h") =>
            {
                Ok(Action::UpdateHelp)
            }
            "update" => {
                Err("Use dodex update without options to open Companion's update controls.".into())
            }
            _ => Ok(Action::Native),
        };
    }
    Ok(Action::Native)
}

fn app_request(args: &[OsString], mut options: Options, cwd: &Path) -> Result<Action, String> {
    let (mut index, mut separated, mut values) = (0, false, Vec::new());
    while index < args.len() {
        if !separated && args[index] == "--" {
            separated = true;
            index += 1;
            continue;
        }
        if !separated
            && args[index]
                .to_str()
                .is_some_and(|value| value.starts_with('-') && value != "-")
        {
            let (end, consumed) =
                option(args, index).ok_or("dodex app supports [PATH] and -C/--cd only.")?;
            options.extend(consumed);
            index = end;
        } else {
            values.push(&args[index]);
            index += 1;
        }
    }
    if options.iter().any(|(name, _)| name == "--help") {
        return Ok(Action::AppHelp);
    }
    if options.iter().any(|(name, _)| name != "--cd") || options.len() > 1 || values.len() > 1 {
        return Err("dodex app supports one [PATH] and at most one -C/--cd directory.".into());
    }
    if values.is_empty() && options.is_empty() {
        return Ok(Action::App(None));
    }
    let mut workspace = cwd.to_path_buf();
    if let Some((_, Some(directory))) = options.first() {
        workspace = workspace.join(directory);
    }
    if let Some(path) = values.first() {
        workspace = workspace.join(path);
    }
    if !workspace.is_dir() {
        return Err("The workspace must be an existing directory.".into());
    }
    Ok(Action::App(Some(std::path::absolute(workspace).map_err(
        |_| "Could not resolve the workspace directory.",
    )?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn prompts_option_values_and_subcommands_preserve_native_ownership() {
        for values in [
            &["--", "app"][..],
            &["resume", "id", "app"],
            &["--image", "picture", "app"],
            &["--future", "value", "update"],
            &["--model", "app"],
            &["--help", "app"],
        ] {
            assert_eq!(
                route(&args(values), Path::new(".")).unwrap(),
                Action::Native
            );
        }
        assert_eq!(
            route(&args(&["update"]), Path::new(".")).unwrap(),
            Action::Update
        );
        assert_eq!(
            route(&args(&["app"]), Path::new(".")).unwrap(),
            Action::App(None)
        );
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
    #[test]
    fn project_paths_and_cd_reach_one_public_app_route() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("project with spaces")).unwrap();
        assert_eq!(
            route(&args(&["-C", "project with spaces", "app"]), temp.path()).unwrap(),
            Action::App(Some(temp.path().join("project with spaces")))
        );
        assert!(route(&args(&["app", "--config", "model=o3"]), temp.path()).is_err());
    }
}
