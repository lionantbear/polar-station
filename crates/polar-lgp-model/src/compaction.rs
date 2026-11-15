use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

/// Inclusive canonical LGP source range summarized by a compaction record.
///
/// Both endpoints are stable LGP refs such as `lgp:message:msg-1`; raw backend
/// transcript text is intentionally not part of this span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSourceSpan {
    pub start_source_ref: String,
    pub end_source_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTriggerKind {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompactionSummaryQuality {
    Accepted,
    Rejected { reasons: Vec<String> },
}

/// Canonical readable compaction truth for an LGP thread.
///
/// Provider-native opaque compaction artifacts and physical cache/slot state
/// belong in backend extensions and are not represented here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompactionRecord {
    pub compaction_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub source_spans: Vec<CompactionSourceSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_compaction_ref: Option<String>,
    pub summary_text: String,
    pub first_kept_message_id: String,
    pub tokens_before: i64,
    pub tokens_after_estimate: i64,
    pub context_compaction_epoch: i64,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCompactionRecordError {
    reason: String,
}

impl ContextCompactionRecordError {
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ContextCompactionRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid canonical LGP compaction record: {}",
            self.reason
        )
    }
}

impl Error for ContextCompactionRecordError {}

impl ContextCompactionRecord {
    /// Enforces the readable, LGP-addressed invariants required before this
    /// value can be admitted as canonical compaction truth.
    pub fn validate(&self) -> Result<(), ContextCompactionRecordError> {
        for (field, value) in [
            ("compaction_id", self.compaction_id.as_str()),
            ("thread_id", self.thread_id.as_str()),
            ("summary_text", self.summary_text.as_str()),
            ("first_kept_message_id", self.first_kept_message_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(invalid_record(format!("{field} must not be blank")));
            }
        }
        if self.source_spans.is_empty() {
            return Err(invalid_record("source_spans must not be empty"));
        }
        for span in &self.source_spans {
            if !is_lgp_message_ref(&span.start_source_ref)
                || !is_lgp_message_ref(&span.end_source_ref)
            {
                return Err(invalid_record(
                    "source span endpoints must be stable lgp:message: refs",
                ));
            }
        }
        if self
            .previous_compaction_ref
            .as_deref()
            .is_some_and(|source_ref| !source_ref.starts_with("lgp:compaction:"))
        {
            return Err(invalid_record(
                "previous_compaction_ref must be an lgp:compaction: ref",
            ));
        }
        if self.tokens_before < 0 || self.tokens_after_estimate < 0 {
            return Err(invalid_record("token counts must not be negative"));
        }
        if self.tokens_after_estimate > self.tokens_before {
            return Err(invalid_record(
                "tokens_after_estimate must not exceed tokens_before",
            ));
        }
        if self.context_compaction_epoch <= 0 {
            return Err(invalid_record("context_compaction_epoch must be positive"));
        }
        if self.created_at_unix_ms < 0 {
            return Err(invalid_record("created_at_unix_ms must not be negative"));
        }
        Ok(())
    }
}

fn is_lgp_message_ref(value: &str) -> bool {
    value.trim().starts_with("lgp:message:")
}

fn invalid_record(reason: impl Into<String>) -> ContextCompactionRecordError {
    ContextCompactionRecordError {
        reason: reason.into(),
    }
}
