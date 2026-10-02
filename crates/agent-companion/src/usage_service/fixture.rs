//! Tiny protocol peer compiled by the tests for the host platform.
use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::PathBuf,
};

fn main() {
    let home = PathBuf::from(env::var_os("CODEX_HOME").unwrap());
    let arguments: Vec<_> = env::args().collect();
    let setting = |name: &str| {
        arguments
            .iter()
            .find_map(|arg| arg.strip_prefix(&format!("{name}=")).map(str::to_owned))
            .unwrap_or("null".into())
    };
    // Keep the startup account, like a native auth manager. Changing auth.json
    // must replace this process before another account's reading is trusted.
    let second_user = fs::read_to_string(home.join("auth.json"))
        .unwrap_or_default()
        .contains("\"fixture_account\":\"b\"");
    fs::write(home.join(format!("pid-{}", std::process::id())), "").unwrap();
    fs::write(
        home.join("environment.log"),
        format!(
            "home={}\ndatabase={}\n",
            home.display(),
            env::var("CODEX_SQLITE_HOME").unwrap()
        ),
    )
    .unwrap();
    fs::write(
        home.join("arguments.log"),
        env::args().collect::<Vec<_>>().join("\n"),
    )
    .unwrap();
    let mut log = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(home.join("requests.log"))
        .unwrap();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        writeln!(log, "{line}").unwrap();
        log.flush().unwrap();
        let id = line
            .split("\"id\":")
            .nth(1)
            .and_then(|text| text.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|text| text.parse::<u64>().ok())
            .unwrap_or(0);
        let mode = fs::read_to_string(home.join("mode")).unwrap_or_default();
        let response = if line.contains("\"method\":\"initialize\"") {
            Some("{\"id\":1,\"result\":{}}".to_owned())
        } else if line.contains("\"method\":\"config/read\"") {
            let service = if mode == "wrong-config" {
                "\"https://wrong-source.invalid\"".into()
            } else {
                setting("chatgpt_base_url")
            };
            Some(format!(
                "{{\"id\":2,\"result\":{{\"config\":{{\"cli_auth_credentials_store\":{},\"chatgpt_base_url\":{service},\"sqlite_home\":{}}}}}}}",
                setting("cli_auth_credentials_store"),
                setting("sqlite_home")
            ))
        } else if line.contains("account/rateLimits/read") {
            if mode == "exit" {
                std::process::exit(2);
            }
            if mode == "hang" {
                continue;
            }
            let used = if second_user {
                73
            } else if home.file_name().unwrap() == "dodex" {
                61
            } else {
                23
            };
            Some(format!(
                "{{\"id\":2,\"result\":{{\"rateLimits\":{{\"secondary\":{{\"usedPercent\":{used},\"windowDurationMins\":10080,\"resetsAt\":200}}}}}}}}"
            ))
        } else if line.contains("account/usage/read") {
            if mode == "history-hang" {
                continue;
            }
            Some("{\"id\":2,\"result\":{\"summary\":{\"lifetimeTokens\":1000}}}".to_owned())
        } else if line.contains("\"method\":\"initialized\"") {
            None
        } else {
            panic!("Unexpected account or model method: {line}");
        };
        if let Some(response) = response {
            let response = response.replace("\"id\":2", &format!("\"id\":{id}"));
            println!("{response}");
            io::stdout().flush().unwrap();
        }
    }
}
