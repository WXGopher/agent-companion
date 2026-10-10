//! Codex rate-limit usage from local session rollout logs.
//!
//! Codex writes its rate limits into session rollout logs. Compare the times
//! of their last `token_count` readings, not file modification times: Windows
//! can leave those unchanged while Codex still holds the writer open.
//!
//! ## Field names, verified against real rollout files
//!
//! Checked read-only against this machine's own `~/.codex/sessions` before
//! writing the parser, because the shapes have drifted between Codex builds:
//!
//! - `rate_limits` sits at `payload.rate_limits`, a **sibling of `payload.info`**
//!   — not inside it. `info` is sometimes `null` while `rate_limits` is present.
//! - `plan_type` is **inside `rate_limits`**, not at the payload's top level as
//!   the design note assumed. Recent records read
//!   `{"limit_id":…,"limit_name":…,"primary":…,"secondary":…,"credits":…,
//!   "individual_limit":…,"plan_type":"pro","rate_limit_reached_type":…}`.
//! - `primary` and `secondary` are each either a window object or `null`.
//! - A window carries `used_percent` and `window_minutes`, plus **either**
//!   `resets_at` (absolute Unix seconds, current builds) **or**
//!   `resets_in_seconds` (relative, older builds). Both spellings are still on
//!   disk here, so both are parsed.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A byte-order mark, which `serde_json` will not parse past.
pub const BOM: char = '\u{feff}';

/// Parse JSON that another program wrote.
///
/// Windows tools — PowerShell above all, whose `Set-Content` does it by default
/// — write UTF-8 with a byte-order mark, and `serde_json` rejects a document
/// that starts with one. Agent Companion reads several files it did not write, so every
/// one of those reads comes through here.
///
/// This one cost real time to find: the user's own usage cache parsed as empty
/// and the readout said "usage unavailable" with perfectly good numbers sitting
/// on disk three bytes away.
pub fn parse_foreign_json(text: &str) -> Option<Value> {
    serde_json::from_str(text.trim_start_matches(BOM)).ok()
}

/// Prefix of every Codex rollout log.
const ROLLOUT_PREFIX: &str = "rollout-";

/// Read backwards in chunks so long sessions normally cost only one tail read.
const CODEX_TAIL_CHUNK: usize = 64 * 1024;

/// One rate-limit window, reported by Codex.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowUsage {
    /// How much of the window is consumed, 0–100.
    pub used_percent: f64,
    /// When the window rolls over, in Unix seconds.
    pub resets_at: Option<u64>,
    /// The window's length in minutes.
    pub window_minutes: Option<u64>,
}

/// Seconds since the Unix epoch for an RFC 3339 timestamp.
///
/// Written out rather than pulled in: the shapes that actually arrive are
/// `2026-08-24T00:39:59.848967+08:00` and its `Z` variant, and a date-time crate
/// would be a large dependency for one field.
pub fn parse_iso8601(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    // Split the offset off the end before touching the clock.
    let (clock, offset_secs) = match rest.rfind(['+', '-']) {
        Some(index) => {
            let (clock, offset) = rest.split_at(index);
            (clock, parse_offset(offset)?)
        }
        None => (rest.trim_end_matches(['Z', 'z']), 0),
    };

    let clock = clock.split('.').next()?;
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next().unwrap_or("0").parse().ok()?;

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_secs;
    u64::try_from(seconds).ok()
}

fn parse_offset(text: &str) -> Option<i64> {
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = &text[1..];
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 => digits.split_at(2),
        None => (digits, "0"),
    };
    Some(sign * (hours.parse::<i64>().ok()? * 3_600 + minutes.parse::<i64>().ok()? * 60))
}

/// Days from 1970-01-01 to a proleptic Gregorian date. Howard Hinnant's
/// `days_from_civil`, which is the shortest correct way to do this.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Codex's two windows, plus the plan they belong to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexUsage {
    /// The short window — five hours on current plans.
    pub primary: Option<WindowUsage>,
    /// The long window — a week on current plans.
    pub secondary: Option<WindowUsage>,
    /// `rate_limits.plan_type`, e.g. `pro`.
    pub plan_type: Option<String>,
    /// The rollout file these numbers came from.
    pub source: Option<PathBuf>,
}

impl CodexUsage {
    pub fn is_empty(&self) -> bool {
        self.primary.is_none() && self.secondary.is_none()
    }
}

/// Agent Companion's local data directory.
pub fn agent_companion_data_dir() -> io::Result<PathBuf> {
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(local).join("AgentCompanion"));
    }
    let home = crate::home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine the local data directory",
        )
    })?;
    Ok(home.join(".agent-companion"))
}

// -------------------------------------------------------------------- Codex

/// The same scan with an explicit Codex data directory (including CODEX_HOME).
pub fn scan_codex_usage_at(codex_home: &Path) -> io::Result<Option<CodexUsage>> {
    let mut rollouts = Vec::new();
    collect_rollouts(&codex_home.join("sessions"), &mut rollouts)?;
    rollouts.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

    let mut latest = None;
    let mut latest_key = None;
    // An old session can be resumed after any number of newer ones. Capping
    // this list by mtime would exclude exactly the reading we need.
    for (path, modified) in rollouts {
        let Ok(Some((mut usage, recorded_at))) = read_codex_reading(&path) else {
            continue;
        };
        let key = (recorded_at.unwrap_or(modified), modified, path.clone());
        if latest_key.as_ref().is_none_or(|previous| key > *previous) {
            latest_key = Some(key);
            usage.source = Some(path);
            latest = Some(usage);
        }
    }
    Ok(latest)
}

/// The last `token_count` event in one rollout file, or `None` if it has none.
pub fn read_codex_rollout(path: &Path) -> io::Result<Option<CodexUsage>> {
    Ok(read_codex_reading(path)?.map(|(usage, _)| usage))
}

fn read_codex_reading(path: &Path) -> io::Result<Option<(CodexUsage, Option<SystemTime>)>> {
    let mut file = fs::File::open(path)?;
    let mut position = file.metadata()?.len();
    let mut chunk = [0; CODEX_TAIL_CHUNK];
    // A line spanning chunks is accumulated in reverse byte order, then
    // reversed once. Even very long messages remain linear to read.
    let mut carry = Vec::new();
    while position > 0 {
        let count = position.min(CODEX_TAIL_CHUNK as u64) as usize;
        position -= count as u64;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut chunk[..count])?;
        let mut lines = chunk[..count].rsplit(|byte| *byte == b'\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() && position > 0 {
                carry.extend(line.iter().rev());
                break;
            }
            let reading = if carry.is_empty() {
                parse_codex_reading(line)
            } else {
                carry.extend(line.iter().rev());
                carry.reverse();
                let reading = parse_codex_reading(&carry);
                carry.clear();
                reading
            };
            if reading.is_some() {
                return Ok(reading);
            }
        }
    }
    Ok(None)
}

fn parse_codex_reading(line: &[u8]) -> Option<(CodexUsage, Option<SystemTime>)> {
    if !line
        .windows(b"token_count".len())
        .any(|part| part == b"token_count")
    {
        return None;
    }
    let record: Value = serde_json::from_slice(line).ok()?;
    let payload = record.get("payload")?;
    if record.get("type")?.as_str()? != "event_msg"
        || payload.get("type")?.as_str()? != "token_count"
    {
        return None;
    }
    let usage = parse_codex_rate_limits(payload.get("rate_limits")?);
    if usage.is_empty() {
        return None;
    }
    let at = record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|text| {
            let seconds = parse_iso8601(text)?;
            let fraction = text
                .split_once('.')
                .map(|(_, fraction)| fraction)
                .unwrap_or("");
            let mut nanos = 0;
            let mut place = 100_000_000;
            for digit in fraction.bytes().take(9).take_while(u8::is_ascii_digit) {
                nanos += u32::from(digit - b'0') * place;
                place /= 10;
            }
            SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
        });
    Some((usage, at))
}

/// Parse Codex's `rate_limits` object.
pub fn parse_codex_rate_limits(value: &Value) -> CodexUsage {
    // A rollout can interleave account usage with other quota buckets. Only
    // the Codex bucket describes the account allowance shown by Companion;
    // a later, unused bucket must not turn its weekly card into "100% left".
    // Older clients omitted the bucket ID, so retain that legacy format.
    if let Some(id) = value
        .get("limit_id")
        .or_else(|| value.get("limitId"))
        .filter(|id| !id.is_null())
        && id.as_str() != Some("codex")
    {
        return CodexUsage::default();
    }
    CodexUsage {
        primary: parse_window(value.get("primary")),
        secondary: parse_window(value.get("secondary")),
        plan_type: value
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plan| !plan.is_empty() && *plan != "unknown")
            .map(str::to_string),
        source: None,
    }
}

fn collect_rollouts(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_rollouts(&path, out)?;
            continue;
        }
        let is_rollout = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(ROLLOUT_PREFIX) && name.ends_with(".jsonl"));
        if is_rollout && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
            out.push((path, modified));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ parsing

/// One Codex window, or `None` when the value is absent,
/// `null`, or carries no percentage.
fn parse_window(value: Option<&Value>) -> Option<WindowUsage> {
    let value = value?;
    if value.is_null() {
        return None;
    }

    let used_percent =
        number(value.get("used_percent")).or_else(|| number(value.get("usedPercent")))?;

    // Absolute reset time where available; older Codex builds only give the
    // remaining seconds, which is useless without knowing when it was written —
    // so it is resolved against the current clock at parse time.
    let resets_at = number(value.get("resets_at"))
        .or_else(|| number(value.get("resetsAt")))
        .map(|seconds| seconds as u64)
        .or_else(|| {
            let remaining = number(value.get("resets_in_seconds"))
                .or_else(|| number(value.get("resetsInSeconds")))?;
            Some(crate::now_unix_secs().saturating_add(remaining.max(0.0) as u64))
        });

    Some(WindowUsage {
        used_percent: used_percent.clamp(0.0, 100.0),
        resets_at,
        window_minutes: number(value.get("window_minutes"))
            .or_else(|| number(value.get("windowMinutes")))
            .map(|minutes| minutes as u64),
    })
}

/// A number, whether it arrived as one or as a string like `"23.5"` or `"23%"`.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().trim_end_matches('%').trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_sessions_dir(home: &Path) -> PathBuf {
        home.join(".codex").join("sessions")
    }

    fn scan_codex_usage(home: &Path) -> io::Result<Option<CodexUsage>> {
        scan_codex_usage_at(&home.join(".codex"))
    }
    use serde_json::json;
    use std::io::Write;

    const NOW: u64 = 1_787_000_000;

    #[test]
    fn rfc_3339_timestamps_parse_in_every_shape_the_api_sends() {
        // Offsets, fractional seconds, and Z, against known epoch seconds.
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(
            parse_iso8601("2026-08-23T16:39:59Z"),
            parse_iso8601("2026-08-24T00:39:59+08:00")
        );
        assert_eq!(
            parse_iso8601("2026-08-24T00:39:59.848967+08:00"),
            parse_iso8601("2026-08-24T00:39:59+08:00")
        );
        assert_eq!(
            parse_iso8601("2026-08-24T00:39:59+0800"),
            parse_iso8601("2026-08-24T00:39:59+08:00")
        );
        // A leap year, and a date before the epoch, which has no representation.
        assert!(parse_iso8601("2024-02-29T12:00:00Z").is_some());
        assert_eq!(parse_iso8601("1969-12-31T23:59:59Z"), None);
        assert_eq!(parse_iso8601("not a timestamp"), None);
        assert_eq!(parse_iso8601(""), None);
    }

    fn token_count(primary: Value, secondary: Value, plan: &str) -> Value {
        json!({
            "timestamp": "2026-08-23T10:00:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {"total_tokens": 1_000},
                    "model_context_window": 258_400,
                },
                "rate_limits": {
                    "limit_id": "codex",
                    "limit_name": null,
                    "primary": primary,
                    "secondary": secondary,
                    "credits": null,
                    "individual_limit": null,
                    "plan_type": plan,
                    "rate_limit_reached_type": null,
                },
            },
        })
    }

    fn write_rollout(home: &Path, day: &str, name: &str, lines: &[Value]) -> PathBuf {
        let dir = codex_sessions_dir(home).join(day);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn codex_rate_limits_parse_in_the_current_shape() {
        let usage = parse_codex_rate_limits(&json!({
            "limit_id": "codex",
            "primary": {"used_percent": 23.0, "window_minutes": 10_080, "resets_at": NOW + 600},
            "secondary": null,
            "plan_type": "pro",
        }));

        let primary = usage.primary.unwrap();
        assert_eq!(primary.used_percent, 23.0);
        assert_eq!(primary.window_minutes, Some(10_080));
        assert_eq!(primary.resets_at, Some(NOW + 600));
        assert!(usage.secondary.is_none());
        assert_eq!(usage.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn explicit_non_codex_buckets_do_not_provide_codex_usage() {
        for key in ["limit_id", "limitId"] {
            let mut limits = json!({
                "primary": {"used_percent": 0, "window_minutes": 300},
                "secondary": {"used_percent": 0, "window_minutes": 10_080},
                "plan_type": "pro",
            });
            limits[key] = json!("synthetic-other-model");
            assert_eq!(parse_codex_rate_limits(&limits), CodexUsage::default());
        }
    }

    #[test]
    fn codex_and_legacy_bucket_identifiers_preserve_usage() {
        for identifier in [
            json!({"limit_id": "codex"}),
            json!({"limitId": "codex"}),
            json!({"limit_id": null}),
            json!({"limitId": null}),
            json!({}),
        ] {
            let mut limits = identifier;
            limits["secondary"] = json!({"used_percent": 33, "window_minutes": 10_080});
            assert_eq!(
                parse_codex_rate_limits(&limits)
                    .secondary
                    .unwrap()
                    .used_percent,
                33.0
            );
        }
    }

    fn bucket_token_count(bucket: &str, weekly_used: f64, timestamp: &str) -> Value {
        let mut record = token_count(
            json!(null),
            json!({"used_percent": weekly_used, "window_minutes": 10_080}),
            "pro",
        );
        record["timestamp"] = json!(timestamp);
        record["payload"]["rate_limits"]["limit_id"] = json!(bucket);
        record
    }

    #[test]
    fn later_other_bucket_does_not_replace_codex_in_one_rollout() {
        let home = tempfile::tempdir().unwrap();
        let path = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-multiple-buckets.jsonl",
            &[
                bucket_token_count("codex", 33.0, "2026-08-23T10:00:00.000Z"),
                bucket_token_count("synthetic-other-model", 0.0, "2026-08-23T10:00:01.000Z"),
            ],
        );

        let usage = read_codex_rollout(&path).unwrap().unwrap();
        assert_eq!(usage.secondary.unwrap().used_percent, 33.0);
    }

    #[test]
    fn newer_other_bucket_rollout_does_not_replace_codex_usage() {
        let home = tempfile::tempdir().unwrap();
        let codex = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-codex.jsonl",
            &[bucket_token_count(
                "codex",
                33.0,
                "2026-08-23T10:00:00.000Z",
            )],
        );
        let other = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-other-bucket.jsonl",
            &[bucket_token_count(
                "synthetic-other-model",
                0.0,
                "2026-08-23T11:00:00.000Z",
            )],
        );
        set_mtime(&codex, SystemTime::UNIX_EPOCH + Duration::from_secs(NOW));
        set_mtime(
            &other,
            SystemTime::UNIX_EPOCH + Duration::from_secs(NOW + 3600),
        );

        let usage = scan_codex_usage(home.path()).unwrap().unwrap();
        assert_eq!(usage.secondary.unwrap().used_percent, 33.0);
        assert_eq!(usage.source, Some(codex));
    }

    #[test]
    fn other_buckets_alone_leave_codex_usage_unavailable() {
        let home = tempfile::tempdir().unwrap();
        let path = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-other-only.jsonl",
            &[bucket_token_count(
                "synthetic-other-model",
                0.0,
                "2026-08-23T10:00:00.000Z",
            )],
        );

        assert!(read_codex_rollout(&path).unwrap().is_none());
        assert!(scan_codex_usage(home.path()).unwrap().is_none());
    }

    #[test]
    fn an_unused_codex_bucket_still_has_all_of_its_quota() {
        let home = tempfile::tempdir().unwrap();
        write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-codex-unused.jsonl",
            &[bucket_token_count("codex", 0.0, "2026-08-23T10:00:00.000Z")],
        );

        let usage = scan_codex_usage(home.path()).unwrap().unwrap();
        let weekly = usage.secondary.unwrap();
        assert_eq!(weekly.used_percent, 0.0);
    }

    #[test]
    fn the_older_relative_reset_spelling_is_understood() {
        // Older builds write `resets_in_seconds` instead of `resets_at`.
        let usage = parse_codex_rate_limits(&json!({
            "primary": {"used_percent": 3.0, "window_minutes": 299, "resets_in_seconds": 17_148},
        }));
        let resets_at = usage
            .primary
            .unwrap()
            .resets_at
            .expect("resolved to absolute");
        let now = crate::now_unix_secs();
        assert!(
            resets_at >= now + 17_100 && resets_at <= now + 17_200,
            "expected roughly now + 17148, got {resets_at} against {now}"
        );
    }

    #[test]
    fn an_unknown_plan_is_reported_as_no_plan() {
        let usage = parse_codex_rate_limits(&json!({
            "primary": {"used_percent": 1.0},
            "plan_type": "unknown",
        }));
        assert!(usage.plan_type.is_none());
    }

    #[test]
    fn the_last_token_count_in_the_file_wins() {
        let home = tempfile::tempdir().unwrap();
        let path = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-2026-08-23T10-00-00-synthetic.jsonl",
            &[
                json!({"type": "response_item", "payload": {"type": "message"}}),
                token_count(
                    json!({"used_percent": 10.0, "window_minutes": 300, "resets_at": NOW}),
                    json!(null),
                    "pro",
                ),
                json!({"type": "response_item", "payload": {"type": "message"}}),
                token_count(
                    json!({"used_percent": 42.0, "window_minutes": 300, "resets_at": NOW + 60}),
                    json!({"used_percent": 8.0, "window_minutes": 10_080, "resets_at": NOW + 99}),
                    "pro",
                ),
            ],
        );

        let usage = read_codex_rollout(&path).unwrap().unwrap();
        assert_eq!(usage.primary.unwrap().used_percent, 42.0);
        assert_eq!(usage.secondary.unwrap().used_percent, 8.0);
    }

    #[test]
    fn a_null_info_does_not_hide_the_rate_limits() {
        let home = tempfile::tempdir().unwrap();
        let path = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-2026-08-23T10-00-00-synthetic.jsonl",
            &[json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": null,
                    "rate_limits": {"primary": {"used_percent": 12.0}},
                },
            })],
        );
        assert_eq!(
            read_codex_rollout(&path)
                .unwrap()
                .unwrap()
                .primary
                .unwrap()
                .used_percent,
            12.0
        );
    }

    #[test]
    fn the_scan_prefers_the_newest_rollout() {
        let home = tempfile::tempdir().unwrap();
        let old = write_rollout(
            home.path(),
            "2026/08/22",
            "rollout-2026-08-22T10-00-00-synthetic.jsonl",
            &[token_count(
                json!({"used_percent": 5.0}),
                json!(null),
                "pro",
            )],
        );
        let new = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-2026-08-23T10-00-00-synthetic.jsonl",
            &[token_count(
                json!({"used_percent": 77.0}),
                json!(null),
                "prolite",
            )],
        );
        set_mtime(
            &old,
            std::time::SystemTime::now() - std::time::Duration::from_secs(86_400),
        );
        set_mtime(&new, std::time::SystemTime::now());

        let usage = scan_codex_usage(home.path()).unwrap().unwrap();
        assert_eq!(usage.primary.unwrap().used_percent, 77.0);
        assert_eq!(usage.plan_type.as_deref(), Some("prolite"));
        assert_eq!(usage.source.as_deref(), Some(new.as_path()));
    }

    #[test]
    fn the_scan_falls_through_a_rollout_with_no_token_count() {
        let home = tempfile::tempdir().unwrap();
        let with_numbers = write_rollout(
            home.path(),
            "2026/08/22",
            "rollout-2026-08-22T10-00-00-synthetic.jsonl",
            &[token_count(
                json!({"used_percent": 31.0}),
                json!(null),
                "pro",
            )],
        );
        // A session that ended before Codex ever reported usage.
        let empty = write_rollout(
            home.path(),
            "2026/08/23",
            "rollout-2026-08-23T10-00-00-synthetic.jsonl",
            &[json!({"type": "session_meta", "payload": {"id": "synthetic"}})],
        );
        set_mtime(
            &with_numbers,
            std::time::SystemTime::now() - std::time::Duration::from_secs(600),
        );
        set_mtime(&empty, std::time::SystemTime::now());

        let usage = scan_codex_usage(home.path()).unwrap().unwrap();
        assert_eq!(usage.primary.unwrap().used_percent, 31.0);
    }

    #[test]
    fn a_resumed_old_rollout_beats_more_than_five_newer_files() {
        let home = tempfile::tempdir().unwrap();
        let mut fresh = token_count(json!({"used_percent": 25}), json!(null), "pro");
        fresh["timestamp"] = json!("2026-09-10T07:25:12.617Z");
        let resumed = write_rollout(home.path(), "2026/08/23", "rollout-resumed.jsonl", &[fresh]);
        set_mtime(&resumed, SystemTime::UNIX_EPOCH + Duration::from_secs(NOW));
        for index in 0..8 {
            let mut stale = token_count(json!({"used_percent": 21}), json!(null), "pro");
            stale["timestamp"] = json!("2026-09-10T04:06:36.271Z");
            write_rollout(
                home.path(),
                "2026/09/10",
                &format!("rollout-new-{index}.jsonl"),
                &[stale],
            );
        }
        let usage = scan_codex_usage(home.path()).unwrap().unwrap();
        assert_eq!(usage.primary.unwrap().used_percent, 25.0);
        assert_eq!(usage.source, Some(resumed));
    }

    #[test]
    fn timestamps_include_subseconds_and_allow_quota_to_reset() {
        let home = tempfile::tempdir().unwrap();
        let mut reset = token_count(json!({"used_percent": 0}), json!(null), "pro");
        reset["timestamp"] = json!("2026-09-10T07:25:12.900Z");
        let earlier_file =
            write_rollout(home.path(), "2026/09/10", "rollout-reset.jsonl", &[reset]);
        set_mtime(
            &earlier_file,
            SystemTime::UNIX_EPOCH + Duration::from_secs(NOW),
        );
        let mut stale = token_count(json!({"used_percent": 99}), json!(null), "pro");
        stale["timestamp"] = json!("2026-09-10T15:25:12.100+08:00");
        write_rollout(home.path(), "2026/09/10", "rollout-stale.jsonl", &[stale]);
        let usage = scan_codex_usage_at(&home.path().join(".codex"))
            .unwrap()
            .unwrap();
        assert_eq!(usage.primary.unwrap().used_percent, 0.0);
    }

    #[test]
    fn tail_read_handles_long_lines_chunk_boundaries_and_an_incomplete_final_record() {
        let home = tempfile::tempdir().unwrap();
        let mut reading = token_count(json!({"used_percent": 25}), json!(null), "pro");
        reading["payload"]["padding"] = json!("中".repeat(CODEX_TAIL_CHUNK));
        let path = write_rollout(
            home.path(),
            "2026/09/10",
            "rollout-chunks.jsonl",
            &[
                reading,
                json!({"type": "response_item", "payload": "文".repeat(CODEX_TAIL_CHUNK)}),
                token_count(json!(null), json!(null), "pro"),
            ],
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"payload\":{\"type\":\"token_count\"")
            .unwrap();
        assert_eq!(
            read_codex_rollout(&path)
                .unwrap()
                .unwrap()
                .primary
                .unwrap()
                .used_percent,
            25.0
        );
    }

    #[test]
    fn a_legacy_reading_without_a_timestamp_uses_the_file_time() {
        let home = tempfile::tempdir().unwrap();
        let mut record = token_count(json!({"used_percent": 25}), json!(null), "pro");
        record.as_object_mut().unwrap().remove("timestamp");
        write_rollout(home.path(), "2026/09/10", "rollout-legacy.jsonl", &[record]);
        assert_eq!(
            scan_codex_usage(home.path())
                .unwrap()
                .unwrap()
                .primary
                .unwrap()
                .used_percent,
            25.0
        );
    }

    #[test]
    fn files_that_are_not_rollouts_are_ignored() {
        let home = tempfile::tempdir().unwrap();
        let dir = codex_sessions_dir(home.path()).join("2026/08/23");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("notes.jsonl"),
            token_count(json!({"used_percent": 99.0}), json!(null), "pro").to_string(),
        )
        .unwrap();

        assert!(scan_codex_usage(home.path()).unwrap().is_none());
    }

    #[test]
    fn a_missing_codex_directory_is_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        assert!(scan_codex_usage(home.path()).unwrap().is_none());
    }

    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
}
