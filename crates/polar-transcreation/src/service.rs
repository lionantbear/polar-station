use crate::{EditKind, EditRequest, EditResult, ExecutionConfig};
use anyhow::{ensure, Result};
use polar_inhouse_contract::{InhouseAgentRunner, InhouseAgentTurnOutcome};

pub async fn edit(
    runner: &mut dyn InhouseAgentRunner,
    input: EditRequest,
    config: ExecutionConfig,
) -> Result<EditResult> {
    edit_in_session(runner, &mut EditSession::default(), input, config).await
}

/// Recording-scoped context, never shared across independent dictations.
#[derive(Default)]
pub struct EditSession {
    scope: Option<String>,
    backend_session: Option<String>,
    history: Vec<polar_inhouse_contract::InhouseCanonicalThreadMessage>,
}

impl EditSession {
    pub fn bind_scope(&mut self, scope: &str) {
        if self.scope.as_deref() != Some(scope) {
            *self = Self {
                scope: Some(scope.to_owned()),
                ..Default::default()
            };
        }
    }
    pub fn release_scope(&mut self, scope: &str) {
        if self.scope.as_deref() == Some(scope) {
            *self = Self::default();
        }
    }
}

pub async fn edit_in_session(
    runner: &mut dyn InhouseAgentRunner,
    session: &mut EditSession,
    input: EditRequest,
    config: ExecutionConfig,
) -> Result<EditResult> {
    // Retain the existing hard text budget even when the provider cannot report
    // an exact preflight token count. Returning to the base keeps fixed policy
    // unchanged without pretending a byte count is an exact token measurement.
    if session.history.iter().map(|m| m.text.len()).sum::<usize>() + input.original.len()
        > config.max_text_bytes.min(crate::policy::MAX_TEXT_BYTES)
    {
        session.history.clear();
        session.backend_session = None;
    }
    let mut prepared = crate::policy::prepare_with_history(&input, &config, &session.history)?;
    if input.original.trim().is_empty()
        && matches!(
            input.edit,
            EditKind::CleanupTranscript { .. } | EditKind::CleanupTranslateEnglish { .. }
        )
    {
        return Ok(EditResult {
            operation_id: input.operation_id,
            base_revision: input.base_revision,
            text: String::new(),
        });
    }
    let current_message = prepared.options.canonical_thread_messages.last().cloned();
    // Include preflight in the deadline. Dropping this future cannot insert text.
    let output = tokio::time::timeout(config.timeout, async {
        prepared.options.context_budget = runner
            .preflight_context_budget(
                &prepared.input,
                &prepared.options,
                config.context_window_tokens,
                config.reserve_tokens,
            )
            .await?;
        if let Some(budget) = prepared.options.context_budget {
            ensure!(
                budget.exact_prompt_tokens <= budget.effective_input_budget_tokens,
                "text.edit_context_limit"
            );
        }
        runner
            .run_turn_streaming_outcome(
                session.backend_session.clone(),
                prepared.input,
                prepared.options,
                None,
                None,
                None,
            )
            .await
    })
    .await??;
    let (backend_session, text) = match output {
        InhouseAgentTurnOutcome::ImmediateOutput(output) if output.incomplete.is_none() => {
            (output.session_id, output.reply_text)
        }
        InhouseAgentTurnOutcome::Immediate {
            session_id,
            reply_text,
        } => (session_id, reply_text),
        _ => anyhow::bail!("text.edit_incomplete"),
    };
    let text = text.trim().to_owned();
    ensure!(
        !text.is_empty()
            && text.len() <= config.max_text_bytes.min(crate::policy::MAX_TEXT_BYTES)
            && !text.contains('\0'),
        "text.edit_invalid_result"
    );
    session.backend_session = Some(backend_session);
    if let Some(message) = current_message {
        session.history.push(message);
    }
    session
        .history
        .push(polar_inhouse_contract::InhouseCanonicalThreadMessage {
            message_id: format!("{}:result", input.operation_id),
            role: "assistant".into(),
            text: text.clone(),
        });
    Ok(EditResult {
        operation_id: input.operation_id,
        base_revision: input.base_revision,
        text,
    })
}

/// Prepare execution resources without editing or adding history.
pub async fn prepare_execution(
    runner: &mut dyn InhouseAgentRunner,
    turn_ref: &polar_inhouse_contract::InhouseBackendTurnRef,
) -> Result<polar_inhouse_contract::InhousePreparationOutcome> {
    runner.prepare_execution(turn_ref).await
}
