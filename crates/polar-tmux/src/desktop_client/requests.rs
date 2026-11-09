//! One correlation ledger for all desktop control APIs. No native/UI ownership.
use polar_tmux_protocol::{DesktopSendError, LocalClientFrame, LocalHostFrame};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use tokio::time::Instant;

pub(super) const LIMIT: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(8);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFailure {
    pub request_id: String,
    pub api: String,
    pub code: String,
    pub message: String,
    pub outcome_unknown: bool,
}
#[derive(Debug)]
struct Pending {
    api: String,
    owner: Option<Value>,
    target: Option<Value>,
    deadline: Instant,
    writing: bool,
    detached: bool,
    mutation: bool,
    sequence: u64,
    queued: Option<LocalClientFrame>,
}
#[derive(Debug, Default)]
pub(super) struct Requests {
    pending: BTreeMap<String, Pending>,
    next_sequence: u64,
}
fn envelope(frame: &LocalClientFrame) -> Option<Value> {
    match frame {
        LocalClientFrame::Request { .. } | LocalClientFrame::ApplicationRequest { .. } => {
            serde_json::to_value(frame).ok()?.get("request").cloned()
        }
        _ => None,
    }
}
impl Requests {
    pub fn admit(
        &mut self,
        frame: &LocalClientFrame,
        now: Instant,
    ) -> Result<Option<String>, DesktopSendError> {
        let Some(v) = envelope(frame) else {
            return Ok(None);
        };
        let id = v["request_id"]
            .as_str()
            .ok_or(DesktopSendError::InvalidRequest)?
            .to_owned();
        if self.pending.contains_key(&id) {
            return Err(DesktopSendError::InvalidRequest);
        }
        if self.pending.len() >= LIMIT {
            return Err(DesktopSendError::QueueFull);
        }
        let command = &v["command"];
        let verb = command["kind"]
            .as_str()
            .or_else(|| command["type"].as_str())
            .unwrap_or("");
        // Unknown commands are conservatively effectful, never safe to replay.
        let mutation = !matches!(
            verb,
            "get_sender_status"
                | "get_sender_settings"
                | "get_host"
                | "get_snapshot"
                | "list_execution_targets"
                | "list_sessions"
                | "list_windows"
                | "list_panes"
                | "list_clients"
                | "get_companion_pairing"
                | "get_projection"
                | "session_editor"
                | "get_ui"
                | "get_settings"
                | "get_bindings"
                | "get_binding"
                | "get_completions"
                | "get_receipt"
                | "get_page"
                | "list_history"
                | "list_loaded"
                | "read_history_detail"
                | "read_thread"
                | "read_directory"
                | "get_telegram_status"
                | "get_log_tail"
                | "get_codex_notify_status"
                | "snapshot"
                | "prepare_attach"
        );
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(DesktopSendError::Unavailable)?;
        self.pending.insert(
            id.clone(),
            Pending {
                api: v["api"].as_str().unwrap_or("tmux_v1").into(),
                owner: v.get("owner_host_id").cloned(),
                target: v.get("execution_target").cloned(),
                deadline: now + TIMEOUT,
                writing: false,
                detached: false,
                mutation,
                sequence: self.next_sequence,
                queued: None,
            },
        );
        Ok(Some(id))
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn disconnect(&mut self, message: &str) -> Vec<RequestFailure> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|(id, p)| {
                (!p.detached).then(|| failure(&id, &p, "connection_lost", message))
            })
            .collect()
    }
    pub fn rollback(&mut self, id: &str) {
        self.pending.remove(id);
    }
    pub fn writing(&mut self, frame: &LocalClientFrame) -> bool {
        let Some(v) = envelope(frame) else {
            return true;
        };
        let Some(id) = v["request_id"].as_str() else {
            return false;
        };
        let Some(p) = self.pending.get(id) else {
            return false;
        };
        if p.detached {
            self.pending.remove(id);
            return false;
        }
        let blocked = p.mutation
            && self
                .pending
                .values()
                .any(|other| other.mutation && other.api == p.api && other.sequence < p.sequence);
        let p = self.pending.get_mut(id).expect("checked request");
        if blocked {
            p.queued = Some(frame.clone());
            return false;
        }
        p.writing = true;
        true
    }
    pub fn next_write(&mut self) -> Option<LocalClientFrame> {
        let id = self
            .pending
            .iter()
            .filter(|(_, p)| p.queued.is_some() && !p.detached)
            .filter(|(_, p)| {
                !self.pending.values().any(|other| {
                    other.mutation && other.api == p.api && other.sequence < p.sequence
                })
            })
            .min_by_key(|(_, p)| p.sequence)
            .map(|(id, _)| id.clone())?;
        let p = self.pending.get_mut(&id)?;
        p.writing = true;
        p.queued.take()
    }
    pub fn cancel(&mut self, id: &str) -> Option<RequestFailure> {
        let p = self.pending.get_mut(id)?;
        if p.detached {
            return None;
        }
        p.detached = true;
        let result = failure(
            id,
            p,
            "request_cancelled",
            "The caller cancelled this request.",
        );
        if p.queued.is_some() {
            self.pending.remove(id);
        }
        Some(result)
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
            .values()
            .filter(|p| !p.detached)
            .map(|p| p.deadline)
            .min()
    }
    pub fn expire(&mut self, now: Instant) -> Vec<RequestFailure> {
        let failures = self
            .pending
            .iter_mut()
            .filter_map(|(id, p)| {
                if p.detached || p.deadline > now {
                    return None;
                }
                p.detached = true;
                Some(failure(
                    id,
                    p,
                    "request_timeout",
                    "The Host did not confirm this request before its deadline.",
                ))
            })
            .collect();
        self.pending
            .retain(|_, p| !(p.detached && p.queued.is_some()));
        failures
    }
    /// None is a notification, Some(false) drains a detached consumer's reply.
    pub fn response(&mut self, frame: &LocalHostFrame) -> Result<Option<bool>, &'static str> {
        if !matches!(
            frame,
            LocalHostFrame::Response { .. } | LocalHostFrame::ApplicationResponse { .. }
        ) {
            return Ok(None);
        }
        let v = serde_json::to_value(frame).map_err(|_| "invalid response")?;
        let v = &v["response"];
        let id = v["request_id"].as_str().ok_or("missing request ID")?;
        let p = self.pending.get(id).ok_or("unsolicited response")?;
        if v["api"].as_str().unwrap_or("tmux_v1") != p.api
            || v.get("owner_host_id") != p.owner.as_ref()
            || v.get("execution_target") != p.target.as_ref()
        {
            return Err("response identity mismatch");
        }
        let active = !p.detached;
        self.pending.remove(id);
        Ok(Some(active))
    }
}
fn failure(id: &str, p: &Pending, code: &str, message: &str) -> RequestFailure {
    let outcome_unknown = p.mutation && p.writing;
    let message = if outcome_unknown {
        format!("{message} Its outcome is unknown; refresh before retrying.")
    } else if p.mutation {
        format!("The request was not sent. {message}")
    } else {
        message.to_owned()
    };
    RequestFailure {
        request_id: id.into(),
        api: p.api.clone(),
        code: code.into(),
        message,
        outcome_unknown,
    }
}




