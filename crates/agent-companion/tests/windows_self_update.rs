//! Isolated native updater acceptance. The real app runs only in headless or
//! hidden-helper mode, on a throwaway pipe/config/install. No desktop is opened.
#![cfg(windows)]

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    mem::size_of,
    os::windows::{io::FromRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use windows::{
    Win32::{
        Foundation::{FILETIME, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                CREATE_NO_WINDOW, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
                QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
            },
        },
    },
    core::PWSTR,
};

const FILES: &[&str] = &[
    "agent-companion.exe",
    "acomp.exe",
    "dodex.exe",
    "agent-companion-hook.exe",
    "agent-companion-codex.exe",
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "docs/RESUME.md",
    "docs/terminal-dependency-licenses.md",
    "docs/macos-dual-instance.md",
    "docs/panel.png",
    "docs/windows-usage.png",
    "docs/codex-tui.png",
    "docs/windows-dual.png",
    "docs/macos-menu-bar.png",
    "docs/macos-panel.png",
    "docs/macos-panel-light.png",
    "docs/macos-settings.png",
];

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FixtureDescendants {
    executable: PathBuf,
    helper_pid: u32,
}
impl Drop for FixtureDescendants {
    fn drop(&mut self) {
        stop_fixture_descendants(&self.executable, self.helper_pid);
    }
}

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_agent-companion"))
}

fn hash(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    format!("{:x}", digest.finalize())
}

fn bounded_wait(child: &mut Child, duration: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + duration;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "isolated child did not exit: {}",
            child.id()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn hidden_helper_rejects_missing_extra_and_forged_plan_without_starting_the_app() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("isolated-config");
    let forged = temporary.path().join("plan.json");
    fs::write(
        &forged,
        br#"{"schema":1,"executable":"C:\\Windows\\System32\\cmd.exe","unknown":true}"#,
    )
    .unwrap();
    let cases = [
        vec!["--internal-windows-update-helper".into()],
        vec![
            "--internal-windows-update-helper".into(),
            forged.as_os_str().to_owned(),
        ],
        vec![
            "--internal-windows-update-helper".into(),
            forged.as_os_str().to_owned(),
            "--unexpected".into(),
        ],
    ];
    for args in cases {
        let mut child = OwnedChild(
            Command::new(binary())
                .args(args)
                .env("AGENT_COMPANION_CONFIG_DIR", &config)
                .env(
                    "AGENT_COMPANION_PIPE_NAME",
                    format!("agent-companion-hidden-helper-test-{}", std::process::id()),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW.0)
                .spawn()
                .unwrap(),
        );
        assert!(!bounded_wait(&mut child.0, Duration::from_secs(5)).success());
        let mut output = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(
            output.starts_with("ERROR "),
            "helper bypassed its private dispatcher: {output}"
        );
        assert!(!config.exists());
    }
}

fn compile_probe(directory: &Path) -> PathBuf {
    let path = directory.join("updated-probe.exe");
    let output = Command::new("rustc")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/windows_update_probe.rs"))
        .args([
            "--edition=2024",
            "--crate-name",
            "windows_update_probe",
            "-C",
            "opt-level=1",
            "-C",
            "debuginfo=0",
            "-o",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "probe build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

fn process_created(process: HANDLE) -> u64 {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }.unwrap();
    ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64
}

fn tiny_image() -> Vec<u8> {
    let mut bytes = vec![0; 128];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes
}

fn wait_file_contains(path: &Path, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if fs::read_to_string(path)
            .unwrap_or_default()
            .contains(expected)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} did not contain {expected}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn stop_fixture_descendants(executable: &Path, helper_pid: u32) {
    // The fallback process was created by our helper, after our original
    // headless process exited. Only its verified isolated image may be stopped.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return;
    };
    let _snapshot = unsafe { File::from_raw_handle(snapshot.0) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut next = unsafe { Process32FirstW(snapshot, &mut entry) };
    while next.is_ok() {
        if entry.th32ParentProcessID == helper_pid
            && let Ok(process) = unsafe {
                OpenProcess(
                    PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ProcessID,
                )
            }
        {
            let _process = unsafe { File::from_raw_handle(process.0) };
            let mut image = vec![0u16; 32768];
            let mut length = image.len() as u32;
            if unsafe {
                QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(image.as_mut_ptr()),
                    &mut length,
                )
            }
            .is_ok()
            {
                let image = String::from_utf16_lossy(&image[..length as usize]);
                let normalize = |value: &str| value.trim_start_matches(r"\\?\").to_lowercase();
                if normalize(&image) == normalize(&executable.to_string_lossy()) {
                    let _ = unsafe { TerminateProcess(process, 0) };
                    let _ = unsafe { WaitForSingleObject(process, 2000) };
                }
            }
        }
        next = unsafe { Process32NextW(snapshot, &mut entry) };
    }
}

#[test]
#[ignore = "explicit native acceptance: build/run with AGENT_COMPANION_RELEASE_BUILD=1; uses only temporary headless processes"]
fn real_helper_replaces_restarts_and_rolls_back_an_isolated_package() {
    let version = Command::new(binary())
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&version.stdout).contains("100.0"),
        "requires AGENT_COMPANION_RELEASE_BUILD=1"
    );
    let temporary = tempfile::tempdir().unwrap();
    let updated = compile_probe(temporary.path());
    for behavior in ["ack", "startup-fail"] {
        native_case(temporary.path(), &updated, behavior);
    }
}

fn native_case(root: &Path, updated: &Path, behavior: &str) {
    let directory = root.join(format!("install with spaces 中文 {behavior}"));
    fs::create_dir(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    let executable = directory.join("agent-companion.exe");
    let config = directory.join("test-user-config");
    fs::create_dir(&config).unwrap();
    fs::write(config.join("keep.json"), b"unchanged user config").unwrap();
    fs::create_dir(directory.join("docs")).unwrap();
    let mut originals = Vec::new();
    for name in FILES {
        let path = directory.join(name);
        if *name == "agent-companion.exe" {
            fs::copy(binary(), &path).unwrap();
        } else if !name.starts_with("docs/") {
            fs::write(&path, format!("old package {name}")).unwrap();
        }
        originals.push((name.to_string(), path.exists().then(|| hash(&path))));
    }
    fs::write(directory.join("unrelated.txt"), b"preserve me").unwrap();
    let stage = tempfile::Builder::new()
        .prefix(".agent-companion-update-")
        .tempdir_in(&directory)
        .unwrap();
    for name in [
        "payload",
        "payload/docs",
        "backup",
        "backup/docs",
        "discard",
        "discard/docs",
    ] {
        fs::create_dir(stage.path().join(name)).unwrap();
    }
    let helper = stage.path().join("updater.exe");
    fs::copy(binary(), &helper).unwrap();
    let nonce = "a".repeat(64);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pipe = format!(
        "agent-companion-self-update-{}-{unique}",
        std::process::id()
    );
    let log = directory.join("parent.log");
    let mut original = OwnedChild(
        Command::new(&executable)
            .arg("headless")
            .env("AGENT_COMPANION_CONFIG_DIR", &config)
            .env("AGENT_COMPANION_PIPE_NAME", &pipe)
            .current_dir(&directory)
            .stdin(Stdio::null())
            .stdout(File::create(&log).unwrap())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap(),
    );
    wait_file_contains(&log, "listening on", Duration::from_secs(10));
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            true,
            original.0.id(),
        )
    }
    .unwrap();
    let parent_created = process_created(process);
    let inherited_parent = unsafe { File::from_raw_handle(process.0) };
    let mut entries = Vec::new();
    for name in FILES {
        let payload = stage.path().join("payload").join(name);
        if *name == "agent-companion.exe" {
            fs::copy(updated, &payload).unwrap();
        } else if name.ends_with(".exe") {
            fs::write(&payload, tiny_image()).unwrap();
        } else {
            fs::write(&payload, format!("new package {name}")).unwrap();
        }
        let old = originals
            .iter()
            .find(|(entry, _)| entry == name)
            .unwrap()
            .1
            .as_ref()
            .map(|sha256| {
                json!({
                    "size": fs::metadata(directory.join(name)).unwrap().len(), "sha256": sha256,
                })
            });
        entries.push(json!({"new": {"name": name, "size": fs::metadata(&payload).unwrap().len(), "sha256": hash(&payload)}, "old": old}));
    }
    let plan = json!({
        "schema": 1, "executable": executable, "parent_pid": original.0.id(), "parent_created": parent_created,
        "helper_sha256": hash(&helper), "tag": "v999.0.0", "version": "999.0.0", "nonce": nonce,
        "args": ["headless".encode_utf16().collect::<Vec<_>>()], "working_directory": directory,
        "entries": entries, "result_path": config.join("windows-update-result.json"),
    });
    let path = stage.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let mut child = OwnedChild(
        Command::new(&helper)
            .arg("--internal-windows-update-helper")
            .arg(&path)
            .env("AGENT_COMPANION_CONFIG_DIR", &config)
            .env("AGENT_COMPANION_PIPE_NAME", &pipe)
            .env("AC_UPDATE_PROBE_CONFIG", &config)
            .env("AC_UPDATE_PROBE_PIPE", &pipe)
            .env("AC_UPDATE_PROBE_BEHAVIOR", behavior)
            .current_dir(&directory)
            .stdin(Stdio::from(inherited_parent))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap(),
    );
    let helper_pid = child.0.id();
    let _descendants = FixtureDescendants {
        executable: executable.clone(),
        helper_pid,
    };
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .take(2049)
            .read_line(&mut line)
            .map(|_| line);
        let _ = sender.send(result);
    });
    let readiness = receiver
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    reader.join().unwrap();
    assert_eq!(readiness, format!("READY {nonce}\n"));
    assert!(original.0.try_wait().unwrap().is_none());
    assert_eq!(
        hash(&executable),
        originals[0].1.clone().unwrap(),
        "package changed before original exit"
    );
    original.0.kill().unwrap(); // Only this isolated test-owned parent.
    let _ = bounded_wait(&mut original.0, Duration::from_secs(5));
    let status = bounded_wait(&mut child.0, Duration::from_secs(15));
    let journal = fs::read_to_string(stage.path().join("journal.jsonl")).unwrap();
    if behavior == "ack" {
        assert!(
            status.success(),
            "helper failed: {}",
            fs::read_to_string(stage.path().join("result.json")).unwrap_or_default()
        );
        assert!(journal.contains("\"event\":\"committed\""));
        assert_eq!(hash(&executable), hash(updated));
        assert!(stage.path().join("probe-environment-ok").exists());
        wait_file_contains(
            &stage.path().join("probe-exited-after-commit"),
            "ok",
            Duration::from_secs(5),
        );
    } else {
        stop_fixture_descendants(&executable, helper_pid);
        assert!(!status.success());
        assert!(journal.contains("\"event\":\"failed\""));
        for (name, expected) in originals {
            match expected {
                Some(expected) => assert_eq!(hash(&directory.join(name)), expected),
                None => assert!(!directory.join(name).exists()),
            }
        }
        let result: Value =
            serde_json::from_slice(&fs::read(config.join("windows-update-result.json")).unwrap())
                .unwrap();
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("previous package was restored")
        );
    }
    assert_eq!(
        fs::read(directory.join("unrelated.txt")).unwrap(),
        b"preserve me"
    );
    assert_eq!(
        fs::read(config.join("keep.json")).unwrap(),
        b"unchanged user config"
    );
}
