//! An initialized helper owns held files and an exact parent process handle.
//! All renames are journaled before mutation and have a reverse rollback.
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use tempfile::TempDir;
use windows::Win32::{
    Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ},
    System::{
        Console::{GetStdHandle, STD_INPUT_HANDLE},
        Threading::{
            CREATE_NO_WINDOW, GetProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, WaitForSingleObject,
        },
    },
};

pub(super) use super::native::pin_directory;
use super::{
    Selection, native,
    package::{self, Entry},
};

const HELPER_ARG: &str = "--internal-windows-update-helper";
const STAGE_PREFIX: &str = ".agent-companion-update-";
const MAX_PLAN: u64 = 128 * 1024;
const MAX_RESULT: u64 = 16 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const PARENT_TIMEOUT: Duration = Duration::from_secs(45);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const RESTART_ENV: &str = "AGENT_COMPANION_UPDATE_RESTART";
const ACK_DIR_ENV: &str = "AGENT_COMPANION_UPDATE_ACK_DIR";
const ACK_NONCE_ENV: &str = "AGENT_COMPANION_UPDATE_ACK_NONCE";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fingerprint {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlannedEntry {
    new: Entry,
    old: Option<Fingerprint>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: u32,
    executable: PathBuf,
    parent_pid: u32,
    parent_created: u64,
    helper_sha256: String,
    tag: String,
    version: String,
    nonce: String,
    args: Vec<Vec<u16>>,
    working_directory: PathBuf,
    entries: Vec<PlannedEntry>,
    result_path: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartupAck {
    schema: u32,
    nonce: String,
    pid: u32,
    version: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResultMessage {
    message: String,
}

fn configured_result_path() -> Option<PathBuf> {
    let path = agent_companion_core::usage_service::settings_path()
        .ok()?
        .with_file_name("windows-update-result.json");
    Some(if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    })
}

pub(super) fn validate_install(executable: &Path) -> io::Result<PathBuf> {
    if !native::plain_absolute(executable)
        || !executable.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case("agent-companion.exe")
        })
    {
        return Err(io::Error::other(
            "Run the packaged agent-companion.exe to update this installation",
        ));
    }
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("Invalid installation directory"))?;
    let _pin = pin_directory(directory)?;
    match fs::symlink_metadata(directory.join(".agent-companion-cli.json")) {
        Ok(_) => {
            return Err(io::Error::other(
                "This is an installed CLI alias directory. Open Agent Companion from its release package to update it",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let _executable = native::open_held(executable, false)?;
    Ok(directory.to_path_buf())
}

fn create_owned_tree(stage: &Path) -> io::Result<()> {
    for name in ["backup", "backup/docs", "discard", "discard/docs"] {
        fs::create_dir(stage.join(name))?;
    }
    Ok(())
}

fn nonce(stage: &Path) -> io::Result<String> {
    let temporary = tempfile::Builder::new()
        .prefix("nonce-")
        .rand_bytes(32)
        .tempfile_in(stage)?;
    Ok(temporary
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned())
}

fn write_json_new(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn read_bounded<T: for<'de> Deserialize<'de>>(path: &Path, maximum: u64) -> io::Result<T> {
    let file = native::open_held(path, false)?;
    if file.metadata()?.len() > maximum {
        return Err(io::Error::other("Update metadata is too large"));
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(io::Error::other("Update metadata is too large"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub(super) fn prepare_helper(
    temporary: TempDir,
    executable: PathBuf,
    selected: &Selection,
    entries: Vec<Entry>,
    result_path: Option<PathBuf>,
) -> io::Result<Child> {
    let directory = validate_install(&executable)?;
    let _pins = pin_directory(&directory)?;
    let stage = temporary.path();
    create_owned_tree(stage)?;
    let helper = stage.join("updater.exe");
    let mut source = native::open_held(&executable, false)?;
    let original = native::fingerprint(&mut source)?;
    let mut copy = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&helper)?;
    std::io::Seek::rewind(&mut source)?;
    io::copy(&mut source, &mut copy)?;
    copy.sync_all()?;
    drop(copy);
    if super::hash_file(&helper)? != original.sha256 {
        return Err(io::Error::other("The copied update helper did not verify"));
    }
    let mut planned = Vec::new();
    for entry in entries {
        let path = directory.join(&entry.name);
        let old = if native::regular_or_absent(&path)? {
            Some(native::fingerprint(&mut native::open_held(&path, false)?)?)
        } else {
            None
        };
        planned.push(PlannedEntry { new: entry, old });
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            true,
            std::process::id(),
        )
    }
    .map_err(native::win_error)?;
    // A real process handle (never the GetCurrentProcess pseudo handle) is
    // passed as stdin. Rust inherits only the explicitly configured handles.
    let parent = unsafe { File::from_raw_handle(process.0) };
    let plan = Plan {
        schema: 1,
        executable,
        parent_pid: std::process::id(),
        parent_created: native::process_created(HANDLE(parent.as_raw_handle()))?,
        helper_sha256: original.sha256,
        tag: selected.tag.clone(),
        version: selected.version.clone(),
        nonce: nonce(stage)?,
        args: std::env::args_os()
            .skip(1)
            .map(|arg| arg.encode_wide().collect())
            .collect(),
        working_directory: std::env::current_dir()?,
        entries: planned,
        result_path: result_path.map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        }),
    };
    let path = stage.join("plan.json");
    write_json_new(&path, &plan)?;
    drop(source);
    // From here the helper may outlive its parent. Retain only this self-owned
    // directory, including backups and journal, for recovery on any failure.
    let stage = temporary.keep();
    let mut child = Command::new(&helper)
        .arg(HELPER_ARG)
        .arg(&path)
        .current_dir(&plan.working_directory)
        .stdin(Stdio::from(parent))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "Cannot start update helper ({error}); staging retained at {}",
                    stage.display()
                ),
            )
        })?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Update helper has no readiness channel"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("update-helper-ready".into())
        .spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output)
                .take(1025)
                .read_line(&mut line)
                .map(|_| line);
            let _ = sender.send(result);
        })?;
    let response = receiver.recv_timeout(READY_TIMEOUT);
    let expected = format!("READY {}\n", plan.nonce);
    let ready = matches!(&response, Ok(Ok(line)) if line == &expected)
        && matches!(child.try_wait(), Ok(None));
    if !ready {
        let _ = stop_owned(&mut child);
        let _ = reader.join();
        let detail = match response {
            Ok(Ok(line)) if line.starts_with("ERROR ") => {
                line.trim().trim_start_matches("ERROR ").to_owned()
            }
            _ => "The helper did not initialize within the readiness deadline".into(),
        };
        return Err(io::Error::other(format!(
            "{detail}; staging retained at {}",
            stage.display()
        )));
    }
    let _ = reader.join();
    Ok(child)
}

fn valid_plan(plan: &Plan, stage: &Path) -> io::Result<()> {
    let directory = validate_install(&plan.executable)?;
    let stage_name = stage
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if plan.schema != 1
        || stage.parent() != Some(directory.as_path())
        || !stage_name.starts_with(STAGE_PREFIX)
        || stage_name.len() > 128
        || !stage_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || plan.parent_pid == 0
        || plan.parent_pid == std::process::id()
        || plan.parent_created == 0
        || !native::plain_absolute(&plan.working_directory)
        || plan.nonce.len() < 32
        || plan.nonce.len() > 96
        || !plan
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || plan.args.len() > 128
        || plan.args.iter().map(Vec::len).sum::<usize>() > 32760
        || plan.args.iter().any(|arg| arg.contains(&0))
        || super::parse_digest(&plan.helper_sha256).as_deref() != Some(plan.helper_sha256.as_str())
        || super::stable_version(&plan.tag)
            .map(|version| version.to_string())
            .as_deref()
            != Some(plan.version.as_str())
        || plan.result_path != configured_result_path()
    {
        return Err(io::Error::other("Update plan identity is invalid"));
    }
    package::validate_manifest(
        &plan
            .entries
            .iter()
            .map(|entry| entry.new.clone())
            .collect::<Vec<_>>(),
    )?;
    if !plan
        .entries
        .iter()
        .find(|entry| entry.new.name == "agent-companion.exe")
        .and_then(|entry| entry.old.as_ref())
        .is_some_and(|old| old.sha256 == plan.helper_sha256)
    {
        return Err(io::Error::other(
            "Update plan does not own the running executable",
        ));
    }
    let target = super::stable_version(&plan.version)
        .ok_or_else(|| io::Error::other("Invalid target version"))?;
    let current = super::stable_version(env!("AGENT_COMPANION_SEMVER_VERSION"))
        .ok_or_else(|| io::Error::other("Invalid helper build version"))?;
    if cfg!(companion_local_build)
        || !cfg!(target_arch = "x86_64")
        || !target.cmp_precedence(&current).is_gt()
    {
        return Err(io::Error::other(
            "This helper cannot install the selected release",
        ));
    }
    Ok(())
}

pub(super) fn try_run_helper() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HELPER_ARG)) {
        return None;
    }
    let path = args.next().map(PathBuf::from);
    let result = if args.next().is_some() {
        Err(io::Error::other("Unexpected update helper arguments"))
    } else {
        path.ok_or_else(|| io::Error::other("Missing update plan"))
            .and_then(|path| run_helper(&path))
    };
    if let Err(error) = &result {
        // stdout is the parent's private readiness pipe, never a terminal.
        let _ = writeln!(
            io::stdout(),
            "ERROR {}",
            error.to_string().replace(['\r', '\n'], " ")
        );
    }
    Some(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run_helper(path: &Path) -> io::Result<()> {
    if !native::plain_absolute(path) || path.file_name() != Some(std::ffi::OsStr::new("plan.json"))
    {
        return Err(io::Error::other("Invalid update plan path"));
    }
    let stage = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid update stage"))?;
    let _stage_pin = pin_directory(stage)?;
    let _plan_file = native::open_held(path, false)?;
    let plan: Plan = read_bounded(path, MAX_PLAN)?;
    valid_plan(&plan, stage)?;
    let helper = std::env::current_exe()?.canonicalize()?;
    if !native::same_path(&helper, &stage.join("updater.exe"))
        || super::hash_file(&helper)? != plan.helper_sha256
    {
        return Err(io::Error::other(
            "Update helper executable identity is invalid",
        ));
    }
    let process = unsafe { GetStdHandle(STD_INPUT_HANDLE) }.map_err(native::win_error)?;
    if process.is_invalid() || process.0.is_null() {
        return Err(io::Error::other("Missing inherited parent process handle"));
    }
    unsafe { SetHandleInformation(process, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
        .map_err(native::win_error)?;
    let parent = unsafe { File::from_raw_handle(process.0) };
    let handle = HANDLE(parent.as_raw_handle());
    if unsafe { GetProcessId(handle) } != plan.parent_pid
        || native::process_created(handle)? != plan.parent_created
        || !native::same_path(&native::process_image(handle)?, &plan.executable)
        || unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT
    {
        return Err(io::Error::other(
            "Inherited handle does not identify the live original app",
        ));
    }
    let mut transaction = Transaction::initialize(&plan, stage)?;
    transaction.journal("ready", None)?;
    writeln!(io::stdout(), "READY {}", plan.nonce)?;
    io::stdout().flush()?;
    let mut owned_child = None;
    let result: io::Result<()> = (|| {
        wait_parent(handle, PARENT_TIMEOUT)?;
        transaction.apply()?;
        // The new GUI starts its own manager before final commit. Clear an
        // older attempt's feedback before spawning it, so a successful restart
        // cannot retain a stale failure snapshot from that previous attempt.
        let _ = save_result(stage, plan.result_path.as_deref(), "");
        owned_child = Some(spawn_app(&plan, stage, true)?);
        wait_startup(owned_child.as_mut().unwrap(), &plan, stage, STARTUP_TIMEOUT)?;
        transaction.journal("committed", None)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            let _ = save_result(stage, plan.result_path.as_deref(), "");
            transaction.cleanup_committed();
            Ok(())
        }
        Err(error) => {
            let changed = transaction.changed();
            // The child remains owned through the durable commit, so every
            // post-spawn error (including a journal flush failure) stops and
            // waits for exactly that child before any rollback can begin.
            let rollback = rollback_failed(&mut transaction, &mut owned_child);
            let message = match rollback {
                Ok(()) => format!(
                    "Update failed: {error}. The previous package was restored. Recovery files: {}",
                    stage.display()
                ),
                Err(rollback) => format!(
                    "Update failed: {error}. Rollback needs attention: {rollback}. Previous files are retained at {}",
                    stage.join("backup").display()
                ),
            };
            let _ = transaction.journal("failed", None);
            let _ = save_result(stage, plan.result_path.as_deref(), &message);
            if changed && transaction.restored() {
                // A failed replacement gets one strict restart of the restored
                // app. It may not take over another independently started GUI.
                if let Err(restart) = spawn_app(&plan, stage, false) {
                    let _ = save_result(
                        stage,
                        plan.result_path.as_deref(),
                        &format!("{message}. Start the app manually: {restart}"),
                    );
                }
            }
            Err(io::Error::other(message))
        }
    }
}

fn wait_parent(parent: HANDLE, timeout: Duration) -> io::Result<()> {
    match unsafe { WaitForSingleObject(parent, timeout.as_millis().min(u32::MAX as u128) as u32) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(io::Error::other(
            "The original app did not exit; no package files were changed",
        )),
        _ => Err(io::Error::last_os_error()),
    }
}

fn spawn_app(plan: &Plan, stage: &Path, updated: bool) -> io::Result<Child> {
    let mut command = Command::new(&plan.executable);
    command
        .args(plan.args.iter().map(|arg| OsString::from_wide(arg)))
        .current_dir(&plan.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    if updated {
        command
            .env(RESTART_ENV, "updated")
            .env(ACK_DIR_ENV, stage)
            .env(ACK_NONCE_ENV, &plan.nonce);
    } else {
        command
            .env(RESTART_ENV, "rollback")
            .env_remove(ACK_DIR_ENV)
            .env_remove(ACK_NONCE_ENV);
    }
    command.spawn()
}

pub(super) fn is_update_restart() -> bool {
    std::env::var_os(RESTART_ENV).is_some()
}

pub(super) fn acknowledge_startup() {
    if std::env::var(RESTART_ENV).as_deref() != Ok("updated") {
        return;
    }
    // The call itself is made on the first successful GUI event-loop turn.
    // Hashing the running image and flushing its acknowledgement stay off it.
    let _ = std::thread::Builder::new()
        .name("update-startup-ack".into())
        .spawn(|| {
            let _ = acknowledge_startup_inner();
        });
}

fn acknowledge_startup_inner() -> io::Result<()> {
    if std::env::var(RESTART_ENV).as_deref() != Ok("updated") {
        return Ok(());
    }
    let stage = PathBuf::from(
        std::env::var_os(ACK_DIR_ENV)
            .ok_or_else(|| io::Error::other("Missing update startup directory"))?,
    );
    let _pins = pin_directory(&stage)?;
    let executable = std::env::current_exe()?.canonicalize()?;
    if stage.parent() != executable.parent()
        || !stage
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(STAGE_PREFIX))
    {
        return Err(io::Error::other(
            "Startup acknowledgement escaped its install directory",
        ));
    }
    let plan: Plan = read_bounded(&stage.join("plan.json"), MAX_PLAN)?;
    let nonce = std::env::var(ACK_NONCE_ENV).map_err(io::Error::other)?;
    if plan.schema != 1
        || plan.nonce != nonce
        || !native::same_path(&plan.executable, &executable)
        || plan.version != env!("AGENT_COMPANION_SEMVER_VERSION")
        || cfg!(companion_local_build)
    {
        return Err(io::Error::other(
            "Restarted executable did not match the selected release",
        ));
    }
    let expected = plan
        .entries
        .iter()
        .find(|entry| entry.new.name == "agent-companion.exe")
        .ok_or_else(|| io::Error::other("Missing GUI package entry"))?;
    if super::hash_file(&executable)? != expected.new.sha256 {
        return Err(io::Error::other(
            "Restarted executable checksum did not match",
        ));
    }
    write_json_new(
        &stage.join("startup-ack.json"),
        &StartupAck {
            schema: 1,
            nonce,
            pid: std::process::id(),
            version: env!("AGENT_COMPANION_SEMVER_VERSION").into(),
        },
    )
}

fn wait_startup(child: &mut Child, plan: &Plan, stage: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "Updated app exited during startup ({status})"
            )));
        }
        match read_bounded::<StartupAck>(&stage.join("startup-ack.json"), MAX_RESULT) {
            Ok(ack)
                if ack.schema == 1
                    && ack.nonce == plan.nonce
                    && ack.pid == child.id()
                    && ack.version == plan.version =>
            {
                std::thread::sleep(Duration::from_millis(250));
                if child.try_wait()?.is_none() {
                    return Ok(());
                }
                return Err(io::Error::other(
                    "Updated app stopped immediately after startup",
                ));
            }
            Ok(_) => {
                return Err(io::Error::other(
                    "Updated app sent an invalid startup acknowledgement",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // The child writes and flushes this very small new file. A sharing
            // violation while it is open is transient; corrupt data is fatal.
            Err(error) if error.raw_os_error() == Some(32) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Updated app did not confirm GUI startup"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop_owned(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    child.kill()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Updater child did not stop"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn rollback_failed(transaction: &mut Transaction<'_>, child: &mut Option<Child>) -> io::Result<()> {
    if let Some(child) = child.as_mut() {
        stop_owned(child).map_err(|error| io::Error::other(format!("Cannot stop the updater-owned child: {error}; package files were retained without another restart")))?;
    }
    transaction.rollback()
}

pub(super) fn read_result(path: &Path) -> Option<String> {
    let result: ResultMessage = read_bounded(path, MAX_RESULT).ok()?;
    (!result.message.is_empty()).then_some(result.message)
}

fn save_result(stage: &Path, path: Option<&Path>, message: &str) -> io::Result<()> {
    let result = ResultMessage {
        message: message.chars().take(8000).collect(),
    };
    let _ = replace_result(&stage.join("result.json"), &result);
    if let Some(path) = path {
        replace_result(path, &result)?;
    }
    Ok(())
}

fn replace_result(path: &Path, result: &ResultMessage) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid update result path"))?;
    fs::create_dir_all(parent)?;
    let _pins = pin_directory(parent)?;
    native::regular_or_absent(path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, result)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

// The mutation implementation and native fixtures are kept below the protocol
// so the startup/exit safety boundary can be reviewed independently.
struct Record {
    new: File,
    old: Option<File>,
    retired: bool,
    installed: bool,
}

struct Transaction<'a> {
    plan: &'a Plan,
    stage: &'a Path,
    records: Vec<Record>,
    journal: File,
    _lock: File,
    _pins: Vec<native::DirectoryPins>,
    docs_pin: Option<native::DirectoryPins>,
    created_docs: bool,
}

fn probe_writable(directory: &Path, stage: &Path, name: &str) -> io::Result<()> {
    let mut probe = tempfile::Builder::new()
        .prefix(".agent-companion-update-probe-")
        .tempfile_in(directory)?;
    probe.write_all(b"Agent Companion update permission probe\n")?;
    probe.as_file().sync_all()?;
    let destination = stage.join(name);
    let file = probe
        .persist_noclobber(&destination)
        .map_err(|error| error.error)?;
    drop(file);
    fs::remove_file(destination)
}

impl<'a> Transaction<'a> {
    fn initialize(plan: &'a Plan, stage: &'a Path) -> io::Result<Self> {
        let directory = plan.executable.parent().unwrap();
        let mut pins = vec![pin_directory(directory)?, pin_directory(stage)?];
        let lock_path = directory.join(".agent-companion-update.lock");
        native::regular_or_absent(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|_| io::Error::other("Another package update is already running"))?;
        for name in [
            "payload",
            "payload/docs",
            "backup",
            "backup/docs",
            "discard",
            "discard/docs",
        ] {
            pins.push(pin_directory(&stage.join(name))?);
        }
        match fs::symlink_metadata(directory.join("docs")) {
            Ok(_) => pins.push(pin_directory(&directory.join("docs"))?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // Prove create + source-rename permission in each real destination
        // directory before the GUI exits. Only random, self-owned probe names
        // are created; package files remain untouched until parent termination.
        probe_writable(directory, stage, "permission-probe-root")?;
        if directory.join("docs").exists() {
            probe_writable(&directory.join("docs"), stage, "permission-probe-docs")?;
        }
        let mut records = Vec::new();
        for entry in &plan.entries {
            let path = stage.join("payload").join(&entry.new.name);
            let mut new = native::open_held(&path, true)?;
            if native::fingerprint(&mut new)?
                != (Fingerprint {
                    size: entry.new.size,
                    sha256: entry.new.sha256.clone(),
                })
            {
                return Err(io::Error::other(
                    "Staged package changed before helper initialization",
                ));
            }
            if entry.new.name.ends_with(".exe") {
                package::validate_executable(&path)?;
            }
            let target = directory.join(&entry.new.name);
            let old = if native::regular_or_absent(&target)? {
                let mut old = native::open_held(&target, true).map_err(|error| io::Error::new(error.kind(), format!("Cannot prepare {}: {error}. Close the program using this package and try again", entry.new.name)))?;
                if Some(native::fingerprint(&mut old)?) != entry.old {
                    return Err(io::Error::other(
                        "Installed package changed before helper initialization",
                    ));
                }
                Some(old)
            } else {
                if entry.old.is_some() {
                    return Err(io::Error::other("Installed package entry disappeared"));
                }
                None
            };
            if fs::symlink_metadata(stage.join("backup").join(&entry.new.name)).is_ok()
                || fs::symlink_metadata(stage.join("discard").join(&entry.new.name)).is_ok()
            {
                return Err(io::Error::other("Update backup paths are already occupied"));
            }
            records.push(Record {
                new,
                old,
                retired: false,
                installed: false,
            });
        }
        let journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(stage.join("journal.jsonl"))?;
        let mut transaction = Self {
            plan,
            stage,
            records,
            journal,
            _lock: lock,
            _pins: pins,
            docs_pin: None,
            created_docs: false,
        };
        transaction.journal("prepared", None)?;
        Ok(transaction)
    }

    fn journal(&mut self, event: &str, index: Option<usize>) -> io::Result<()> {
        serde_json::to_writer(
            &mut self.journal,
            &serde_json::json!({"event": event, "entry": index}),
        )?;
        self.journal.write_all(b"\n")?;
        self.journal.sync_all()
    }

    fn apply(&mut self) -> io::Result<()> {
        let directory = self.plan.executable.parent().unwrap();
        let docs = directory.join("docs");
        match fs::symlink_metadata(&docs) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.journal("intent-create-docs", None)?;
                fs::create_dir(&docs)?;
                self.created_docs = true;
                self.journal("created-docs", None)?;
            }
            Err(error) => return Err(error),
        }
        // A docs directory missing at initialization may have appeared while
        // waiting for the parent. Pin and revalidate it even in that case.
        self.docs_pin = Some(pin_directory(&docs)?);
        let mut order = (0..self.records.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| self.plan.entries[*index].new.name == "agent-companion.exe");
        for index in order {
            let name = self.plan.entries[index].new.name.clone();
            let target = directory.join(&name);
            if self.records[index].old.is_some() {
                self.journal("intent-retire", Some(index))?;
                native::rename_bounded(
                    self.records[index].old.as_ref().unwrap(),
                    &self.stage.join("backup").join(&name),
                )
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "Cannot retire {name}: {error}; no running TUI or wrapper was stopped"
                        ),
                    )
                })?;
                self.records[index].retired = true;
                self.journal("retired", Some(index))?;
            }
            self.journal("intent-install", Some(index))?;
            native::rename_bounded(&self.records[index].new, &target)?;
            self.records[index].installed = true;
            self.journal("installed", Some(index))?;
        }
        Ok(())
    }

    fn changed(&self) -> bool {
        self.records
            .iter()
            .any(|record| record.retired || record.installed)
    }

    fn restored(&self) -> bool {
        !self.changed()
    }

    fn rollback(&mut self) -> io::Result<()> {
        let directory = self.plan.executable.parent().unwrap();
        let mut errors = Vec::new();
        // Reverse actual publication order, including the GUI which is last.
        let mut order = (0..self.records.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| self.plan.entries[*index].new.name == "agent-companion.exe");
        for index in order.into_iter().rev() {
            let name = self.plan.entries[index].new.name.clone();
            if self.records[index].installed {
                let _ = self.journal("intent-remove-new", Some(index));
                match native::rename_bounded(
                    &self.records[index].new,
                    &self.stage.join("discard").join(&name),
                ) {
                    Ok(()) => {
                        self.records[index].installed = false;
                        let _ = self.journal("removed-new", Some(index));
                    }
                    Err(error) => {
                        errors.push(format!("{name}: cannot move replacement aside ({error})"));
                        continue;
                    }
                }
            }
            if self.records[index].retired {
                let _ = self.journal("intent-restore", Some(index));
                match native::rename_bounded(
                    self.records[index].old.as_ref().unwrap(),
                    &directory.join(&name),
                ) {
                    Ok(()) => {
                        self.records[index].retired = false;
                        let _ = self.journal("restored", Some(index));
                    }
                    Err(error) => errors.push(format!(
                        "{name}: previous file retained in backup ({error})"
                    )),
                }
            }
        }
        // Never recursively remove a package or user directory. If a formerly
        // missing docs directory now has unrelated entries, leave it intact.
        if self.created_docs {
            self.docs_pin = None;
            let _ = fs::remove_dir(directory.join("docs"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }

    fn cleanup_committed(&mut self) {
        // Images still mapped by a sibling process may resist deletion. Keep
        // those backups without touching the process; they are valid recovery
        // files, and no uncommitted stage is recursively cleaned on startup.
        self.records.clear();
        for entry in &self.plan.entries {
            let _ = fs::remove_file(self.stage.join("backup").join(&entry.new.name));
        }
        let _ = fs::remove_file(self.stage.join("download.zip"));
    }
}

#[cfg(test)]
mod tests;
