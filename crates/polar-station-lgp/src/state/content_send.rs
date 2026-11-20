//! Durable send data for the existing Chat and Idea draft owners.
use super::*;
use crate::send_history::{Outcome, Row, MAX_ENTRIES};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Document {
    pub text: String,
    #[serde(default)]
    pub node_blocks: Vec<IdeaNodeBlock>,
    #[serde(default)]
    pub context: Option<idea_codex::Context>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ChatSubmission {
    pub intent: String,
    pub revision: u64,
    pub created_at_secs: u64,
    pub text: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Entry {
    pub intent: String,
    pub text: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<Document>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<idea_codex::Target>,
}

impl Draft {
    pub(super) fn document(&self) -> Document {
        Document { text: self.text.clone(), node_blocks: self.node_blocks.clone(), context: self.idea_context.clone() }
    }

    pub(super) fn clear_sent_document(&mut self, revision: u64) -> Result<(), String> {
        if self.revision == revision {
            self.revision = self.revision.checked_add(1).ok_or("draft version exhausted")?;
            self.text.clear();
            self.node_blocks.clear();
            self.idea_context = None;
        }
        Ok(())
    }

    fn empty_document(&self) -> bool {
        self.text.is_empty() && self.node_blocks.is_empty() && self.idea_context.is_none()
    }

    pub(super) fn record_send(&mut self, entry: Entry) {
        self.send_history.push(entry);
        if self.send_history.len() > MAX_ENTRIES { self.send_history.remove(0); }
    }

    pub(super) fn finish_chat_send(&mut self, outcome: Outcome) {
        if let Some(send) = self.chat_submission.take() {
            self.record_send(Entry { intent: send.intent, text: send.text, outcome, document: None, target: None });
        }
        self.pending = None;
    }

    pub(super) fn send_rows(&self) -> Vec<Row> {
        let chat = self.chat_submission.as_ref().map(|send| Row::new(&send.intent, &send.text, "Sending", ""));
        let idea = self.idea_submission.as_ref().and_then(|send| {
            let polar_station_protocol::CodexCommandV1::SubmitThreadText { text, .. } = send.command.as_ref()? else { return None; };
            let (status, action) = if self.pending.is_some() { ("Sending", "") } else { ("Result unknown", "Check receipt") };
            Some(Row::new(&send.intent, text, status, action))
        });
        chat.into_iter().chain(idea).chain(self.send_history.iter().rev().filter(|entry| self.history_targets(entry)).map(|entry|
            Row::completed(&entry.intent, &entry.text, entry.outcome, self.empty_document())
        )).collect()
    }

    fn history_targets(&self, entry: &Entry) -> bool {
        match (&entry.target, &self.idea_target) {
            (None, None) => true,
            (Some(old), Some(current)) => old.owner_host_id == current.owner_host_id
                && old.endpoint_id == current.endpoint_id && old.thread_id == current.thread_id,
            _ => false,
        }
    }
}

impl Engine {
    pub fn trim_send_history(&mut self) -> bool {
        let oldest = self.drafts.values_mut().filter_map(|draft| {
            let index = draft.send_history.iter().position(|entry| entry.outcome == Outcome::Sent)
                .or_else(|| (!draft.send_history.is_empty()).then_some(0))?;
            Some((draft.send_history[index].outcome != Outcome::Sent, index, draft))
        }).min_by_key(|(retained, _, _)| *retained);
        if let Some((_, index, draft)) = oldest {
            draft.send_history.remove(index);
            return true;
        }
        false
    }

    pub fn has_chat_submission(&self, intent: &str) -> bool {
        self.drafts.values().any(|d| d.chat_submission.as_ref().is_some_and(|send| send.intent == intent))
    }

    pub fn saved_chat_submission(&mut self, intent: &str, success: bool) -> Result<Vec<Effect>, String> {
        let Some((key, send)) = self.drafts.iter().find_map(|(key, draft)|
            draft.chat_submission.as_ref().filter(|s| s.intent == intent).map(|s| (key.clone(), s.clone()))
        ) else { return Ok(vec![]); };
        let DraftContext::Thread(scope) = &key.1 else { return Err("Invalid Chat target".into()); };
        let mut out = Vec::new();
        if !success {
            let draft = self.drafts.get_mut(&key).expect("Chat draft");
            draft.chat_submission = None;
            draft.pending = None;
            draft.status = "Could not save the submission. Draft retained.".into();
        } else {
            self.drafts.get_mut(&key).expect("Chat draft").clear_sent_document(send.revision)?;
            let lease = self.threads.get(scope).filter(|t| t.status == "live" && t.subscription.is_some()).and_then(|t| t.lease.clone());
            let request = lease.ok_or_else(|| "Thread is not connected".to_owned()).and_then(|lease| {
                self.request(&scope.target_id, json!({"op":"send","expected_node_id":scope.node_id,"lease_id":lease,
                    "client_intent_id":send.intent,"created_at_secs":send.created_at_secs,"text":send.text}),
                    Purpose::Send { scope: scope.clone(), window: key.0.clone(), revision: send.revision, text: send.text }, &mut out)
            });
            let draft = self.drafts.get_mut(&key).expect("Chat draft");
            match request {
                Ok(id) => draft.pending = Some(id),
                Err(error) => {
                    draft.finish_chat_send(Outcome::Failed);
                    draft.status = format!("Not sent — {error}");
                    out.push(Effect::PersistDrafts);
                }
            }
        }
        self.composer_effects(&mut out);
        self.flush_requests(&mut out);
        Ok(out)
    }

    pub(super) fn recover_content_send(&mut self, window: &str, pane: &str, revision: u64, intent: &str, out: &mut Vec<Effect>) -> Result<(), String> {
        if self.selected.get(window).is_none_or(|selected| selected != pane) {
            return Err("Input target changed".into());
        }
        let key = self.draft_key(window)?;
        let draft = self.drafts.get_mut(&key).ok_or("Missing draft")?;
        if draft.revision != revision || matches!(key.1, DraftContext::Inquiry { .. }) {
            return Err("Input draft changed".into());
        }
        if draft.idea_submission.as_ref().is_some_and(|submission| submission.intent == intent) {
            return self.idea_check_receipt(window, out);
        }
        let entry = draft.send_history.iter().find(|entry| entry.intent == intent && draft.history_targets(entry)).ok_or("Send history entry is unavailable")?;
        if entry.outcome != Outcome::Failed || !draft.empty_document() {
            out.push(Effect::CopySendText { window_id: window.into(), text: entry.text.clone() });
            return Ok(());
        }
        draft.revision = draft.revision.checked_add(1).ok_or("draft version exhausted")?;
        if let Some(doc) = &entry.document {
            draft.text.clone_from(&doc.text);
            draft.node_blocks.clone_from(&doc.node_blocks);
            draft.idea_context.clone_from(&doc.context);
        } else {
            draft.text.clone_from(&entry.text);
        }
        draft.status.clear();
        out.push(Effect::PersistDrafts);
        Ok(())
    }

    pub(super) fn chat_delivery_failed(&mut self, scope: Scope, window: String, outcome: Outcome, message: &str, out: &mut Vec<Effect>) {
        let (label, status) = if outcome == Outcome::Failed { ("Not sent", "send_rejected") } else { ("Unconfirmed", "send_unconfirmed") };
        if let Some(draft) = self.drafts.get_mut(&(window, scope.clone().into())) {
            draft.finish_chat_send(outcome);
            draft.status = format!("{label} — {message}");
            out.push(Effect::PersistDrafts);
        }
        self.status(&scope, status, message, out);
    }
}
