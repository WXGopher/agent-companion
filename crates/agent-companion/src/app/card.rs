//! Codex permission cards and native app-server question forms.
use super::cardview::CardKind;
use crate::util::{project_name, truncate};
use agent_companion_core::protocol::{HookDecision, HookPayload, events};
use agent_companion_core::state::correlation_key;

pub const HOVER_DWELL_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub kind: CardKind,
    pub form: Option<super::form::Form>,
    pub session_id: String,
    pub key: String,
    pub event: String,
    pub title: String,
    pub tool: String,
    pub detail: String,
}

impl Card {
    pub fn for_request(payload: &HookPayload) -> Option<Self> {
        let (kind, form, tool, detail) = match payload.event_name() {
            events::CODEX_USER_INPUT => {
                let request = serde_json::from_value(payload.tool_input.clone()?).ok()?;
                let form = super::form::Form::new(request)?;
                if payload.session_id.as_deref() != Some(form.request.thread_id.as_str()) {
                    return None;
                }
                (CardKind::Form, Some(form), "Question".into(), String::new())
            }
            events::PERMISSION_REQUEST => (
                CardKind::Approval,
                None,
                payload.tool_name.clone().unwrap_or_else(|| "?".into()),
                crate::headless::summarize_input(payload),
            ),
            _ => return None,
        };
        Some(Self {
            kind,
            form,
            session_id: payload.session_id.clone()?,
            key: correlation_key(payload),
            event: payload.event_name().into(),
            title: title_for(payload),
            tool,
            detail,
        })
    }

    pub fn decision(&self, allow: bool) -> Option<HookDecision> {
        let reason = Some(
            if allow {
                "approved in Agent Companion"
            } else {
                "denied in Agent Companion"
            }
            .to_string(),
        );
        if allow {
            HookDecision::allow_for(&self.event, reason)
        } else {
            HookDecision::deny_for(&self.event, reason)
        }
    }
}

fn title_for(payload: &HookPayload) -> String {
    let name = payload
        .cwd
        .as_deref()
        .map(project_name)
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => truncate(&name, 28),
        None => payload
            .session_id
            .as_deref()
            .map(|id| id.chars().take(8).collect())
            .unwrap_or_else(|| "session".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn payload(value: serde_json::Value) -> HookPayload {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn permission_cards_render_input_and_send_native_decisions() {
        let request = payload(
            json!({"hook_event_name":"PermissionRequest","session_id":"s1","cwd":"C:/synthetic/project","tool_name":"Bash","tool_use_id":"tu1","tool_input":{"command":"git status"}}),
        );
        let card = Card::for_request(&request).unwrap();
        assert_eq!(card.kind, CardKind::Approval);
        assert_eq!(card.title, "project");
        assert_eq!(card.detail, "git status");
        assert_eq!(card.key, "tu1");
        let allow: serde_json::Value =
            serde_json::from_str(&card.decision(true).unwrap().to_stdout_json()).unwrap();
        assert_eq!(
            allow["hookSpecificOutput"]["decision"],
            json!({"behavior":"allow"})
        );
        let deny: serde_json::Value =
            serde_json::from_str(&card.decision(false).unwrap().to_stdout_json()).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
    }
    #[test]
    fn activity_never_raises_permission_cards() {
        for event in [
            events::PRE_TOOL_USE,
            events::STOP,
            events::SESSION_START,
            events::POST_TOOL_USE,
        ] {
            let request = payload(json!({"hook_event_name":event,"session_id":"s1"}));
            assert!(Card::for_request(&request).is_none());
        }
    }
    #[test]
    fn native_question_form_requires_the_matching_thread() {
        let mut request = payload(
            json!({"hook_event_name":events::CODEX_USER_INPUT,"session_id":"s1","tool_input":{
                "threadId":"s1","turnId":"turn1","itemId":"item1","questions":[{"id":"q1","header":"Name","question":"Project name?","options":null,"isOther":false,"isSecret":false}]
            }}),
        );
        let card = Card::for_request(&request).unwrap();
        assert_eq!(card.kind, CardKind::Form);
        assert!(card.decision(true).is_none());
        request.session_id = Some("other".into());
        assert!(Card::for_request(&request).is_none());
    }
}
