//! Optional Codex title and completion-event adapter.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Command;

use polar_tmux_core::CONTROL_MODE_AGENT_EVENT_OPTION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexTitleState {
    Working,
    ActionRequired,
    Ready,
}

pub(crate) fn decode_codex_title(
    pane_current_command: &str,
    pane_title: &str,
) -> Option<CodexTitleState> {
    if pane_current_command.rsplit('/').next()? != "codex" {
        return None;
    }
    let normalized = pane_title.to_ascii_lowercase();
    if normalized.contains("action required") {
        Some(CodexTitleState::ActionRequired)
    } else if ["working", "thinking", "waiting", "starting"]
        .iter()
        .any(|state| normalized.contains(state))
        || pane_title.chars().any(is_codex_activity_spinner)
    {
        Some(CodexTitleState::Working)
    } else if normalized.contains("ready") {
        Some(CodexTitleState::Ready)
    } else {
        None
    }
}

fn is_codex_activity_spinner(value: char) -> bool {
    matches!(
        value,
        '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏'
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodexAgentEventKind {
    TurnComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexAgentEvent {
    pub(crate) version: u8,
    pub(crate) kind: CodexAgentEventKind,
    pub(crate) event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexAgentEventQueue {
    pub(crate) version: u8,
    pub(crate) events: Vec<CodexAgentEvent>,
}

pub(crate) fn parse_codex_event_queue(value: &str) -> Result<Vec<CodexAgentEvent>, String> {
    let queue: CodexAgentEventQueue =
        serde_json::from_str(value).map_err(|_| "Codex event queue is invalid".to_owned())?;
    if queue.version != 1
        || queue.events.len() > 8
        || queue.events.iter().any(|event| {
            event.version != 1
                || event.event_id.len() != 64
                || !event.event_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err("Codex event queue is outside the bounded v1 contract".to_owned());
    }
    Ok(queue.events)
}

fn append_codex_event(
    encoded_queue: Option<&str>,
    event: CodexAgentEvent,
) -> Result<String, String> {
    let mut events = match encoded_queue.filter(|value| !value.is_empty()) {
        Some(value) => parse_codex_event_queue(value)?,
        None => Vec::new(),
    };
    if !events
        .iter()
        .any(|current| current.event_id == event.event_id)
    {
        events.push(event);
    }
    if events.len() > 8 {
        events.drain(..events.len() - 8);
    }
    serde_json::to_string(&CodexAgentEventQueue { version: 1, events })
        .map_err(|_| "Codex event queue could not be encoded".to_owned())
}

/// Records one privacy-minimized Codex completion in the current tmux pane.
///
/// Invalid payloads and invocations outside tmux deliberately become no-ops so
/// Codex's notification subprocess can never block or fail a completed turn.
pub(crate) fn record_codex_notify(payload: &str) -> Result<bool, String> {
    let Ok(event) = parse_codex_notify_payload(payload) else {
        return Ok(false);
    };
    let Ok(pane_id) = std::env::var("TMUX_PANE") else {
        return Ok(false);
    };
    if !pane_id.strip_prefix('%').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Ok(false);
    }
    let current = Command::new("tmux")
        .args([
            "show-options",
            "-p",
            "-v",
            "-t",
            &pane_id,
            CONTROL_MODE_AGENT_EVENT_OPTION,
        ])
        .output()
        .map_err(|_| "tmux event option could not be read".to_owned())?;
    let current = current
        .status
        .success()
        .then(|| String::from_utf8_lossy(&current.stdout).trim().to_owned());
    let encoded = append_codex_event(current.as_deref(), event)?;
    let status = Command::new("tmux")
        .args([
            "set-option",
            "-p",
            "-t",
            &pane_id,
            CONTROL_MODE_AGENT_EVENT_OPTION,
            &encoded,
        ])
        .status()
        .map_err(|_| "tmux event option could not be written".to_owned())?;
    Ok(status.success())
}

#[derive(Deserialize)]
struct CodexNotifyPayload {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "thread-id")]
    thread_id: String,
    #[serde(rename = "turn-id")]
    turn_id: String,
}

pub(crate) fn parse_codex_notify_payload(value: &str) -> Result<CodexAgentEvent, String> {
    let payload: CodexNotifyPayload =
        serde_json::from_str(value).map_err(|_| "Codex notify payload is invalid".to_owned())?;
    if payload.kind != "agent-turn-complete"
        || !valid_opaque_id(&payload.thread_id)
        || !valid_opaque_id(&payload.turn_id)
    {
        return Err("Codex notify payload is not a valid turn completion".to_owned());
    }
    let mut digest = Sha256::new();
    digest.update(b"polar-station-codex-event-v1\0");
    digest.update(payload.thread_id.as_bytes());
    digest.update(b"\0");
    digest.update(payload.turn_id.as_bytes());
    let event_id = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(CodexAgentEvent {
        version: 1,
        kind: CodexAgentEventKind::TurnComplete,
        event_id,
    })
}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}


