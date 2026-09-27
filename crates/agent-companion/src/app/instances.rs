//! Instance-scoped Windows monitoring and presentation. Secondary account state
//! is dropped immediately on disable, including pending reads and task caches.
use super::*;
use crate::windows_deployment::{self, InstanceConfig};

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
    pub(super) fn instance_quota_tooltip(&self) -> String {
        let mut items: Vec<String> = self
            .instance_quotas()
            .into_iter()
            .map(|row| format!("{} week {} · {}", row.label, row.value, row.resets))
            .collect();
        if self.display.borrow().visible(HookSource::Claude) {
            let claude = self.usage.borrow().compact(&[HookSource::Claude]);
            if !claude.is_empty() {
                items.push(claude);
            }
        }
        items.join("\n")
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
        let (good, warn) = self.config.borrow().taskbar.thresholds();
        let row = |id: &str, label: &str| {
            let snapshot = self.subscription_snapshot(id);
            let quota = snapshot.weekly();
            let reset = match quota.and_then(|q| q.1) {
                Some(at) if at <= now => "Reset time reached · waiting for the next query".into(),
                Some(at) => {
                    crate::usage_cache::reset_label(Some(at), now, win::local_offset_secs())
                        .map(|s| format!("Week · resets {s}"))
                        .unwrap_or_default()
                }
                None => "Weekly remaining".into(),
            };
            ui::UsageRow {
                heading: true,
                agent: id.into(),
                label: label.into(),
                value: quota
                    .map(|q| subscription::quota_value(q.0, snapshot.failed()))
                    .unwrap_or_else(|| "—".into())
                    .into(),
                tier: quota
                    .map(|q| crate::usage_cache::left_tier(q.0, good, warn))
                    .unwrap_or("")
                    .into(),
                fill: quota.map(|q| q.0 as f32 / 100.0).unwrap_or(0.0),
                resets: format!(
                    "{reset} · {}",
                    snapshot.status(now, win::local_offset_secs())
                )
                .into(),
            }
        };
        let mut rows = vec![row("codex", "Codex")];
        if self.secondary.borrow().is_some() {
            rows.push(row("dodex", "Dodex"));
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
            let outcomes = if is_secondary {
                task_status::outcomes(
                    &secondary.as_ref().unwrap().table,
                    HookSource::Codex,
                    now_unix_secs(),
                )
            } else {
                primary.outcomes
            };
            chips.push(taskbar::Chip {
                agent: Some(HookSource::Codex),
                value: format!(
                    "{} {}",
                    if is_secondary { "D" } else { "C" },
                    if row.tier.is_empty() {
                        "—"
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
                outcomes,
            });
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
