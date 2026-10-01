use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{Environment, Session, safe_text, valid_id};

/// Inventory is independent of monitoring preferences and includes every matching
/// directory, not just the monitor's recent/active window. Symlinks are resolved
/// for exact directory identity; descendants are deliberately not included.
pub fn discover_sessions(environments: &[Environment], cwd: &Path) -> Result<Vec<Session>, String> {
    let cwd = fs::canonicalize(cwd).map_err(|_| "无法定位当前项目目录。")?;
    let mut result = Vec::new();
    for environment in environments {
        if !environment.home.is_dir() {
            continue;
        }
        let mut sessions = HashMap::<String, Session>::new();
        let mut paths = Vec::new();
        collect(&environment.home.join("sessions"), &mut paths)?;
        for path in paths {
            if let Some(mut session) = read_rollout(environment, &path)? {
                if !same_directory(&session.cwd, &cwd) {
                    continue;
                }
                session.cwd = cwd.clone();
                if let Some(previous) = sessions.get_mut(&session.id) {
                    if previous.rollout_path != session.rollout_path {
                        previous
                            .blockers
                            .push("同一存储环境存在多个同 ID 历史文件，无法确定原会话。".into());
                    }
                } else {
                    sessions.insert(session.id.clone(), session);
                }
            }
        }
        merge_database(environment, &cwd, &mut sessions)?;
        for session in sessions.values_mut() {
            match writer_busy(&environment.home, &session.id) {
                Ok(busy) => session.busy = busy,
                Err(reason) => session.blockers.push(reason),
            }
        }
        result.extend(sessions.into_values());
    }
    result.sort_by(|left, right| {
        right
            .modified_secs
            .cmp(&left.modified_secs)
            .then_with(|| left.environment_id.cmp(&right.environment_id))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(result)
}

pub(super) fn same_directory(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("无法读取会话历史目录。".into()),
    };
    for entry in entries {
        let entry = entry.map_err(|_| "无法读取会话目录项。")?;
        let kind = entry.file_type().map_err(|_| "无法检查会话目录项。")?;
        // Never follow arbitrary filesystem links or recurse into link cycles.
        if kind.is_dir() {
            collect(&entry.path(), paths)?;
        } else if kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn read_rollout(environment: &Environment, path: &Path) -> Result<Option<Session>, String> {
    let file = File::open(path).map_err(|_| "无法读取会话元数据。")?;
    let modified_secs = file
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_secs());
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    // Session metadata is a single first line. Bound allocations even when an
    // unrelated/corrupt file contains no newline.
    use std::io::Read;
    reader
        .by_ref()
        .take(4 * 1024 * 1024)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "无法读取会话元数据。")?;
    let record: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if record["type"] != "session_meta" {
        return Ok(None);
    }
    let payload = &record["payload"];
    let Some(id) = payload["id"]
        .as_str()
        .or_else(|| payload["session_id"].as_str())
        .filter(|id| valid_id(id))
    else {
        return Ok(None);
    };
    if payload["source"].get("subagent").is_some() || payload["source"] == "subagent" {
        return Ok(None);
    }
    let Some(cwd) = payload["cwd"].as_str() else {
        return Ok(None);
    };
    let mut title = payload["title"]
        .as_str()
        .map(safe_text)
        .unwrap_or_else(|| id.to_owned());
    for _ in 0..100 {
        bytes.clear();
        let count = reader
            .by_ref()
            .take(256 * 1024)
            .read_until(b'\n', &mut bytes)
            .map_err(|_| "无法读取会话标题。")?;
        if count == 0 {
            break;
        }
        if bytes.last() != Some(&b'\n') {
            break;
        }
        if let Ok(record) = serde_json::from_slice::<Value>(&bytes)
            && record["type"] == "event_msg"
            && record["payload"]["type"] == "user_message"
            && let Some(message) = record["payload"]["message"].as_str()
        {
            title = safe_text(
                &message
                    .split_whitespace()
                    .take(40)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            break;
        }
    }
    Ok(Some(Session {
        id: id.to_owned(),
        title,
        cwd: PathBuf::from(cwd),
        rollout_path: path.to_owned(),
        environment_id: environment.id.clone(),
        modified_secs,
        creation_source: None,
        busy: false,
        blockers: Vec::new(),
    }))
}

fn read_only(path: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "无法只读打开原会话数据库。")?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(|_| "无法检查原会话数据库。")?;
    db.execute_batch("PRAGMA query_only = ON; PRAGMA trusted_schema = OFF;")
        .map_err(|_| "无法检查原会话数据库。")?;
    Ok(db)
}

pub(super) fn database_home(environment: &Environment) -> PathBuf {
    if let Some(path) = &environment.database_home {
        return path.clone();
    }
    if let Ok(text) = fs::read_to_string(environment.home.join("config.toml"))
        && let Ok(document) = text.parse::<toml_edit::DocumentMut>()
        && let Some(path) = document
            .get("sqlite_home")
            .and_then(toml_edit::Item::as_str)
    {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            path
        } else {
            environment.home.join(path)
        };
    }
    environment.home.clone()
}

fn merge_database(
    environment: &Environment,
    cwd: &Path,
    sessions: &mut HashMap<String, Session>,
) -> Result<(), String> {
    let database_home = database_home(environment);
    if let Ok(entries) = fs::read_dir(&database_home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("state_") && name.ends_with(".sqlite") && name != "state_5.sqlite" {
                return Err("发现尚未验证版本的会话数据库；未将旧格式列表作为全部历史。".into());
            }
        }
    }
    let state_path = database_home.join("state_5.sqlite");
    if !state_path.exists() {
        return Ok(());
    }
    let db = read_only(&state_path)?;
    let columns: HashSet<String> = db
        .prepare("PRAGMA table_info(threads)")
        .map_err(|_| "会话数据库结构不兼容。")?
        .query_map([], |row| row.get(1))
        .map_err(|_| "会话数据库结构不兼容。")?
        .collect::<Result<_, _>>()
        .map_err(|_| "会话数据库结构不兼容。")?;
    if ![
        "id",
        "cwd",
        "title",
        "archived",
        "updated_at",
        "rollout_path",
        "source",
    ]
    .iter()
    .all(|name| columns.contains(*name))
    {
        return Err("原会话数据库结构未验证；未将不完整列表作为全部历史。".into());
    }
    let title = if columns.contains("name") {
        "COALESCE(NULLIF(name, ''), title)"
    } else {
        "title"
    };
    let excluded: HashSet<String> = db.prepare("SELECT id FROM threads WHERE archived != 0 OR source NOT IN ('cli', 'vscode', 'appServer')")
        .map_err(|_| "无法确认已归档会话。")?.query_map([], |row| row.get(0))
        .map_err(|_| "无法确认已归档会话。")?.collect::<Result<_, _>>().map_err(|_| "无法确认已归档会话。")?;
    sessions.retain(|id, _| !excluded.contains(id));
    let mode = if columns.contains("history_mode") {
        "history_mode"
    } else {
        "'legacy'"
    };
    let sql = format!(
        "SELECT id, cwd, {title}, updated_at, rollout_path, {mode} FROM threads WHERE archived = 0 AND source IN ('cli', 'vscode', 'appServer')"
    );
    let mut query = db.prepare(&sql).map_err(|_| "原会话数据库查询不兼容。")?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| "无法读取原会话数据库。")?;
    for row in rows {
        let (id, directory, title, modified, rollout, mode) =
            row.map_err(|_| "无法读取原会话数据库行。")?;
        if !valid_id(&id) || !same_directory(Path::new(&directory), cwd) {
            continue;
        }
        let mut blockers = Vec::new();
        if !matches!(mode.as_str(), "paginated" | "legacy") {
            blockers.push("原会话使用未验证的历史存储模式。".into());
        }
        let rollout_path = if mode == "paginated" {
            let history = database_home.join("thread_history_1.sqlite");
            if !history.is_file() {
                blockers.push("分页历史数据库缺失。".into());
            }
            history
        } else {
            let path = PathBuf::from(rollout);
            if !path.is_file() {
                blockers.push("原生历史文件缺失；不会使用摘要或新建会话替代。".into());
            }
            path
        };
        if let Some(existing) = sessions.get_mut(&id) {
            existing.title = safe_text(&title);
            existing.modified_secs = existing.modified_secs.max(modified.max(0) as u64);
            if mode == "paginated" {
                existing.rollout_path = rollout_path;
            } else if existing.rollout_path != rollout_path {
                existing
                    .blockers
                    .push("数据库与磁盘历史指向不同原会话文件，接力已禁用。".into());
            }
            existing.blockers.extend(blockers);
        } else {
            sessions.insert(
                id.clone(),
                Session {
                    id,
                    title: safe_text(&title),
                    cwd: cwd.to_owned(),
                    rollout_path,
                    environment_id: environment.id.clone(),
                    modified_secs: modified.max(0) as u64,
                    creation_source: None,
                    busy: false,
                    blockers,
                },
            );
        }
    }
    Ok(())
}

pub(super) fn writer_busy(home: &Path, id: &str) -> Result<bool, String> {
    if !valid_id(id) {
        return Err("会话 ID 无效。".into());
    }
    let directory = home.join("thread-writer-locks");
    let coordination = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join(".coordination.lock"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !directory.exists() => {
            return Ok(false);
        }
        Err(_) => return Err("无法验证原生写入协调锁；请先退出原会话。".into()),
    };
    coordination
        .try_lock_shared()
        .map_err(|_| "原生客户端正在修改写入锁，请稍后重试。")?;
    let writer = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join(format!("{id}.lock")))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("无法验证原生会话写入锁。".into()),
    };
    match writer.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(_)) => Err("无法验证原生会话写入锁。".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    const ID: &str = "01999999-0000-7000-8000-000000000001";
    fn environment(root: &Path) -> Environment {
        let home = root.join("source");
        fs::create_dir_all(home.join("sessions/2000/01/01")).unwrap();
        Environment {
            id: "codex".into(),
            label: "Codex".into(),
            home,
            executable: root.join("codex"),
            database_home: None,
        }
    }
    fn rollout(environment: &Environment, cwd: &Path, id: &str, filename: &str) -> PathBuf {
        let path = environment.home.join("sessions/2000/01/01").join(filename);
        let records = [
            json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"source":"cli"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Original conversation"}}),
        ];
        fs::write(
            &path,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        path
    }
    fn database(environment: &Environment) -> Connection {
        let db = Connection::open(environment.home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads (id TEXT, cwd TEXT, title TEXT, archived INTEGER, updated_at INTEGER, rollout_path TEXT, history_mode TEXT, source TEXT);").unwrap();
        db
    }
    #[test]
    fn discovers_old_sessions_only_in_exact_current_directory() {
        let root = TempDir::new().unwrap();
        let environment = environment(root.path());
        let cwd = root.path().join("project");
        fs::create_dir_all(cwd.join("child")).unwrap();
        rollout(&environment, &cwd, ID, "rollout-original.jsonl");
        rollout(
            &environment,
            &cwd.join("child"),
            "01999999-0000-7000-8000-000000000002",
            "rollout-child.jsonl",
        );
        let sessions = discover_sessions(&[environment], &cwd).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, ID);
        assert_eq!(sessions[0].creation_source, None);
        assert_eq!(sessions[0].environment_id, "codex");
        assert_eq!(sessions[0].title, "Original conversation");
    }
    #[test]
    fn duplicate_storage_blocks_instead_of_choosing_latest_copy() {
        let root = TempDir::new().unwrap();
        let environment = environment(root.path());
        rollout(&environment, root.path(), ID, "rollout-one.jsonl");
        rollout(&environment, root.path(), ID, "rollout-two.jsonl");
        let sessions = discover_sessions(&[environment], root.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(!sessions[0].blockers.is_empty());
    }
    #[test]
    fn paginated_sessions_do_not_need_a_rollout_and_subagents_stay_excluded() {
        let root = TempDir::new().unwrap();
        let environment = environment(root.path());
        let db = database(&environment);
        for (id, source) in [
            (ID, "appServer"),
            ("01999999-0000-7000-8000-000000000002", "subagent"),
        ] {
            db.execute(
                "INSERT INTO threads VALUES (?1,?2,'Paged conversation',0,100,'','paginated',?3)",
                rusqlite::params![id, root.path().to_str().unwrap(), source],
            )
            .unwrap();
        }
        File::create(environment.home.join("thread_history_1.sqlite")).unwrap();
        let sessions = discover_sessions(std::slice::from_ref(&environment), root.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, ID);
        assert_eq!(
            sessions[0].rollout_path,
            environment.home.join("thread_history_1.sqlite")
        );
    }
    #[test]
    fn unknown_database_versions_and_storage_modes_fail_closed() {
        let root = TempDir::new().unwrap();
        let environment = environment(root.path());
        let db = database(&environment);
        db.execute(
            "INSERT INTO threads VALUES (?1,?2,'Unknown mode',0,100,'','future','cli')",
            rusqlite::params![ID, root.path().to_str().unwrap()],
        )
        .unwrap();
        let sessions = discover_sessions(std::slice::from_ref(&environment), root.path()).unwrap();
        assert!(
            sessions[0]
                .blockers
                .iter()
                .any(|reason| reason.contains("存储模式"))
        );
        File::create(environment.home.join("state_6.sqlite")).unwrap();
        assert!(discover_sessions(&[environment], root.path()).is_err());
    }
}
