//! Shared save barrier and Codex transport for domain-owned input documents.
use crate::{
    codex::Pending, generic::Submission, input::Effect as InputEffect, terminal::present,
    InputEngine,
};
use polar_station_lgp::Effect;
use polar_station_protocol::{CodexCommandV1, CodexResultV1};

#[derive(Default)]
pub(crate) struct SubmissionCoordinator {
    pub(crate) saving: std::collections::BTreeMap<String, u64>,
}

impl InputEngine {
    pub(crate) fn content_submission_effects(
        &mut self,
        effects: Vec<Effect>,
    ) -> Result<Vec<Effect>, String> {
        let (intents, mut out): (Vec<_>, Vec<_>) = effects
            .into_iter()
            .partition(|e| matches!(e, Effect::CodexSubmission { .. }));
        for effect in intents {
            let Effect::CodexSubmission { intent, check } = effect else {
                unreachable!()
            };
            let Some((owner, command)) = self.content.idea_submission(&intent) else {
                continue;
            };
            if check {
                let pending = match command {
                    Some(command) => Pending::RetryReceipt {
                        intent: intent.clone(),
                        owner: owner.clone(),
                        command: Box::new(command),
                    },
                    None => Pending::Send(intent.clone()), // Legacy records can only check receipts.
                };
                match self.codex_request_content(
                    owner,
                    CodexCommandV1::GetReceipt {
                        intent_id: intent.clone(),
                    },
                    pending,
                ) {
                    Ok(effect) => out.push(effect),
                    Err(message) => out.extend(self.content.complete_idea_submission(
                        &intent,
                        CodexResultV1::Error { code: message },
                        false,
                    )?),
                }
            } else {
                match self.stage_submission_snapshot(intent.clone()) {
                    Ok(snapshot) => {
                        out.extend(snapshot);
                    }
                    Err(message) => out.extend(self.content.complete_idea_submission(
                        &intent,
                        CodexResultV1::Error {
                            code: format!("Could not save submission: {message}"),
                        },
                        true,
                    )?),
                }
            }
        }
        Ok(out)
    }
    pub(crate) fn stage_submission_snapshot(
        &mut self,
        intent: String,
    ) -> Result<Vec<Effect>, String> {
        let snapshot = self.dispatch_content(polar_station_lgp::Input::ExportDrafts)?;
        self.submissions
            .saving
            .insert(intent, self.snapshot_revision);
        Ok(snapshot)
    }
    pub(crate) fn snapshot_stored(
        &mut self,
        revision: u64,
        success: bool,
    ) -> Result<Vec<InputEffect>, String> {
        if revision > self.snapshot_revision {
            return Err("Unknown snapshot receipt".into());
        }
        let intents = self
            .submissions
            .saving
            .iter()
            .filter(|(_, saved)| **saved <= revision)
            .map(|(intent, _)| intent.clone())
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for intent in intents {
            self.submissions.saving.remove(&intent);
            if !success {
                out.extend(self.reject_unsent_submission(
                    &intent,
                    "Could not save the submission; nothing was sent. Draft retained.".into(),
                )?);
                continue;
            }
            let submission =
                if let Some((owner_host_id, command)) = self.content.idea_submission(&intent) {
                    command.map(|command| Submission::Codex {
                        owner_host_id,
                        command,
                    })
                } else {
                    self.generic.saved_submission(&intent)
                };
            match submission {
                Some(Submission::Terminal(input)) => {
                    out.push(self.dispatch_saved_terminal(intent, input))
                }
                Some(Submission::Codex {
                    owner_host_id,
                    command,
                }) => {
                    match self.codex_request(owner_host_id, command, Pending::Send(intent.clone()))
                    {
                        Ok(effect) => out.push(effect),
                        Err(message) => out.extend(self.reject_unsent_submission(
                            &intent,
                            format!("Send was not dispatched: {message}. Draft retained."),
                        )?),
                    }
                }
                None => {}
            }
        }
        Ok(out)
    }
    fn reject_unsent_submission(
        &mut self,
        intent: &str,
        message: String,
    ) -> Result<Vec<InputEffect>, String> {
        if self.content.idea_submission(intent).is_some() {
            self.content
                .complete_idea_submission(intent, CodexResultV1::Error { code: message }, true)
                .map(|effects| {
                    effects
                        .into_iter()
                        .map(|e| InputEffect::Content(Box::new(e)))
                        .collect()
                })
        } else {
            self.generic.reject(intent, message)?;
            Ok(self
                .generic
                .active_views()
                .into_iter()
                .map(present)
                .collect())
        }
    }
    pub(crate) fn has_codex_submission(&self, intent: &str) -> bool {
        self.generic.has_pending(intent) || self.content.idea_submission(intent).is_some()
    }
}
