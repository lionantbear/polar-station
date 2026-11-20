//! Shared history rows; the domain that owns the draft also owns its restore data.
use serde::{Deserialize, Serialize};

pub const MAX_ENTRIES: usize = 20;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Sent, Failed, Unknown }

#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub intent: String,
    pub preview: String,
    pub status: &'static str,
    pub action: &'static str,
}

impl Row {
    pub fn new(intent: &str, text: &str, status: &'static str, action: &'static str) -> Self {
        let preview: String = text.split_whitespace()
            .flat_map(|word| word.chars().chain(std::iter::once(' ')))
            .take(60).collect();
        Self { intent: intent.into(), preview: preview.trim_end().into(), status, action }
    }

    pub fn completed(intent: &str, text: &str, outcome: Outcome, can_restore: bool) -> Self {
        let status = match outcome {
            Outcome::Sent => "Sent",
            Outcome::Failed => "Failed",
            Outcome::Unknown => "Result unknown",
        };
        Self::new(intent, text, status, if outcome == Outcome::Failed && can_restore { "Restore" } else { "Copy" })
    }
}
