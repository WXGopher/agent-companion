//! Windows presentation of the shared account-query snapshots.
//! Scheduling, processes, cancellation and caching belong to `usage_service`.
use std::collections::BTreeMap;

use agent_companion_core::usage_service::{InstanceSnapshot, Source};
use serde::Deserialize;

use super::ui;
use crate::usage_cache::{left_tier, reset_label, window_label};

/// Navigation decides only when history is entered. It cannot schedule an
/// allowance query; the four allowance events call the shared service directly.
#[derive(Default)]
pub struct Navigation {
    usage: bool,
    secondary: bool,
    history_source: Option<Source>,
}

impl Navigation {
    pub fn select_page(&mut self, usage: bool) -> bool {
        let entered = usage && !self.usage;
        self.usage = usage;
        entered
    }

    pub fn select_instance(&mut self, secondary: bool) -> bool {
        let entered = !self.usage || self.secondary != secondary;
        self.usage = true;
        self.secondary = secondary;
        entered
    }

    pub fn secondary(&self) -> bool {
        self.secondary
    }

    pub fn remove_secondary(&mut self) {
        self.secondary = false;
    }

    pub fn history_entered(&mut self, source: Option<Source>) {
        self.history_source = source;
    }

    /// Replacing a source behind the visible page is a source-switch boundary,
    /// including a late registration or falling back after Dodex is disabled.
    /// An unchanged identity never asks the history cache about its age.
    pub fn visible_history_source_changed(
        &mut self,
        source: Option<Source>,
        visible: bool,
    ) -> bool {
        if !visible {
            return false;
        }
        let changed = self.history_source != source;
        self.history_source = source;
        changed && self.history_source.is_some()
    }
}

pub fn opens_full_panel(open: bool, was_preview: bool, preview: bool) -> bool {
    !preview && (!open || was_preview)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub lifetime_tokens: Option<i64>,
    pub peak_daily_tokens: Option<i64>,
    pub longest_running_turn_sec: Option<i64>,
    pub current_streak_days: Option<i64>,
    pub longest_streak_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub start_date: String,
    pub tokens: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub summary: Summary,
    pub daily_usage_buckets: Option<Vec<Day>>,
}

impl Tokens {
    pub fn recent_days(&self) -> Vec<&Day> {
        let mut days: Vec<_> = self
            .daily_usage_buckets
            .iter()
            .flatten()
            .filter(|day| day.tokens >= 0)
            .collect();
        days.sort_by(|left, right| right.start_date.cmp(&left.start_date));
        days.truncate(7);
        days
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    used_percent: f64,
    window_duration_mins: Option<u64>,
    resets_at: Option<u64>,
}

impl Window {
    fn remaining(&self) -> Option<i64> {
        if !self.used_percent.is_finite() {
            return None;
        }
        Some(100 - self.used_percent.clamp(0.0, 100.0).round() as i64)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<Window>,
    secondary: Option<Window>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    rate_limits: Bucket,
    rate_limits_by_limit_id: Option<BTreeMap<String, Bucket>>,
}

impl Limits {
    pub fn weekly(&self) -> Option<(i64, Option<u64>)> {
        let bucket = match self
            .rate_limits_by_limit_id
            .as_ref()
            .filter(|buckets| !buckets.is_empty())
        {
            Some(buckets) => buckets.get("codex")?,
            None => self
                .rate_limits
                .limit_id
                .as_deref()
                .is_none_or(|id| id == "codex")
                .then_some(&self.rate_limits)?,
        };
        let window = [&bucket.secondary, &bucket.primary]
            .into_iter()
            .flatten()
            .find(|w| w.window_duration_mins == Some(10_080))
            .or_else(|| {
                bucket
                    .secondary
                    .as_ref()
                    .filter(|w| w.window_duration_mins.is_none())
            })?;
        Some((window.remaining()?, window.resets_at))
    }

    pub fn rows(
        &self,
        now: u64,
        offset: i64,
        good: i64,
        warn: i64,
        failed: bool,
    ) -> Vec<ui::UsageRow> {
        let mut buckets: Vec<_> = self
            .rate_limits_by_limit_id
            .as_ref()
            .map(|buckets| {
                buckets
                    .iter()
                    .map(|(id, bucket)| (id.as_str(), bucket))
                    .collect()
            })
            .unwrap_or_default();
        if buckets.is_empty() {
            buckets.push((
                self.rate_limits.limit_id.as_deref().unwrap_or("codex"),
                &self.rate_limits,
            ));
        }
        buckets.sort_by_key(|(id, _)| (*id != "codex", *id));
        let mut rows = Vec::new();
        for (id, bucket) in buckets {
            let mut title = bucket
                .limit_name
                .as_deref()
                .unwrap_or(if id == "codex" { "Codex" } else { id })
                .to_owned();
            if let Some(plan) = &bucket.plan_type
                && plan != "unknown"
            {
                title.push_str(&format!(" · {}", plan.replace('_', " ")));
            }
            rows.push(ui::UsageRow {
                heading: true,
                agent: "codex".into(),
                label: title.into(),
                ..Default::default()
            });
            for (window, fallback) in [
                (&bucket.primary, "Primary"),
                (&bucket.secondary, "Secondary"),
            ] {
                let Some(window) = window else { continue };
                let left = window.remaining();
                rows.push(ui::UsageRow {
                    label: if window.window_duration_mins.unwrap_or(0) == 0 {
                        fallback.to_owned()
                    } else {
                        window_label(window.window_duration_mins)
                    }
                    .into(),
                    value: left
                        .map(|left| quota_value(left, failed))
                        .unwrap_or_else(|| "—".into())
                        .into(),
                    tier: left
                        .map(|left| left_tier(left, good, warn))
                        .unwrap_or("")
                        .into(),
                    fill: left.unwrap_or(0) as f32 / 100.0,
                    resets: if window.resets_at.is_some_and(|at| at <= now) {
                        "Reset time reached · waiting for the next query".into()
                    } else {
                        reset_label(window.resets_at, now, offset)
                            .map(|label| format!("Resets {label}"))
                            .unwrap_or_default()
                            .into()
                    },
                    ..Default::default()
                });
            }
            if bucket.primary.is_none() && bucket.secondary.is_none() {
                rows.push(ui::UsageRow {
                    label: "No allowance window reported".into(),
                    value: "—".into(),
                    ..Default::default()
                });
            }
        }
        rows
    }
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub tokens: Option<Tokens>,
    pub limits: Option<Limits>,
    pub token_error: String,
    pub limits_error: String,
    pub read_at: Option<u64>,
    pub loading: bool,
    pub history_loading: bool,
}

impl Snapshot {
    pub fn from_shared(snapshot: Option<InstanceSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self::default();
        };
        Self {
            tokens: snapshot
                .history
                .value
                .and_then(|value| serde_json::from_value(value).ok()),
            limits: snapshot
                .limits
                .value
                .and_then(|value| serde_json::from_value(value).ok()),
            token_error: snapshot.history.error.unwrap_or_default(),
            limits_error: snapshot.limits.error.unwrap_or_default(),
            read_at: snapshot.limits.last_success_at,
            loading: snapshot.limits.loading,
            history_loading: snapshot.history.loading,
        }
    }

    pub fn weekly(&self) -> Option<(i64, Option<u64>)> {
        self.limits.as_ref()?.weekly()
    }

    /// Retain a successful reading during refresh. Before the first reading,
    /// distinguish an in-flight query from an unavailable account/window.
    pub fn weekly_value(&self) -> String {
        self.weekly()
            .map(|quota| quota_value(quota.0, self.failed()))
            .unwrap_or_else(|| if self.loading { "…" } else { "—" }.into())
    }

    pub fn failed(&self) -> bool {
        !self.limits_error.is_empty()
    }

    pub fn status(&self, now: u64, offset: i64) -> String {
        let mut status = self
            .read_at
            .map(|at| last_success_label(at, now, offset))
            .unwrap_or_else(|| "No successful query yet".into());
        if self.failed() {
            status.push_str(" · ");
            status.push_str(&self.limits_error);
        }
        if self.loading {
            status.push_str(" · Reading allowance…");
        }
        status
    }
}

pub fn quota_value(left: i64, failed: bool) -> String {
    format!("{left}%{}", if failed { "*" } else { "" })
}

pub fn last_success_label(at: u64, now: u64, offset: i64) -> String {
    let clock = crate::usage_cache::local_clock(at, offset);
    let days = now.saturating_sub(at) / 86_400;
    if days == 0 {
        format!("Last success {clock}")
    } else {
        format!("Last success {clock} · {days}d ago")
    }
}

pub fn number(value: Option<i64>) -> String {
    let Some(value) = value.filter(|value| *value >= 0) else {
        return "—".into();
    };
    for (divisor, suffix) in [
        (1_000_000_000_000.0, "T"),
        (1_000_000_000.0, "B"),
        (1_000_000.0, "M"),
        (1_000.0, "K"),
    ] {
        if value as f64 >= divisor {
            let number = format!("{:.1}", value as f64 / divisor);
            return format!("{}{suffix}", number.strip_suffix(".0").unwrap_or(&number));
        }
    }
    value.to_string()
}

pub fn duration(value: Option<i64>) -> String {
    match value.filter(|value| *value >= 0) {
        Some(seconds) if seconds >= 3600 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        Some(seconds) if seconds >= 60 => format!("{}m {}s", seconds / 60, seconds % 60),
        Some(seconds) => format!("{seconds}s"),
        None => "—".into(),
    }
}

pub fn day_count(value: Option<i64>) -> String {
    match value.filter(|value| *value >= 0) {
        Some(1) => "1 day".into(),
        Some(days) => format!("{days} days"),
        None => "—".into(),
    }
}

#[cfg(test)]
#[path = "subscription/tests.rs"]
mod tests;
