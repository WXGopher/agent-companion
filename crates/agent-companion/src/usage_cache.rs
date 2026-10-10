//! Local Codex quota readings and shared percentage/reset formatting.
use agent_companion_core::usage::{self, CodexUsage};

pub const REFRESH_SECS: u64 = 30;
pub const LEFT_COMFORTABLE: i64 = 50;
pub const LEFT_TIGHT: i64 = 20;

/// Green at `good_at`, amber at `warn_at`, red below.
pub fn left_tier(left: i64, good_at: i64, warn_at: i64) -> &'static str {
    if left >= good_at {
        "good"
    } else if left >= warn_at {
        "warn"
    } else {
        "low"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateWindow {
    pub label: String,
    pub left: i64,
    pub resets_at: Option<u64>,
}

/// Diagnostic readings from local logs; the GUI uses the account usage service.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub codex: Option<CodexUsage>,
    pub refreshed_at: Option<u64>,
}

impl UsageSnapshot {
    pub fn read(now: u64) -> Self {
        Self {
            codex: agent_companion_core::install::codex_home()
                .ok()
                .and_then(|home| usage::scan_codex_usage_at(&home).ok())
                .flatten(),
            refreshed_at: Some(now),
        }
    }

    pub fn refreshed(&mut self, now: u64) -> Self {
        if self
            .refreshed_at
            .is_none_or(|at| now.saturating_sub(at) >= REFRESH_SECS)
        {
            *self = Self::read(now);
        }
        self.clone()
    }

    pub fn windows(&self) -> Vec<RateWindow> {
        let windows = match &self.codex {
            Some(codex) => [codex.primary, codex.secondary],
            None => [None, None],
        };
        windows
            .into_iter()
            .enumerate()
            .filter_map(|(index, window)| {
                let window = window?;
                let label = match window.window_minutes {
                    Some(minutes) if minutes >= 1440 => "Week".to_string(),
                    Some(_) => window_label(window.window_minutes),
                    None => ["5h", "Week"][index].to_string(),
                };
                Some(RateWindow {
                    label,
                    left: remaining(window.used_percent),
                    resets_at: window.resets_at,
                })
            })
            .collect()
    }

    pub fn usage_summary(&self) -> String {
        self.windows()
            .iter()
            .map(|window| format!("{} {}%", window.label, window.left))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    pub fn detail_lines(&self) -> Vec<String> {
        let line = self.usage_summary();
        if line.is_empty() {
            Vec::new()
        } else {
            vec![format!("codex {}", line.replace(" · ", " "))]
        }
    }
}

/// Remaining allowance, clamped to the display range.
pub fn remaining(used_percent: f64) -> i64 {
    (100.0 - used_percent).round().clamp(0.0, 100.0) as i64
}

/// `HH:MM` in local time, for a Unix timestamp and a UTC offset in seconds.
pub fn local_clock(unix: u64, offset_secs: i64) -> String {
    let local = (unix as i64 + offset_secs).rem_euclid(86_400);
    format!("{:02}:{:02}", local / 3_600, (local % 3_600) / 60)
}

/// How to say when a window rolls over, or `None` for one that already has.
///
/// A clock time is unambiguous within the day, and the seven-day window is the
/// only one that ever reaches past it — so anything further out gets a weekday
/// in front. A full date would cost more room in the panel than it buys.
pub fn reset_label(resets_at: Option<u64>, now: u64, offset_secs: i64) -> Option<String> {
    let at = resets_at?;
    if at <= now {
        return None;
    }
    let clock = local_clock(at, offset_secs);
    if at - now < 86_400 {
        return Some(clock);
    }
    Some(format!("{} {clock}", weekday(at, offset_secs)))
}

/// The three-letter local weekday of a Unix timestamp.
///
/// 1970-01-01 was a Thursday, which is where the `+ 4` comes from.
fn weekday(unix: u64, offset_secs: i64) -> &'static str {
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let days = (unix as i64 + offset_secs).div_euclid(86_400);
    NAMES[(days + 4).rem_euclid(7) as usize]
}

/// Codex reports its own window length, and it is not always exactly 5 h / 7 d —
/// real rollout files carry 299, 300, 10 079, and 10 080 minutes. Round to the
/// nearest familiar name rather than claiming a precision we do not have.
pub fn window_label(window_minutes: Option<u64>) -> String {
    match window_minutes {
        Some(minutes) if minutes >= 1_440 => format!("{}d", (minutes as f64 / 1_440.0).round()),
        Some(minutes) if minutes >= 60 => format!("{}h", (minutes as f64 / 60.0).round()),
        Some(minutes) => format!("{minutes}m"),
        None => "usage".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_companion_core::usage::WindowUsage;
    const NOW: u64 = 1_787_000_000;

    #[test]
    fn quota_windows_preserve_missing_data_and_report_remaining_allowance() {
        assert!(UsageSnapshot::default().windows().is_empty());
        assert!(UsageSnapshot::default().detail_lines().is_empty());
        let snapshot = UsageSnapshot {
            codex: Some(CodexUsage {
                primary: None,
                secondary: Some(WindowUsage {
                    used_percent: 58.0,
                    resets_at: Some(NOW - 60),
                    window_minutes: Some(10080),
                }),
                ..Default::default()
            }),
            refreshed_at: Some(NOW),
        };
        assert_eq!(snapshot.windows().len(), 1);
        assert_eq!(snapshot.detail_lines(), ["codex Week 42%"]);
        assert_eq!(reset_label(snapshot.windows()[0].resets_at, NOW, 0), None);
    }

    #[test]
    fn the_percentage_colour_turns_at_the_thresholds() {
        let tier = |left| left_tier(left, LEFT_COMFORTABLE, LEFT_TIGHT);
        assert_eq!(tier(100), "good");
        assert_eq!(tier(LEFT_COMFORTABLE), "good");
        assert_eq!(tier(LEFT_COMFORTABLE - 1), "warn");
        assert_eq!(tier(LEFT_TIGHT), "warn");
        assert_eq!(tier(LEFT_TIGHT - 1), "low");
        assert_eq!(tier(0), "low");
        // Nothing outside 0–100 reaches here, but nothing panics if it does.
        assert_eq!(tier(-5), "low");
        assert_eq!(tier(400), "good");

        // Moved thresholds move the turns with them.
        assert_eq!(left_tier(69, 70, 30), "warn");
        assert_eq!(left_tier(70, 70, 30), "good");
        assert_eq!(left_tier(29, 70, 30), "low");
    }

    #[test]
    fn percentages_are_shown_as_what_is_left_not_what_is_spent() {
        assert_eq!(remaining(8.0), 92);
        assert_eq!(remaining(0.0), 100);
        assert_eq!(remaining(100.0), 0);
        assert_eq!(remaining(31.4), 69);
        // A window can report past its own limit; nobody has -7 % left.
        assert_eq!(remaining(107.0), 0);
        assert_eq!(remaining(-3.0), 100);
    }

    #[test]
    fn a_reset_within_the_day_is_a_clock_and_beyond_it_a_weekday() {
        // 1970-01-01 was a Thursday, which is what anchors the weekday maths.
        assert_eq!(weekday(0, 0), "Thu");
        assert_eq!(weekday(86_400, 0), "Fri");
        assert_eq!(weekday(6 * 86_400, 0), "Wed");

        assert_eq!(reset_label(Some(NOW + 3_600), NOW, 0).unwrap().len(), 5);
        let far = reset_label(Some(NOW + 3 * 86_400), NOW, 0).unwrap();
        assert_eq!(far.split(' ').count(), 2, "got {far}");
        assert_eq!(far.split(' ').next().unwrap().len(), 3);

        // Exactly a day out already needs the weekday, and the past gets none.
        assert!(
            reset_label(Some(NOW + 86_400), NOW, 0)
                .unwrap()
                .contains(' ')
        );
        assert_eq!(reset_label(Some(NOW - 1), NOW, 0), None);
        assert_eq!(reset_label(None, NOW, 0), None);
    }

    #[test]
    fn the_local_clock_wraps_the_day_in_both_directions() {
        // Midnight UTC — the constant is already a whole number of days.
        let midnight = 1_787_011_200u64;
        assert_eq!(local_clock(midnight, 0), "00:00");
        assert_eq!(local_clock(midnight, 8 * 3_600), "08:00");
        // Eight hours before midnight UTC is 16:00 the previous local day.
        assert_eq!(local_clock(midnight, -8 * 3_600), "16:00");
        assert_eq!(local_clock(midnight + 3_600 + 1_800, 0), "01:30");
    }

    #[test]
    fn codex_window_labels_round_to_familiar_names() {
        // Real rollout files carry all four of these.
        assert_eq!(window_label(Some(299)), "5h");
        assert_eq!(window_label(Some(300)), "5h");
        assert_eq!(window_label(Some(10_079)), "7d");
        assert_eq!(window_label(Some(10_080)), "7d");
        assert_eq!(window_label(Some(30)), "30m");
        assert_eq!(window_label(None), "usage");
    }
}
