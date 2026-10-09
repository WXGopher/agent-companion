use std::io::{Read, Write};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if let Some(index) = args.iter().position(|arg| arg == "--send-ctrl-c") {
        unsafe extern "system" {
            fn FreeConsole() -> i32;
            fn AttachConsole(pid: u32) -> i32;
            fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
            fn GenerateConsoleCtrlEvent(kind: u32, group: u32) -> i32;
        }
        unsafe {
            FreeConsole();
            assert_ne!(AttachConsole(args[index+1].parse().unwrap()), 0);
            assert_ne!(SetConsoleCtrlHandler(None, 1), 0);
            assert_ne!(GenerateConsoleCtrlEvent(0, 0), 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        return;
    }
    if args.iter().any(|arg| arg == "--signal-wait") {
        #[cfg(windows)]
        {
            unsafe extern "system" { fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32; }
            unsafe extern "system" fn handler(kind: u32) -> i32 {
                if kind == 0 { std::process::exit(77); }
                0
            }
            assert_ne!(unsafe { SetConsoleCtrlHandler(Some(handler), 1) }, 0);
        }
        println!("ready"); std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30)); return;
    }
    println!("args={args:?}");
    println!("runtime={}", std::env::current_exe().unwrap().display());
    for name in ["CODEX_HOME", "CODEX_SQLITE_HOME", "CODEX_INSTALL_DIR", "CODEX_CLI_PATH", "CODEX_THREAD_ID", "CODEX_DAEMON_SOCKET", "CODEX_APP_SERVER_USE_LOCAL_DAEMON", "CODEX_SANDBOX", "HTTPS_PROXY", "OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"] {
        println!("{name}={}", std::env::var(name).unwrap_or_else(|_| "ABSENT".into()));
    }
    println!("cwd={}", std::env::current_dir().unwrap().display());
    let mut input = String::new(); std::io::stdin().read_to_string(&mut input).unwrap();
    println!("stdin={input}");
    if args.iter().any(|arg| arg == "--exit-73") { std::process::exit(73); }
    if args.iter().any(|arg| arg == "--exit-32") { std::process::exit(0x1234); }
}
