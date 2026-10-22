//! Standalone native update acceptance probe. No GUI, account or network use.
#![windows_subsystem = "windows"]

use std::{
    fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

const VERSION: &str = "999.0.0";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => std::process::ExitCode::from(23),
    }
}

fn run() -> Result<(), ()> {
    if std::env::var("AC_UPDATE_PROBE_BEHAVIOR").as_deref() == Ok("startup-fail") {
        return Err(());
    }
    let stage = PathBuf::from(std::env::var_os("AGENT_COMPANION_UPDATE_ACK_DIR").ok_or(())?);
    let nonce = std::env::var("AGENT_COMPANION_UPDATE_ACK_NONCE").map_err(|_| ())?;
    if std::env::var_os("AGENT_COMPANION_CONFIG_DIR") != std::env::var_os("AC_UPDATE_PROBE_CONFIG")
        || std::env::var_os("AGENT_COMPANION_PIPE_NAME") != std::env::var_os("AC_UPDATE_PROBE_PIPE")
        || std::env::args_os().skip(1).collect::<Vec<_>>() != [std::ffi::OsString::from("headless")]
    {
        return Err(());
    }
    fs::write(
        stage.join("probe-environment-ok"),
        b"isolated config, pipe and restart arguments preserved\n",
    )
    .map_err(|_| ())?;
    let mut ack = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(stage.join("startup-ack.json"))
        .map_err(|_| ())?;
    write!(
        ack,
        "{{\"schema\":1,\"nonce\":\"{}\",\"pid\":{},\"version\":\"{}\"}}\n",
        nonce,
        std::process::id(),
        VERSION
    )
    .map_err(|_| ())?;
    ack.sync_all().map_err(|_| ())?;
    drop(ack);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if fs::read_to_string(stage.join("journal.jsonl"))
            .unwrap_or_default()
            .contains("\"event\":\"committed\"")
        {
            fs::write(stage.join("probe-exited-after-commit"), b"ok").map_err(|_| ())?;
            return Ok(());
        }
        if stage.join("probe-stop").exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(())
}
