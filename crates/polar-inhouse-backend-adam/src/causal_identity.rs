//! Adam-native causal identity for provider attempts and tool invocations.
//!
//! The identity root is the existing Polar/Adam turn contract. These records
//! never enter provider-visible prompts or tool results.

use crate::projection_store::AdamTurnAttempt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamCausalIdentityMode {
    Legacy,
    #[default]
    CausalV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamToolInvocationStatus {
    IntentRecorded,
    Completed,
    Failed,
    Rejected,
    Reused,
}

impl AdamToolInvocationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IntentRecorded => "intent_recorded",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Reused => "reused",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::IntentRecorded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamToolInvocationEvidence {
    pub invocation_id: String,
    pub session_id: String,
    pub request_message_id: String,
    pub turn_attempt_id: String,
    pub provider_attempt_ref: Option<String>,
    pub provider_response_id: Option<String>,
    pub provider_call_id: Option<String>,
    pub wire_call_id: String,
    pub call_ordinal: u32,
    pub tool_name: String,
    pub arguments_hash: String,
    pub status: AdamToolInvocationStatus,
    pub reused_from_invocation_id: Option<String>,
    pub polar_tool_call_ref: Option<String>,
    pub polar_tool_result_ref: Option<String>,
    pub error_summary: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl AdamToolInvocationEvidence {
    pub fn settle(
        &self,
        status: AdamToolInvocationStatus,
        reused_from_invocation_id: Option<String>,
        polar_tool_call_ref: Option<String>,
        polar_tool_result_ref: Option<String>,
        error_summary: Option<String>,
        updated_at_unix_ms: i64,
    ) -> Self {
        Self {
            status,
            reused_from_invocation_id,
            polar_tool_call_ref,
            polar_tool_result_ref,
            error_summary,
            updated_at_unix_ms,
            ..self.clone()
        }
    }
}

pub fn adam_tool_invocation_id(
    session_id: &str,
    request_message_id: &str,
    turn_attempt_id: &str,
    call_ordinal: u32,
) -> String {
    let value = serde_json::json!({
        "session_id": session_id,
        "request_message_id": request_message_id,
        "turn_attempt_id": turn_attempt_id,
        "call_ordinal": call_ordinal,
    });
    format!(
        "adam_tool_invocation_{}",
        hex_sha256(&serde_json::to_vec(&value).unwrap_or_default())
    )
}

pub fn adam_tool_arguments_hash(arguments: &Value) -> String {
    format!(
        "sha256:{}",
        hex_sha256(&serde_json::to_vec(arguments).unwrap_or_default())
    )
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamCausalGradeFailure {
    pub code: String,
    pub invocation_id: Option<String>,
    pub detail: String,
}

impl AdamCausalGradeFailure {
    fn new(code: &str, invocation_id: Option<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            invocation_id,
            detail: detail.into(),
        }
    }
}

pub fn grade_adam_causal_evidence(
    attempts: &[AdamTurnAttempt],
    writes: &[AdamToolInvocationEvidence],
) -> Result<(), AdamCausalGradeFailure> {
    let attempts_by_id = attempts
        .iter()
        .map(|attempt| (attempt.attempt_id.as_str(), attempt))
        .collect::<HashMap<_, _>>();
    let mut first_by_invocation = HashMap::<&str, &AdamToolInvocationEvidence>::new();
    let mut terminal_ids = HashSet::<&str>::new();
    let mut ordinals = HashSet::<(&str, u32)>::new();

    for write in writes {
        let attempt = attempts_by_id
            .get(write.turn_attempt_id.as_str())
            .ok_or_else(|| {
                AdamCausalGradeFailure::new(
                    "UNKNOWN_TURN_ATTEMPT",
                    Some(write.invocation_id.clone()),
                    &write.turn_attempt_id,
                )
            })?;
        if write.session_id != attempt.session_id
            || write.request_message_id != attempt.request_message_id
        {
            return Err(AdamCausalGradeFailure::new(
                "ATTEMPT_IDENTITY_MISMATCH",
                Some(write.invocation_id.clone()),
                "session or request_message_id differs from turn attempt",
            ));
        }
        if write.provider_attempt_ref != attempt.provider_attempt_ref {
            return Err(AdamCausalGradeFailure::new(
                "PROVIDER_ATTEMPT_REF_MISMATCH",
                Some(write.invocation_id.clone()),
                "tool evidence does not join the durable provider attempt",
            ));
        }
        if write.provider_response_id.as_deref() == Some(write.turn_attempt_id.as_str())
            || write.provider_response_id.as_deref() == write.provider_attempt_ref.as_deref()
        {
            return Err(AdamCausalGradeFailure::new(
                "FORGED_PROVIDER_RESPONSE_ID",
                Some(write.invocation_id.clone()),
                "provider response identity was copied from a local attempt identity",
            ));
        }
        if attempt.provider_response_id.is_some()
            && write.provider_response_id != attempt.provider_response_id
        {
            return Err(AdamCausalGradeFailure::new(
                "PROVIDER_RESPONSE_ID_MISMATCH",
                Some(write.invocation_id.clone()),
                "tool evidence response differs from completed attempt",
            ));
        }
        if write.provider_call_id.as_deref() == Some(write.wire_call_id.as_str())
            && is_synthetic_wire_fallback(&write.wire_call_id)
        {
            return Err(AdamCausalGradeFailure::new(
                "FORGED_PROVIDER_CALL_ID",
                Some(write.invocation_id.clone()),
                "provider-style wire fallback has no raw provider provenance",
            ));
        }

        match first_by_invocation.get(write.invocation_id.as_str()) {
            None => {
                if write.invocation_id
                    != adam_tool_invocation_id(
                        &write.session_id,
                        &write.request_message_id,
                        &write.turn_attempt_id,
                        write.call_ordinal,
                    )
                {
                    return Err(AdamCausalGradeFailure::new(
                        "INTERNAL_CALL_ID_MISMATCH",
                        Some(write.invocation_id.clone()),
                        "invocation id is not derived from its Adam turn identity and ordinal",
                    ));
                }
                if write.status != AdamToolInvocationStatus::IntentRecorded {
                    return Err(AdamCausalGradeFailure::new(
                        "ORPHAN_TOOL_TERMINAL",
                        Some(write.invocation_id.clone()),
                        "first write is not intent_recorded",
                    ));
                }
                if !ordinals.insert((write.turn_attempt_id.as_str(), write.call_ordinal)) {
                    return Err(AdamCausalGradeFailure::new(
                        "DUPLICATE_CALL_ORDINAL",
                        Some(write.invocation_id.clone()),
                        "call ordinal reused inside one turn attempt",
                    ));
                }
                first_by_invocation.insert(write.invocation_id.as_str(), write);
            }
            Some(intent) => {
                if !write.status.is_terminal() {
                    return Err(AdamCausalGradeFailure::new(
                        "INTERNAL_CALL_ID_REUSED",
                        Some(write.invocation_id.clone()),
                        "two intents reuse one invocation id",
                    ));
                }
                if !terminal_ids.insert(write.invocation_id.as_str()) {
                    return Err(AdamCausalGradeFailure::new(
                        "DUPLICATE_TOOL_TERMINAL",
                        Some(write.invocation_id.clone()),
                        "invocation contains multiple terminal writes",
                    ));
                }
                if intent.wire_call_id != write.wire_call_id {
                    return Err(AdamCausalGradeFailure::new(
                        "WIRE_CALL_ID_MISMATCH",
                        Some(write.invocation_id.clone()),
                        "terminal wire call id differs from intent",
                    ));
                }
                if !same_immutable_identity(intent, write) {
                    return Err(AdamCausalGradeFailure::new(
                        "TOOL_IDENTITY_CHANGED",
                        Some(write.invocation_id.clone()),
                        "terminal identity differs from intent",
                    ));
                }
                if write.updated_at_unix_ms < intent.created_at_unix_ms {
                    return Err(AdamCausalGradeFailure::new(
                        "INVALID_TOOL_EVENT_ORDER",
                        Some(write.invocation_id.clone()),
                        "terminal precedes intent",
                    ));
                }
            }
        }
    }

    if let Some((invocation_id, _)) = first_by_invocation
        .iter()
        .find(|(invocation_id, _)| !terminal_ids.contains(**invocation_id))
    {
        return Err(AdamCausalGradeFailure::new(
            "MISSING_TOOL_TERMINAL",
            Some((*invocation_id).to_string()),
            "tool intent has no terminal evidence",
        ));
    }

    for write in writes
        .iter()
        .filter(|write| write.status == AdamToolInvocationStatus::Reused)
    {
        let source = write.reused_from_invocation_id.as_deref().ok_or_else(|| {
            AdamCausalGradeFailure::new(
                "REUSE_SOURCE_MISSING",
                Some(write.invocation_id.clone()),
                "reused terminal has no source invocation",
            )
        })?;
        if !terminal_ids.contains(source) {
            return Err(AdamCausalGradeFailure::new(
                "REUSE_SOURCE_UNKNOWN",
                Some(write.invocation_id.clone()),
                source,
            ));
        }
    }
    Ok(())
}

fn is_synthetic_wire_fallback(wire_call_id: &str) -> bool {
    wire_call_id == "openai-tool-call"
        || wire_call_id == "chat-tool-call"
        || wire_call_id == "responses-tool-call"
        || wire_call_id.starts_with("adam-wire-fallback-")
}

fn same_immutable_identity(
    intent: &AdamToolInvocationEvidence,
    terminal: &AdamToolInvocationEvidence,
) -> bool {
    intent.session_id == terminal.session_id
        && intent.request_message_id == terminal.request_message_id
        && intent.turn_attempt_id == terminal.turn_attempt_id
        && intent.provider_attempt_ref == terminal.provider_attempt_ref
        && intent.provider_response_id == terminal.provider_response_id
        && intent.provider_call_id == terminal.provider_call_id
        && intent.wire_call_id == terminal.wire_call_id
        && intent.call_ordinal == terminal.call_ordinal
        && intent.tool_name == terminal.tool_name
        && intent.arguments_hash == terminal.arguments_hash
}


