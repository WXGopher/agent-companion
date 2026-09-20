//! Instance-scoped Windows monitoring and presentation. Secondary account state
//! is dropped immediately on disable, including pending reads and task caches.
use super::*;
use crate::windows_deployment::{self, InstanceConfig};
use agent_companion_core::usage::CodexUsage;

pub struct Secondary {
    pub instance: InstanceConfig,
    pub subscription: subscription::Monitor,
    watcher: sessions::CodexWatcher,
    table: SessionTable,
    usage: Option<CodexUsage>,
    completions: notifications::Tracker,
}

impl Secondary {
    fn new(instance: InstanceConfig) -> Self {
        let mut subscription = subscription::Monitor::default();
        subscription.set_executable(Some(instance.cli_path.clone()));
        Self {
            watcher: sessions::CodexWatcher::with_home(Some(instance.codex_home.clone())),
            instance,
            subscription,
            table: SessionTable::new(),
            usage: None,
            completions: notifications::Tracker::default(),
        }
    }
}

pub fn weekly(
    local: Option<&CodexUsage>,
    monitor: &subscription::Monitor,
    now: u64,
) -> Option<(i64, Option<u64>)> {
    monitor.weekly(now).or_else(|| {
        let usage = local?;
        let window = [usage.secondary, usage.primary]
            .into_iter()
            .flatten()
            .find(|w| w.window_minutes == Some(10_080))
            .or_else(|| usage.secondary.filter(|w| w.window_minutes.is_none()))?;
        if !window.used_percent.is_finite() || window.resets_at.is_some_and(|at| at <= now) {
            return None;
        }
        Some((
            crate::usage_cache::remaining(window.used_percent),
            window.resets_at,
        ))
    })
}

impl App {
    pub(super) fn instance_quota_tooltip(&self) -> String {
        let mut items: Vec<String> = self
            .instance_quotas()
            .into_iter()
            .filter(|row| !row.tier.is_empty())
            .map(|row| format!("{} week {}", row.label, row.value))
            .collect();
        if self.display.borrow().visible(HookSource::Claude) {
            let claude = self.usage.borrow().compact(&[HookSource::Claude]);
            if !claude.is_empty() {
                items.push(claude);
            }
        }
        items.join(" · ")
    }
    pub(super) fn secondary_tasks(&self) -> Option<AgentTasks> {
        self.secondary
            .borrow()
            .as_ref()
            .map(|s| s.table.tasks(HookSource::Codex, now_unix_secs()))
    }
    pub(super) fn poll_secondary(&self) {
        let active = windows_deployment::active_instance();
        let mut secondary = self.secondary.borrow_mut();
        if secondary.as_ref().map(|s| &s.instance) != active.as_ref() {
            *secondary = active.map(Secondary::new);
            if secondary.is_none() {
                self.selected_subscription.set(false);
                self.flyout.set_secondary_selected(false);
            }
            self.flyout.set_dual_enabled(secondary.is_some());
        }
        let Some(secondary) = secondary.as_mut() else {
            return;
        };
        let update = secondary.watcher.poll(&mut secondary.table);
        if let Some(usage) = update.usage {
            secondary.usage = Some(usage);
        }
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

    pub(super) fn stop_subscriptions(&self) {
        self.subscription.borrow_mut().stop();
        if let Some(secondary) = self.secondary.borrow_mut().as_mut() {
            secondary.subscription.stop();
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
                phase: state.phase.as_str().into(),
                source: "dodex".into(),
                jumpable: navigation::can_jump(state, true),
            }
        }));
    }

    pub(super) fn instance_quotas(&self) -> Vec<ui::UsageRow> {
        let now = now_unix_secs();
        let (good, warn) = self.config.borrow().taskbar.thresholds();
        let row = |id: &str, label: &str, quota: Option<(i64, Option<u64>)>| ui::UsageRow {
            heading: true,
            agent: id.into(),
            label: label.into(),
            value: quota
                .map(|q| format!("{}%", q.0))
                .unwrap_or_else(|| "—".into())
                .into(),
            tier: quota
                .map(|q| crate::usage_cache::left_tier(q.0, good, warn))
                .unwrap_or("")
                .into(),
            fill: quota.map(|q| q.0 as f32 / 100.0).unwrap_or(0.0),
            resets: quota
                .and_then(|q| crate::usage_cache::reset_label(q.1, now, win::local_offset_secs()))
                .map(|s| format!("Week · resets {s}"))
                .unwrap_or_else(|| "Weekly remaining".into())
                .into(),
        };
        let mut rows = vec![row(
            "codex",
            "Codex",
            weekly(
                self.usage.borrow().codex.as_ref(),
                &self.subscription.borrow(),
                now,
            ),
        )];
        if let Some(secondary) = self.secondary.borrow().as_ref() {
            rows.push(row(
                "dodex",
                "Dodex",
                weekly(secondary.usage.as_ref(), &secondary.subscription, now),
            ));
        }
        rows
    }

    pub(super) fn append_instance_chips(
        &self,
        chips: &mut Vec<taskbar::Chip>,
        primary: taskbar::AgentLine,
        good: i64,
        warn: i64,
    ) {
        chips.retain(|chip| chip.agent.is_some());
        let quotas = self.instance_quotas();
        let secondary = self.secondary.borrow();
        for row in quotas {
            let is_secondary = row.agent == "dodex";
            if !self.config.borrow().taskbar.codex || (is_secondary && secondary.is_none()) {
                continue;
            }
            let tasks = if is_secondary {
                secondary
                    .as_ref()
                    .unwrap()
                    .table
                    .tasks(HookSource::Codex, now_unix_secs())
            } else {
                primary.tasks
            };
            if row.tier.is_empty() && tasks.total() == 0 {
                continue;
            }
            chips.push(taskbar::Chip {
                agent: Some(HookSource::Codex),
                value: format!(
                    "{} {}",
                    if is_secondary { "D" } else { "C" },
                    if row.tier.is_empty() {
                        "--"
                    } else {
                        row.value.as_str()
                    }
                ),
                tier: if row.tier.is_empty() {
                    ""
                } else {
                    crate::usage_cache::left_tier((row.fill * 100.0).round() as i64, good, warn)
                },
                tasks,
            });
        }
        if chips.is_empty() {
            chips.push(taskbar::Chip {
                agent: None,
                value: "--".into(),
                tier: "",
                tasks: AgentTasks::default(),
            });
        }
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
            // Launching the explicit runtime with its data directory delivers the
            // deep link to that instance's own single-instance handler.
            let opened = if plan.is_desktop() {
                windows_deployment::active_instance().as_ref() == Some(&instance)
                    && windows_deployment::launch(Some(&state.session_id)).is_ok()
            } else {
                false
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = APP.with(|slot| slot.borrow().clone())
                    && app.jump_request.get() == request
                    && app
                        .secondary
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.instance == instance)
                    && (opened || (!plan.is_desktop() && plan.activate(&state.session_id, None)))
                {
                    app.close_flyout();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_local_quota_expires_and_other_windows_do_not_replace_it() {
        let mut usage = agent_companion_core::usage::parse_codex_rate_limits(&serde_json::json!({
            "primary":{"used_percent":95,"window_minutes":300,"resets_at":2000},
            "secondary":{"used_percent":25,"window_minutes":10080,"resets_at":2000}
        }));
        let monitor = subscription::Monitor::default();
        assert_eq!(weekly(Some(&usage), &monitor, 1000), Some((75, Some(2000))));
        assert!(weekly(Some(&usage), &monitor, 2000).is_none());
        usage.secondary = None;
        assert!(weekly(Some(&usage), &monitor, 1000).is_none());
    }
}
