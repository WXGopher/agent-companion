//! Wire format shared between the hook binary and the Agent Companion app: hook payloads, approval requests, and decisions.
//!
//! # Transport
//!
//! Newline-delimited JSON over the named pipe `\\.\pipe\atoll` (see
//! [`crate::pipe`]). Every line is one [`Envelope`]. There is no request id:
//! one connection carries one request, and the reply — if the event needs one —
//! comes back on the same connection.
//!
//! # Hook payload passthrough
//!
//! Hook payloads are forwarded *verbatim*. [`HookPayload`] strongly types only
//! the fields Agent Companion actually reads and captures everything else in
//! [`HookPayload::extra`], so unknown or future fields survive a round trip.
//!
//! # Decision output
//!
//! Decisions the hook prints on stdout are built as [`serde_json::Value`]
//! objects rather than structs. `serde_json::Map` is a `BTreeMap` by default,
//! so this gives byte-stable, key-sorted output; the unit tests below lock the
//! exact bytes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Which agent produced a hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookSource {
    Codex,
}

impl HookSource {
    pub fn as_str(self) -> &'static str {
        match self {
            HookSource::Codex => "codex",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(HookSource::Codex),
            _ => None,
        }
    }
}

/// Top-level frame. One per line on the pipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Envelope {
    /// Sent by a peer right after connecting to identify itself.
    Hello { hello: Hello },
    /// A fire-and-forget notification that needs no reply.
    Event { event: Event },
    /// A request. The sender keeps the connection open if it wants a response.
    Command { command: Command },
    /// A reply to a [`Envelope::Command`] on the same connection.
    Response { response: Response },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// The app is going away; connected peers should stop waiting.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Command {
    /// A hook fired. `hook` is the agent's stdin JSON plus Agent Companion's
    /// injected terminal metadata.
    // Keep the historical Codex wire names so new launchers also work with an
    // older running Companion. Only Codex sources are accepted.
    #[serde(rename = "processClaudeHook", alias = "processHook")]
    ProcessHook {
        #[serde(rename = "claudeHook", alias = "hook")]
        hook: HookPayload,
        source: HookSource,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    /// Received, nothing further expected.
    Ack,
    /// A native Codex app-server answer, never a hook permission override.
    CodexInput { answers: crate::questions::Answers },
    /// The user's (or the app's) answer to a blocking hook.
    Decision { decision: HookDecision },
    /// The app could not produce a decision; the hook fails open.
    Error { message: String },
}

/// Key under which Agent Companion injects terminal metadata into a hook payload.
///
/// Nested under a single namespaced key so the passthrough payload keeps
/// exactly the agent's own top-level shape plus one clearly-ours addition.
pub const TERMINAL_META_KEY: &str = "atollTerminal";

/// Environment variables the hook forwards, when set, so the app can jump back
/// to the terminal or editor that owns the session.
pub const TERMINAL_ENV_VARS: &[&str] = &[
    "AGENT_COMPANION_TERMINAL_TARGET",
    "ATOLL_TERMINAL_TARGET",
    "ConEmuPID",
    "SESSIONNAME",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "VSCODE_GIT_ASKPASS_MAIN",
    "VSCODE_GIT_IPC_HANDLE",
    "VSCODE_INJECTION",
    "VSCODE_PID",
    "WT_PROFILE_ID",
    "WT_SESSION",
];

/// A hook's stdin payload: typed where Agent Companion reads it, verbatim everywhere else.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_event_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Every other key, preserved as-is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl HookPayload {
    /// The event name, or `""` when the agent did not send one.
    pub fn event_name(&self) -> &str {
        self.hook_event_name.as_deref().unwrap_or_default()
    }

    /// Whether this event makes the agent wait for a decision on stdout.
    pub fn is_blocking(&self) -> bool {
        matches!(
            self.event_name(),
            events::PRE_TOOL_USE | events::PERMISSION_REQUEST
        )
    }

    /// Attach terminal metadata under [`TERMINAL_META_KEY`].
    pub fn set_terminal_meta(&mut self, meta: TerminalMeta) {
        let value = serde_json::to_value(meta).unwrap_or(Value::Null);
        self.extra.insert(TERMINAL_META_KEY.to_string(), value);
    }

    /// Read back terminal metadata, if the hook injected any.
    pub fn terminal_meta(&self) -> Option<TerminalMeta> {
        let raw = self.extra.get(TERMINAL_META_KEY)?;
        serde_json::from_value(raw.clone()).ok()
    }
}

/// One process in the hook's ancestry, captured while the chain was alive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRef {
    pub pid: u32,
    /// Executable file name only, lowercased: `"windowsterminal.exe"`.
    pub exe: String,
}

/// Where the session lives, as seen from inside the hook process.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalMeta {
    /// The subset of [`TERMINAL_ENV_VARS`] that was actually set.
    pub env: Map<String, Value>,
    /// PID of the hook process. Useless by the time anyone clicks — the hook
    /// exits within milliseconds — but kept on the wire for diagnostics.
    pub hook_pid: u32,
    /// The hook's ancestry, nearest first: the transient shell the agent
    /// spawned the hook through, the agent CLI, the user's shell, the
    /// terminal host. Captured at event time because that is the one moment
    /// every link is certainly alive — the hook's own parent is typically a
    /// `cmd.exe` that dies milliseconds later, so a click resolves against
    /// this list rather than against the process tree of the past. Ends
    /// before `explorer.exe`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<ProcessRef>,
}

/// Hook event names Agent Companion knows about.
pub mod events {
    // Keep the wire event compatible with already running Atoll launchers.
    pub const CODEX_USER_INPUT: &str = "AtollCodexUserInput";
    pub const SESSION_START: &str = "SessionStart";
    pub const SESSION_END: &str = "SessionEnd";
    pub const USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";
    pub const STOP: &str = "Stop";
    pub const INTERRUPT: &str = "Interrupt";
    pub const NOTIFICATION: &str = "Notification";
    pub const PRE_TOOL_USE: &str = "PreToolUse";
    pub const POST_TOOL_USE: &str = "PostToolUse";
    pub const PERMISSION_REQUEST: &str = "PermissionRequest";
}

/// How long a hook blocks waiting for a decision before failing open.
pub mod timeouts {
    use std::time::Duration;

    /// Budget for opening the pipe. Generous enough to ride out the app being
    /// busy, short enough that a missing app costs the session nothing.
    pub const CONNECT: Duration = Duration::from_millis(300);
    /// Budget for pushing a non-blocking event out and leaving.
    pub const SEND: Duration = Duration::from_millis(500);
    /// `PreToolUse` blocks the tool call itself, so it stays short.
    pub const PRE_TOOL_USE: Duration = Duration::from_secs(45);
    /// Codex caps its own permission prompts an hour out.
    pub const PERMISSION_REQUEST_CODEX: Duration = Duration::from_secs(3_600);

    /// The wait budget for `event_name`, or `None` if it does not block.
    pub fn for_event(event_name: &str) -> Option<Duration> {
        match event_name {
            super::events::PRE_TOOL_USE => Some(PRE_TOOL_USE),
            super::events::PERMISSION_REQUEST => Some(PERMISSION_REQUEST_CODEX),
            _ => None,
        }
    }
}

/// A Codex permission-hook decision. Questions use [`Response::CodexInput`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HookDecision {
    PermissionRequest(PermissionRequestDecision),
}

impl HookDecision {
    /// Codex's hook stdout schema, with a trailing newline.
    pub fn to_stdout_json(&self) -> String {
        let Self::PermissionRequest(decision) = self;
        let mut body = serde_json::json!({"behavior": decision.behavior.as_str()});
        if decision.behavior == PermissionBehavior::Deny
            && let Some(message) = &decision.message
        {
            body["message"] = Value::String(message.clone());
        }
        format!(
            "{}\n",
            serde_json::json!({
                "hookSpecificOutput": {"hookEventName": events::PERMISSION_REQUEST, "decision": body}
            })
        )
    }

    pub fn allow_for(event_name: &str, reason: Option<String>) -> Option<Self> {
        (event_name == events::PERMISSION_REQUEST).then_some(Self::PermissionRequest(
            PermissionRequestDecision {
                behavior: PermissionBehavior::Allow,
                message: reason,
            },
        ))
    }

    pub fn deny_for(event_name: &str, reason: Option<String>) -> Option<Self> {
        (event_name == events::PERMISSION_REQUEST).then_some(Self::PermissionRequest(
            PermissionRequestDecision {
                behavior: PermissionBehavior::Deny,
                message: reason,
            },
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    Allow,
    Deny,
}

impl PermissionBehavior {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestDecision {
    pub behavior: PermissionBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Serialize an envelope as one newline-terminated line.
pub fn encode_line(envelope: &Envelope) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(envelope)?;
    line.push('\n');
    Ok(line)
}

/// Parse one line from the pipe.
pub fn decode_line(line: &str) -> serde_json::Result<Envelope> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_approvals_only_emit_fields_supported_by_codex() {
        for (decision, expected) in [
            (
                HookDecision::allow_for(events::PERMISSION_REQUEST, Some("approved".into()))
                    .unwrap(),
                serde_json::json!({"behavior":"allow"}),
            ),
            (
                HookDecision::deny_for(events::PERMISSION_REQUEST, Some("not this command".into()))
                    .unwrap(),
                serde_json::json!({"behavior":"deny","message":"not this command"}),
            ),
        ] {
            let output: Value = serde_json::from_str(&decision.to_stdout_json()).unwrap();
            assert_eq!(
                output,
                serde_json::json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":expected}})
            );
        }
        assert!(HookDecision::allow_for(events::PRE_TOOL_USE, None).is_none());
        assert!(HookDecision::deny_for(events::PRE_TOOL_USE, None).is_none());
    }

    #[test]
    fn deny_ignores_non_blocking_events() {
        assert!(HookDecision::deny_for(events::STOP, None).is_none());
    }

    #[test]
    fn hook_payload_round_trips_unknown_keys() {
        let raw = r#"{
            "hook_event_name": "PreToolUse",
            "session_id": "abc123",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/work",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "permission_mode": "default",
            "future_field": {"nested": [1, 2, 3]},
            "another": "kept"
        }"#;
        let payload: HookPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(payload.event_name(), "PreToolUse");
        assert_eq!(payload.session_id.as_deref(), Some("abc123"));
        assert_eq!(payload.tool_name.as_deref(), Some("Bash"));
        assert_eq!(payload.permission_mode.as_deref(), Some("default"));
        assert!(payload.is_blocking());
        assert_eq!(payload.extra.len(), 2);

        let round_tripped: Value = serde_json::to_value(&payload).unwrap();
        let original: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(round_tripped, original);
    }

    #[test]
    fn absent_fields_are_not_serialized() {
        let payload: HookPayload = serde_json::from_str(r#"{"hook_event_name":"Stop"}"#).unwrap();
        assert!(!payload.is_blocking());
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"hook_event_name":"Stop"}"#
        );
    }

    #[test]
    fn command_envelope_matches_upstream_shape() {
        let payload: HookPayload =
            serde_json::from_str(r#"{"hook_event_name":"SessionStart","session_id":"s1"}"#)
                .unwrap();
        let envelope = Envelope::Command {
            command: Command::ProcessHook {
                hook: payload,
                source: HookSource::Codex,
            },
        };
        let line = encode_line(&envelope).unwrap();
        assert_eq!(
            line,
            concat!(
                r#"{"type":"command","command":{"type":"processClaudeHook","#,
                r#""claudeHook":{"hook_event_name":"SessionStart","session_id":"s1"},"#,
                r#""source":"codex"}}"#,
                "\n"
            )
        );

        let decoded = decode_line(line.trim_end()).unwrap();
        let Envelope::Command {
            command: Command::ProcessHook { hook, .. },
        } = decoded
        else {
            panic!("expected a processHook command");
        };
        assert_eq!(hook.event_name(), "SessionStart");
    }

    #[test]
    fn response_envelope_round_trips() {
        let response = Envelope::Response {
            response: Response::Decision {
                decision: HookDecision::allow_for(events::PERMISSION_REQUEST, None).unwrap(),
            },
        };
        let line = encode_line(&response).unwrap();
        let Envelope::Response {
            response: Response::Decision { decision },
        } = decode_line(line.trim_end()).unwrap()
        else {
            panic!("expected a decision response");
        };
        assert!(decision.to_stdout_json().contains(r#""allow""#));
    }

    #[test]
    fn old_codex_launchers_remain_compatible_without_accepting_other_sources() {
        let old_frame = serde_json::json!({
            "type": "command",
            "command": {
                "type": "processClaudeHook",
                "claudeHook": {
                    "hook_event_name": events::CODEX_USER_INPUT,
                    "session_id": "thread-1",
                },
                "source": "codex",
            },
        });
        let Envelope::Command {
            command: Command::ProcessHook { hook, source },
        } = decode_line(&old_frame.to_string()).unwrap()
        else {
            panic!("expected a hook command");
        };
        assert_eq!(hook.event_name(), events::CODEX_USER_INPUT);
        assert_eq!(hook.session_id.as_deref(), Some("thread-1"));
        assert_eq!(source, HookSource::Codex);
        let mut unsupported_frame = old_frame;
        unsupported_frame["command"]["source"] = serde_json::json!("claude");
        assert!(decode_line(&unsupported_frame.to_string()).is_err());
    }

    #[test]
    fn generic_hook_wire_names_decode_to_the_same_codex_command() {
        let frame = serde_json::json!({
            "type": "command",
            "command": {
                "type": "processHook",
                "hook": {"hook_event_name": events::SESSION_START},
                "source": "codex",
            },
        });
        let Envelope::Command {
            command: Command::ProcessHook { hook, source },
        } = decode_line(&frame.to_string()).unwrap()
        else {
            panic!("expected a hook command");
        };
        assert_eq!(hook.event_name(), events::SESSION_START);
        assert_eq!(source, HookSource::Codex);
    }

    #[test]
    fn allow_for_ignores_non_blocking_events() {
        assert!(HookDecision::allow_for(events::STOP, None).is_none());
        assert!(HookDecision::allow_for(events::POST_TOOL_USE, None).is_none());
    }

    #[test]
    fn terminal_meta_round_trips_through_extra() {
        let mut payload = HookPayload::default();
        let mut env = Map::new();
        env.insert("WT_SESSION".into(), Value::String("guid".into()));
        payload.set_terminal_meta(TerminalMeta {
            env,
            hook_pid: 42,
            ancestors: vec![
                ProcessRef {
                    pid: 41,
                    exe: "cmd.exe".into(),
                },
                ProcessRef {
                    pid: 40,
                    exe: "windowsterminal.exe".into(),
                },
            ],
        });

        // Key order inside the flattened extra map is serde_json's business,
        // not ours: assert presence, and shapes via the decode below.
        let encoded = serde_json::to_string(&payload).unwrap();
        assert!(encoded.contains(r#""atollTerminal""#));
        assert!(encoded.contains(r#""ancestors""#));
        assert!(encoded.contains(r#""windowsterminal.exe""#));

        let decoded: HookPayload = serde_json::from_str(&encoded).unwrap();
        let meta = decoded.terminal_meta().unwrap();
        assert_eq!(meta.hook_pid, 42);
        assert_eq!(meta.ancestors.len(), 2);
        assert_eq!(meta.ancestors[1].exe, "windowsterminal.exe");
        assert_eq!(meta.env["WT_SESSION"], Value::String("guid".into()));
    }

    #[test]
    fn terminal_meta_from_an_older_hook_still_parses() {
        // A hook built before ancestors existed sends meta without them; the
        // session must simply come out not jumpable, not fail to parse.
        let raw = serde_json::json!({
            "atollTerminal": {"env": {}, "hookPid": 7}
        });
        let payload: HookPayload = serde_json::from_value(raw).unwrap();
        let meta = payload.terminal_meta().unwrap();
        assert_eq!(meta.hook_pid, 7);
        assert!(meta.ancestors.is_empty());
    }

    #[test]
    fn blocking_timeouts_match_codex_events() {
        assert_eq!(
            timeouts::for_event(events::PRE_TOOL_USE),
            Some(timeouts::PRE_TOOL_USE)
        );
        assert_eq!(
            timeouts::for_event(events::PERMISSION_REQUEST),
            Some(timeouts::PERMISSION_REQUEST_CODEX)
        );
        assert_eq!(timeouts::for_event(events::SESSION_START), None);
    }

    #[test]
    fn unknown_hook_sources_are_rejected() {
        assert!(serde_json::from_str::<HookSource>("\"unsupported\"").is_err());
    }
}
