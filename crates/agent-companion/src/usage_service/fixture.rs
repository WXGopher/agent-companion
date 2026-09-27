//! Tiny protocol peer compiled by the tests for the host platform.
use std::{env, fs, io::{self, BufRead, Write}, path::PathBuf, time::Duration};

fn main() {
    let home = PathBuf::from(env::var_os("CODEX_HOME").unwrap());
    fs::write(home.join(format!("pid-{}", std::process::id())), "").unwrap();
    fs::write(home.join("environment.log"), format!("home={}\ndatabase={}\n", home.display(), env::var("CODEX_SQLITE_HOME").unwrap())).unwrap();
    fs::write(home.join("arguments.log"), env::args().collect::<Vec<_>>().join("\n")).unwrap();
    let mut log = fs::OpenOptions::new().append(true).create(true).open(home.join("requests.log")).unwrap();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        writeln!(log, "{line}").unwrap();
        log.flush().unwrap();
        let mode = fs::read_to_string(home.join("mode")).unwrap_or_default();
        let response = if line.contains("\"method\":\"initialize\"") {
            Some("{\"id\":1,\"result\":{}}".to_owned())
        } else if line.contains("account/rateLimits/read") {
            if mode == "exit" { std::process::exit(2); }
            if mode == "hang" { std::thread::sleep(Duration::from_secs(60)); }
            let used = if home.file_name().unwrap() == "dodex" { 61 } else { 23 };
            Some(format!("{{\"id\":2,\"result\":{{\"rateLimits\":{{\"secondary\":{{\"usedPercent\":{used},\"windowDurationMins\":10080,\"resetsAt\":200}}}}}}}}"))
        } else if line.contains("account/usage/read") {
            if mode == "history-hang" { std::thread::sleep(Duration::from_secs(60)); }
            Some("{\"id\":2,\"result\":{\"summary\":{\"lifetimeTokens\":1000}}}".to_owned())
        } else if line.contains("\"method\":\"initialized\"") { None }
        else { panic!("Unexpected account or model method: {line}"); };
        if let Some(response) = response { println!("{response}"); io::stdout().flush().unwrap(); }
    }
}
