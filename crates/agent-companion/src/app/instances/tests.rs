use super::*;
use agent_companion_core::protocol::HookPayload;
use agent_companion_core::usage_service::{Scheduler, Source};
use serde_json::json;

use taskbar::ChipSource::{Codex, Dodex};

const NOW: u64 = 1_000;

fn source(id: &str) -> Source {
    Source {
        instance_id: id.into(),
        codex_home: format!("/fixture/{id}/home").into(),
        database_path: format!("/fixture/{id}/database").into(),
        executable_path: Some(format!("/fixture/{id}/codex.exe").into()),
    }
}

fn allowance(left: i64, resets_at: u64) -> serde_json::Value {
    json!({"rateLimits": {"limitId": "codex", "secondary": {
        "usedPercent": 100 - left, "windowDurationMins": 10080, "resetsAt": resets_at
    }}})
}

fn readout(
    scheduler: &Scheduler,
    source: taskbar::ChipSource,
    table: &SessionTable,
) -> InstanceReadout {
    InstanceReadout {
        source,
        snapshot: subscription::Snapshot::from_shared(scheduler.snapshot(source.as_str())),
        tasks: table.tasks(HookSource::Codex, NOW),
        outcomes: task_status::outcomes(table, HookSource::Codex, NOW),
    }
}

fn hook(table: &mut SessionTable, id: &str, event: &str) {
    let payload: HookPayload = serde_json::from_value(json!({
        "session_id": id, "hook_event_name": event, "tool_name": "Bash"
    }))
    .unwrap();
    table.apply(&payload, HookSource::Codex, NOW);
}

#[test]
fn both_accounts_keep_their_own_allowance_tasks_and_outcomes() {
    let mut scheduler = Scheduler::new(5);
    let queries = scheduler.sync_sources(vec![source("codex"), source("dodex")], NOW);
    for query in queries {
        let left = if query.source.instance_id == "codex" {
            72
        } else {
            13
        };
        scheduler.complete(&query, Ok(allowance(left, NOW + 600)), NOW, 10);
    }
    let mut primary = SessionTable::new();
    let mut secondary = SessionTable::new();
    // Identical session IDs must still belong to different task tables.
    hook(&mut primary, "same-id", "UserPromptSubmit");
    hook(&mut secondary, "same-id", "PermissionRequest");
    hook(&mut secondary, "failed-turn", "Stop");
    secondary.get_mut("failed-turn").unwrap().last_event = "turn_failed".into();

    let readouts = [
        readout(&scheduler, Codex, &primary),
        readout(&scheduler, Dodex, &secondary),
    ];
    let mut chips = Vec::new();
    append_readout_chips(&mut chips, &readouts, true, 50, 20);
    assert_eq!(chips.len(), 2);
    assert_eq!(
        (chips[0].agent, chips[0].value.as_str(), chips[0].tier),
        (Some(Codex), "72%", "good")
    );
    assert_eq!(
        (chips[1].agent, chips[1].value.as_str(), chips[1].tier),
        (Some(Dodex), "13%", "low")
    );
    assert_eq!(
        chips[0].tasks,
        AgentTasks {
            running: 1,
            ..Default::default()
        }
    );
    assert_eq!(chips[0].outcomes, task_status::TaskOutcomes::default());
    assert_eq!(
        chips[1].tasks,
        AgentTasks {
            pending: 1,
            done: 1,
            running: 0
        }
    );
    assert_eq!(chips[1].outcomes.failed, 1);

    for (readout, chip) in readouts.iter().zip(&chips) {
        let row = readout.row(NOW, 0, 50, 20);
        assert_eq!(row.agent, readout.source.as_str());
        assert_eq!(row.label, readout.source.label());
        assert_eq!(row.value.as_str(), chip.value);
        assert_eq!(row.tier.as_str(), chip.tier);
    }
}

#[test]
fn loading_signed_out_and_failed_refresh_states_never_borrow_the_other_account() {
    let mut scheduler = Scheduler::new(5);
    let queries = scheduler.sync_sources(vec![source("codex"), source("dodex")], NOW);
    let primary_query = queries
        .iter()
        .find(|query| query.source.instance_id == "codex")
        .unwrap();
    let secondary_query = queries
        .iter()
        .find(|query| query.source.instance_id == "dodex")
        .unwrap();
    let table = SessionTable::new();
    let primary_loading = readout(&scheduler, Codex, &table);
    let secondary_loading = readout(&scheduler, Dodex, &table);
    assert_eq!(primary_loading.chip(50, 20).value, "…");
    assert_eq!(secondary_loading.chip(50, 20).value, "…");

    scheduler.complete(primary_query, Ok(allowance(72, NOW + 600)), NOW + 1, 10);
    assert_eq!(readout(&scheduler, Codex, &table).chip(50, 20).value, "72%");
    assert_eq!(readout(&scheduler, Dodex, &table).chip(50, 20).value, "…");
    scheduler.complete(secondary_query, Err("sign in required".into()), NOW + 2, 10);
    let readouts = [
        readout(&scheduler, Codex, &table),
        readout(&scheduler, Dodex, &table),
    ];
    assert_eq!(readouts[1].chip(50, 20).value, "—");
    assert_eq!(readouts[1].chip(50, 20).tier, "");
    let tooltip = quota_tooltip(&readouts, NOW + 2, 0);
    let sections: Vec<_> = tooltip.split("\n\n").collect();
    assert_eq!(
        sections[0],
        "Codex (C) week 72% left · Dodex (D) week — left"
    );
    assert!(sections[0].encode_utf16().count() < 80);
    assert!(sections[1].starts_with("Codex:") && !sections[1].contains("sign in required"));
    assert!(sections[2].starts_with("Dodex:") && sections[2].contains("sign in required"));
    assert!(!sections[2].contains("72%"));

    let retry = scheduler.refresh("dodex", NOW + 3).remove(0);
    scheduler.complete(&retry, Ok(allowance(31, NOW + 900)), NOW + 4, 10);
    let refresh = scheduler.refresh("dodex", NOW + 5).remove(0);
    let refreshing = readout(&scheduler, Dodex, &table);
    assert_eq!(refreshing.chip(50, 20).value, "31%");
    assert!(
        refreshing
            .details(NOW + 5, 0)
            .contains("Reading allowance…")
    );
    scheduler.complete(&refresh, Err("connection unavailable".into()), NOW + 6, 10);
    assert_eq!(
        readout(&scheduler, Dodex, &table).chip(50, 20).value,
        "31%*"
    );
    assert_eq!(readout(&scheduler, Codex, &table).chip(50, 20).value, "72%");
    scheduler.sync_identity("dodex", Err("sign in required".into()), NOW + 7);
    assert_eq!(readout(&scheduler, Dodex, &table).chip(50, 20).value, "—");
    assert_eq!(readout(&scheduler, Codex, &table).chip(50, 20).value, "72%");
}

#[test]
fn instance_visibility_follows_configuration_and_secondary_availability() {
    let table = SessionTable::new();
    let mut scheduler = Scheduler::new(5);
    scheduler.sync_sources(vec![source("codex"), source("dodex")], NOW);
    let both = [
        readout(&scheduler, Codex, &table),
        readout(&scheduler, Dodex, &table),
    ];
    let mut chips = Vec::new();
    append_readout_chips(&mut chips, &both, true, 50, 20);
    assert_eq!(
        chips
            .iter()
            .filter_map(|chip| chip.agent)
            .collect::<Vec<_>>(),
        [Codex, Dodex]
    );
    chips.clear();
    append_readout_chips(&mut chips, &both[..1], true, 50, 20);
    assert_eq!(
        chips
            .iter()
            .filter_map(|chip| chip.agent)
            .collect::<Vec<_>>(),
        [Codex]
    );
    chips.clear();
    append_readout_chips(&mut chips, &both, false, 50, 20);
    assert_eq!(chips.len(), 1);
    assert!(chips[0].agent.is_none());
}
