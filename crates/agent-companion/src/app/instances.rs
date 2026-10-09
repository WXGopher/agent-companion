//! Instance-scoped Windows monitoring and presentation. Secondary account state
//! is dropped immediately on disable, including pending reads and task caches.
use super::*;
use crate::tui_deployment::{self, InstanceConfig};

/// Keep account identity, allowance and task state together until they reach
/// the UI. Dodex uses Codex's hook protocol, not Codex's account or task table.
struct InstanceReadout {
    source: taskbar::ChipSource,
    snapshot: subscription::Snapshot,
    tasks: AgentTasks,
    outcomes: task_status::TaskOutcomes,
}

impl InstanceReadout {
    fn chip(&self, good: i64, warn: i64) -> taskbar::Chip {
        taskbar::Chip {
            agent: Some(self.source),
            value: self.snapshot.weekly_value(),
            tier: self
                .snapshot
                .weekly()
                .map(|quota| crate::usage_cache::left_tier(quota.0, good, warn))
                .unwrap_or(""),
            tasks: self.tasks,
            outcomes: self.outcomes,
        }
    }

    fn details(&self, now: u64, offset: i64) -> String {
        let quota = self.snapshot.weekly();
        let reset = match quota.and_then(|quota| quota.1) {
            Some(at) if at <= now => "Reset time reached · waiting for the next query".into(),
            Some(at) => crate::usage_cache::reset_label(Some(at), now, offset)
                .map(|label| format!("Week · resets {label}"))
                .unwrap_or_default(),
            None if quota.is_none()
                && self.snapshot.read_at.is_some()
                && !self.snapshot.failed() =>
            {
                "No weekly allowance reported".into()
            }
            None => "Weekly remaining".into(),
        };
        format!("{reset} · {}", self.snapshot.status(now, offset))
    }

    fn row(&self, now: u64, offset: i64, good: i64, warn: i64) -> ui::UsageRow {
        let chip = self.chip(good, warn);
        ui::UsageRow {
            heading: true,
            agent: self.source.as_str().into(),
            label: self.source.label().into(),
            value: chip.value.into(),
            tier: chip.tier.into(),
            fill: self
                .snapshot
                .weekly()
                .map(|quota| quota.0 as f32 / 100.0)
                .unwrap_or(0.0),
            resets: self.details(now, offset).into(),
        }
    }
}

fn quota_tooltip(readouts: &[InstanceReadout], now: u64, offset: i64) -> String {
    // Put both account values before the longer reset/error descriptions.
    let summary = readouts
        .iter()
        .map(|readout| {
            format!(
                "{} ({}) week {} left",
                readout.source.label(),
                readout.source.compact_label(),
                readout.snapshot.weekly_value()
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    std::iter::once(summary)
        .chain(readouts.iter().map(|readout| {
            format!(
                "{}: {}",
                readout.source.label(),
                readout.details(now, offset)
            )
        }))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn append_readout_chips(
    chips: &mut Vec<taskbar::Chip>,
    readouts: &[InstanceReadout],
    show: bool,
    good: i64,
    warn: i64,
) {
    // Replace account blocks as a unit: disabling Dodex cannot leave its last
    // reading behind, and repeated refreshes cannot duplicate either account.
    chips.retain(|chip| chip.agent == Some(taskbar::ChipSource::Claude));
    if show {
        chips.extend(readouts.iter().map(|readout| readout.chip(good, warn)));
    }
    if chips.is_empty() {
        chips.push(taskbar::Chip {
            agent: None,
            value: "--".into(),
            tier: "",
            tasks: AgentTasks::default(),
            outcomes: task_status::TaskOutcomes::default(),
        });
    }
}

pub struct Secondary {
    pub instance: InstanceConfig,
    watcher: sessions::CodexWatcher,
    table: SessionTable,
    completions: notifications::Tracker,
}

impl Secondary {
    fn new(instance: InstanceConfig) -> Self {
        Self {
            watcher: sessions::CodexWatcher::with_home(Some(instance.codex_home.clone())),
            instance,
            table: SessionTable::new(),
            completions: notifications::Tracker::default(),
        }
    }
}

impl App {
    fn instance_readouts(&self) -> Vec<InstanceReadout> {
        let mut readouts = vec![InstanceReadout {
            source: taskbar::ChipSource::Codex,
            snapshot: self.subscription_snapshot("codex"),
            tasks: self.agent_tasks(HookSource::Codex),
            outcomes: self.agent_outcomes(HookSource::Codex),
        }];
        if let Some(secondary) = self.secondary.borrow().as_ref() {
            let now = now_unix_secs();
            readouts.push(InstanceReadout {
                source: taskbar::ChipSource::Dodex,
                snapshot: self.subscription_snapshot("dodex"),
                tasks: secondary.table.tasks(HookSource::Codex, now),
                outcomes: task_status::outcomes(&secondary.table, HookSource::Codex, now),
            });
        }
        readouts
    }

    pub(super) fn instance_quota_tooltip(&self) -> String {
        let mut tooltip = quota_tooltip(
            &self.instance_readouts(),
            now_unix_secs(),
            win::local_offset_secs(),
        );
        if self.display.borrow().visible(HookSource::Claude) {
            let claude = self.usage.borrow().compact(&[HookSource::Claude]);
            if !claude.is_empty() {
                tooltip.push_str("\n\n");
                tooltip.push_str(&claude);
            }
        }
        tooltip
    }
    pub(super) fn secondary_tasks(&self) -> Option<AgentTasks> {
        self.secondary
            .borrow()
            .as_ref()
            .map(|s| s.table.tasks(HookSource::Codex, now_unix_secs()))
    }
    pub(super) fn poll_secondary(&self) {
        let active = tui_deployment::active_instance();
        let mut secondary = self.secondary.borrow_mut();
        if secondary.as_ref().map(|s| &s.instance) != active.as_ref() {
            *secondary = active.map(Secondary::new);
            if secondary.is_none() {
                self.usage_navigation.borrow_mut().remove_secondary();
                self.flyout.set_secondary_selected(false);
            }
            self.flyout.set_dual_enabled(secondary.is_some());
        }
        let Some(secondary) = secondary.as_mut() else {
            return;
        };
        secondary.watcher.poll(&mut secondary.table);
        let now = now_unix_secs();
        secondary.table.sweep(now);
        let notices = secondary.completions.observe(
            &secondary.table,
            now,
            self.config.borrow().completion_notifications,
            |_| self.flyout_open.get() && !self.flyout_peek.get(),
        );
        for mut notice in notices {
            notice.session_id = format!("dodex:{}", notice.session_id);
            notice.title = notice.title.replacen("Codex", "Dodex", 1);
            self.notifier.send(notice);
        }
    }

    pub(super) fn append_secondary_rows(&self, rows: &mut Vec<ui::SessionRow>) {
        for row in rows.iter_mut().filter(|r| r.source == "codex") {
            row.detail = format!("Codex · {}", row.detail).into();
        }
        let secondary = self.secondary.borrow();
        let Some(secondary) = secondary.as_ref() else {
            return;
        };
        rows.extend(secondary.table.sessions().map(|state| {
            let project = state.cwd.as_deref().map(project_name);
            ui::SessionRow {
                id: format!("dodex:{}", state.session_id).into(),
                title: session_title(
                    project.as_deref(),
                    state.display_name.as_deref(),
                    &state.session_id,
                )
                .into(),
                detail: format!("Dodex · {}", describe_session(state)).into(),
                phase: task_status::phase(state).into(),
                source: "dodex".into(),
                jumpable: navigation::can_jump(state, true),
            }
        }));
    }

    pub(super) fn instance_quotas(&self) -> Vec<ui::UsageRow> {
        let now = now_unix_secs();
        let offset = win::local_offset_secs();
        let (good, warn) = self.config.borrow().taskbar.thresholds();
        self.instance_readouts()
            .iter()
            .map(|readout| readout.row(now, offset, good, warn))
            .collect()
    }

    pub(super) fn append_instance_chips(
        &self,
        chips: &mut Vec<taskbar::Chip>,
        good: i64,
        warn: i64,
    ) {
        append_readout_chips(
            chips,
            &self.instance_readouts(),
            self.config.borrow().taskbar.codex,
            good,
            warn,
        );
    }

    pub(super) fn jump_secondary(self: &Rc<Self>, id: &str) {
        let secondary = self.secondary.borrow();
        let Some(secondary) = secondary.as_ref() else {
            return;
        };
        let Some(state) = secondary.table.get(id).cloned() else {
            return;
        };
        let instance = secondary.instance.clone();
        let request = self.jump_request.get().wrapping_add(1);
        self.jump_request.set(request);
        std::thread::spawn(move || {
            let plan = navigation::Plan::resolve(&state);
            let opened = tui_deployment::active_instance().as_ref() == Some(&instance)
                && ((!plan.is_desktop() && plan.activate(&state.session_id, None))
                    || tui_deployment::open_terminal(Some(&state.session_id)).is_ok());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = APP.with(|slot| slot.borrow().clone())
                    && app.jump_request.get() == request
                    && app
                        .secondary
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.instance == instance)
                    && opened
                {
                    app.close_flyout();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests;
