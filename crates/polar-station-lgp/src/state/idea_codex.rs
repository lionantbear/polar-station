//! One-way Idea instructions use the existing Host queue and durable receipts.
use super::*;
use polar_ideacave_presentation::controls::NodeReference;
use polar_station_protocol::{
    ClientApplicationMessage, CodexApiV1, CodexCommandV1, CodexRequestEnvelopeV1, CodexResultV1,
    CodexSubmissionStatusV1, CODEX_INPUT_MAX_BYTES,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Context {
    pub source_host_id: Option<String>,
    pub source_paths: Vec<String>,
    pub references: Vec<NodeReference>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    pub owner_host_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub title: String,
}
impl Target {
    pub(super) fn validate(&self) -> Result<(), String> {
        for id in [&self.owner_host_id, &self.endpoint_id, &self.thread_id] {
            validate_id(id)?;
        }
        if self.title.len() > 4096 || self.title.chars().any(char::is_control) {
            return Err("Invalid session title".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submission {
    pub target: Target,
    pub intent: String,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CodexCommandV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<super::content_send::Document>,
}
#[derive(Default)]
pub(super) struct Codex {
    pub host: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Operation {
    Show {
        owner: String,
        window: String,
        pane: String,
        view: String,
        endpoint: String,
        thread: String,
    },
}
impl Engine {
    pub(super) fn idea_key(&self, window: &str) -> Result<(String, DraftContext), String> {
        let key = self.draft_key(window)?;
        if !matches!(key.1, DraftContext::Idea(_)) {
            return Err("Select an Idea Tree".into());
        }
        Ok(key)
    }
    pub(super) fn idea_capture(&mut self, window: &str) -> Result<(), String> {
        let key = self.idea_key(window)?;
        let view = &self.ideas[self.selected.get(window).ok_or("Select an Idea Tree")?];
        let context = Context {
            source_host_id: self.idea_codex.host.clone(),
            source_paths: view.state.source_paths.clone(),
            references: view.state.selected_reference.clone().into_iter().collect(),
        };
        let draft = self.drafts.entry(key).or_default();
        if draft.idea_context.is_none() {
            draft.idea_context = Some(context);
            draft.revision += 1;
        }
        Ok(())
    }
    pub(super) fn idea_reference(
        &mut self,
        window: &str,
        operation: &str,
        index: usize,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let key = self.idea_key(window)?;
        if operation == "capture" {
            return self.idea_capture(window);
        }
        let pane = self.selected[window].clone();
        let view = &self.ideas[&pane];
        let reference = view.state.selected_reference.clone();
        let identity = view.identity.clone();
        if operation == "add" {
            self.idea_capture(window)?;
            let reference = reference.ok_or("Select a loaded node to attach")?;
            let d = self.drafts.get_mut(&key).ok_or("Missing draft")?;
            let context = d.idea_context.as_mut().ok_or("Missing context")?;
            if !context.references.iter().any(|r| {
                r.source_path == reference.source_path
                    && if reference.node_uuid.is_empty() {
                        r.node_key == reference.node_key
                    } else {
                        r.node_uuid == reference.node_uuid
                    }
            }) {
                if context.references.len() >= 64 {
                    return Err("At most 64 references per instruction".into());
                }
                context.references.push(reference);
                d.revision += 1;
            }
        } else if operation == "remove" {
            let d = self.drafts.get_mut(&key).ok_or("Missing draft")?;
            if let Some(context) = d.idea_context.as_mut() {
                if index < context.references.len() {
                    context.references.remove(index);
                    d.revision += 1;
                }
                if context.references.is_empty() && d.text.is_empty() {
                    d.idea_context = None;
                }
            }
        } else if operation == "locate" {
            let reference = self
                .drafts
                .get(&key)
                .and_then(|d| d.idea_context.as_ref())
                .and_then(|c| c.references.get(index))
                .ok_or("Reference is no longer present")?;
            out.push(Effect::IdeaLocateReference {
                pane_id: pane,
                view_id: identity,
                reference: serde_json::to_value(reference).map_err(|e| e.to_string())?,
            });
        } else {
            return Err("Unknown reference operation".into());
        }
        Ok(())
    }

    pub(super) fn idea_bind(
        &mut self,
        window: &str,
        binding_id: Option<String>,
    ) -> Result<(), String> {
        if binding_id.is_some() {
            return Err("Choose a session in the Resume picker".into());
        }
        let key = self.idea_key(window)?;
        let draft = self.drafts.entry(key).or_default();
        if draft.idea_submission.is_some() {
            return Err("Check the unresolved submission before changing session".into());
        }
        draft.idea_target = None;
        draft.revision += 1;
        draft.status.clear();
        Ok(())
    }
    pub(super) fn idea_operation_owner<'a>(&'a self, op: &'a Operation) -> Option<&'a str> {
        match op {
            Operation::Show { owner, .. } => Some(owner),
        }
    }
    pub(super) fn idea_pick_session(
        &mut self,
        pane: String,
        view: String,
        target: Target,
    ) -> Result<(), String> {
        if self.idea_codex.host.is_none() {
            return Err("Codex protocol unavailable; reconnect to an updated Host".into());
        }
        if self.ideas.get(&pane).is_none_or(|v| v.identity != view) {
            return Err("The original Idea Pane is no longer open".into());
        }
        target.validate()?;
        let draft = self
            .drafts
            .entry((String::new(), DraftContext::Idea(view)))
            .or_default();
        if draft.idea_submission.is_some() {
            return Err("Check the unresolved submission before changing session".into());
        }
        draft.idea_target = Some(target);
        draft.revision += 1;
        draft.status.clear();
        Ok(())
    }
    pub(super) fn idea_codex_request(
        &mut self,
        command: CodexCommandV1,
        operation: Operation,
    ) -> Result<String, String> {
        let host = self
            .idea_operation_owner(&operation)
            .ok_or("Codex Host is disconnected")?
            .to_owned();
        if self.pending.len() >= MAX_QUEUED_REQUESTS {
            return Err("Too many pending requests".into());
        }
        let id = self.next_id("idea-codex")?;
        let request = CodexRequestEnvelopeV1 {
            protocol_version: 1,
            api: CodexApiV1::CodexV1,
            request_id: id.clone(),
            owner_host_id: host,
            command,
        };
        request.validate().map_err(|e| e.to_string())?;
        let frame = serde_json::to_value(LocalClientFrame::ApplicationRequest {
            request: ClientApplicationMessage::CodexV1(Box::new(request)),
        })
        .map_err(|e| e.to_string())?;
        self.pending.insert(
            id.clone(),
            Pending {
                purpose: Purpose::IdeaCodex(operation),
                deadline: self.now_ms.saturating_add(120_000),
                target: "@idea-codex".into(),
                sent: false,
            },
        );
        self.queued.push_back((id.clone(), frame));
        Ok(id)
    }

    pub(super) fn idea_send(&mut self, window: &str, out: &mut Vec<Effect>) -> Result<(), String> {
        if self.idea_codex.host.is_none() {
            return Err("Codex protocol unavailable; reconnect to an updated Host".into());
        }
        let key = self.idea_key(window)?;
        let draft = self.drafts.get(&key).ok_or("Missing draft")?;
        let target = draft.idea_target.clone().ok_or("Bind a Codex session")?;
        if draft.idea_submission.is_some() || !draft.idea_has_content() {
            return Err("Send is not enabled".into());
        }
        let text = draft.idea_document_text();
        if text.len() > CODEX_INPUT_MAX_BYTES {
            return Err(
                "Instruction and references exceed 16 KiB; shorten the text or remove references"
                    .into(),
            );
        }
        let intent = polar_identity::generation::new_uuid_v7().to_string();
        let revision = draft.revision;
        revision.checked_add(1).ok_or("draft version exhausted")?;
        let document = Some(draft.document());
        let command = CodexCommandV1::SubmitThreadText {
            endpoint_id: target.endpoint_id.clone(),
            thread_id: target.thread_id.clone(),
            intent_id: intent.clone(),
            text,
        };
        let draft = self.drafts.get_mut(&key).ok_or("Missing draft")?;
        draft.idea_submission = Some(Submission {
            target,
            intent: intent.clone(),
            revision,
            command: Some(command),
            document,
        });
        draft.pending = Some(intent.clone());
        out.push(Effect::CodexSubmission { intent, check: false });
        draft.status = "Submitting to Codex…".into();
        Ok(())
    }
    pub(super) fn idea_check_receipt(&mut self, window: &str, out: &mut Vec<Effect>) -> Result<(), String> {
        if self.idea_codex.host.is_none() {
            return Err("Codex protocol unavailable".into());
        }
        let key = self.idea_key(window)?;
        let draft = self.drafts.get(&key).ok_or("Missing draft")?;
        if draft.pending.is_some() {
            return Err("Waiting for the current request".into());
        }
        let submission = draft
            .idea_submission
            .clone()
            .ok_or("No unresolved submission")?;
        let draft = self.drafts.get_mut(&key).ok_or("Missing draft")?;
        draft.pending = Some(submission.intent.clone());
        out.push(Effect::CodexSubmission { intent: submission.intent, check: true });
        draft.status = "Checking submission receipt…".into();
        Ok(())
    }
    /// The input coordinator reads the frozen request, never the current editor.
    pub fn idea_submission(&self, intent: &str) -> Option<(String, Option<CodexCommandV1>)> {
        self.drafts.values().find_map(|draft| {
            let submission = draft.idea_submission.as_ref().filter(|s| s.intent == intent)?;
            Some((submission.target.owner_host_id.clone(), submission.command.clone()))
        })
    }
    pub fn saved_idea_submission(&mut self, intent: &str) -> Result<Vec<Effect>, String> {
        if let Some(draft) = self.drafts.values_mut().find(|draft| draft.idea_submission.as_ref().is_some_and(|s| s.intent == intent)) {
            let submission = draft.idea_submission.as_ref().expect("frozen Idea submission");
            if submission.document.is_some() { draft.clear_sent_document(submission.revision)?; }
        }
        let mut out = Vec::new();
        self.idea_composer_effects(&mut out);
        Ok(out)
    }
    pub fn complete_idea_submission(&mut self, intent: &str, result: CodexResultV1, not_sent: bool) -> Result<Vec<Effect>, String> {
        let Some(draft) = self.drafts.values_mut().find(|d|d.idea_submission.as_ref().is_some_and(|s|s.intent==intent)) else { return Ok(vec![]) };
        let submission=draft.idea_submission.as_ref().ok_or("Missing submission")?;
        let revision=submission.revision;
        let cleared=draft.revision!=revision;
        draft.pending=None;
        let mut outcome=None;
        match result {
            CodexResultV1::Receipt { receipt:Some(receipt) } if receipt.intent_id==intent => {
                draft.status=receipt.label;
                if matches!(receipt.status,CodexSubmissionStatusV1::Accepted|CodexSubmissionStatusV1::Rejected) {
                    outcome=Some(if receipt.status==CodexSubmissionStatusV1::Accepted {Outcome::Sent} else {Outcome::Failed});
                }
                if receipt.clear_input && receipt.status==CodexSubmissionStatusV1::Accepted {
                    draft.clear_sent_document(revision)?;
                }
            }
            result=>{
                let message=if let CodexResultV1::Error { code }=result {code} else {"Unconfirmed response".into()};
                if not_sent {
                    outcome=Some(Outcome::Failed);
                    draft.status=format!("Not submitted: {message}");
                } else {
                    draft.status=format!("{message} — Check receipt");
                }
            }
        }
        if let Some(outcome) = outcome {
            let submission = draft.idea_submission.take().expect("frozen Idea submission");
            if let Some(CodexCommandV1::SubmitThreadText { text, .. }) = submission.command.filter(|_| !not_sent || cleared) {
                draft.record_send(super::content_send::Entry { intent: intent.into(), text, outcome, document: submission.document, target: Some(submission.target) });
            }
        }
        let mut out=vec![Effect::PersistDrafts];self.idea_composer_effects(&mut out);Ok(out)
    }
    pub(super) fn disconnect_idea_submissions(&mut self) {
        for draft in self.drafts.values_mut().filter(|d|d.idea_submission.is_some()) {
            draft.pending = None;
            draft.status = "Connection closed — Check receipt".into();
        }
    }
    pub(super) fn idea_show_session(&mut self, window: &str) -> Result<(), String> {
        if self.idea_codex.host.is_none() {
            return Err("Codex protocol unavailable".into());
        }
        let key = self.idea_key(window)?;
        let target = self
            .drafts
            .get(&key)
            .and_then(|d| d.idea_target.as_ref())
            .ok_or("Bind a Codex session")?
            .clone();
        let DraftContext::Idea(view) = key.1 else {
            return Err("Select an Idea Tree".into());
        };
        self.idea_codex_request(
            CodexCommandV1::GetThreadBinding {
                endpoint_id: target.endpoint_id.clone(),
                thread_id: target.thread_id.clone(),
            },
            Operation::Show {
                owner: target.owner_host_id,
                window: window.into(),
                pane: self.selected[window].clone(),
                view,
                endpoint: target.endpoint_id,
                thread: target.thread_id,
            },
        )?;
        Ok(())
    }
    pub(super) fn idea_codex_result(
        &mut self,
        op: Operation,
        result: CodexResultV1,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        match (op, result) {
            (
                Operation::Show {
                    owner,
                    window,
                    pane,
                    view,
                    endpoint,
                    thread,
                },
                CodexResultV1::Binding {
                    binding: Some(state),
                },
            ) => {
                if self.ideas.get(&pane).is_none_or(|v| v.identity != view) {
                    return Ok(());
                }
                if state.binding.owner_host_id != owner
                    || state.binding.endpoint_id != endpoint
                    || state.binding.thread_id != thread
                {
                    return Err("Codex session identity mismatch".into());
                }
                let session = state
                    .binding
                    .session
                    .ok_or("This Codex session has no terminal view")?;
                out.push(Effect::EnsureTerminalVisible { window_id: window, pane_id: pane, token: format!("idea-session:{}", self.next_id("view")?), target: json!({"client_view":{"execution_target_id":session.execution_target.as_str(),"tmux_session_id":session.tmux_session_id}}) });
            }
            (op, CodexResultV1::Error { code }) => self.idea_codex_failed(op, &code, out),
            (op, _) => self.idea_codex_failed(
                op,
                "Unconfirmed response; check receipt before retrying",
                out,
            ),
        }
        Ok(())
    }
    pub(super) fn idea_codex_failed(&mut self, _op:Operation, message:&str, out:&mut Vec<Effect>) {
        out.push(Effect::Error { message:format!("Codex: {message}") });
    }
}
