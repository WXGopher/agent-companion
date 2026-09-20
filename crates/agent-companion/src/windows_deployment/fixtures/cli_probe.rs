//! A dependency-free native child for Windows launcher regression tests.
use std::{io, os::windows::ffi::OsStrExt};

fn main() {
    let arguments: Vec<Vec<u16>> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.encode_wide().collect())
        .collect();
    println!("arguments={arguments:?}");
    println!(
        "directory={:?}",
        std::env::current_dir()
            .unwrap()
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>()
    );
    for name in [
        "CODEX_HOME",
        "CODEX_SQLITE_HOME",
        "OPENAI_API_KEY",
        "CODEX_THREAD_ID",
    ] {
        println!(
            "{name}={:?}",
            std::env::var_os(name).map(|value| value.encode_wide().collect::<Vec<_>>())
        );
    }
    io::copy(&mut io::stdin(), &mut io::stdout()).unwrap();
    eprintln!("CLI stderr is preserved");
    // Includes the sign bit and cannot be represented by an 8-bit ExitCode.
    std::process::exit(0xf1234567u32 as i32);
}
