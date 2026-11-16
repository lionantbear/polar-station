//! Actual execution inputs. These are history, never instructions for execution.
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct InputSnapshot {
    pub request_id: String,
    pub stage: String,
    pub channel: String,
    pub status: String,
    pub content: serde_json::Value,
}
impl InputSnapshot {
    pub fn key(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(format!(
                "{}\0{}\0{}",
                self.request_id, self.stage, self.channel
            ))
        )
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.request_id.is_empty()
                && self.request_id.len() <= 256
                && matches!(self.stage.as_str(), "transcribe" | "refine" | "sources")
                && !self.channel.is_empty()
                && self.channel.len() <= 128
                && matches!(
                    self.status.as_str(),
                    "prepared" | "submitted" | "failed" | "unsupported" | "not_collected"
                ),
            "dictate.invalid_input_snapshot"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputSummary {
    pub key: String,
    pub request_id: String,
    pub stage: String,
    pub channel: String,
    pub status: String,
    pub created_ms: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InputDetails {
    pub entries: Vec<InputSummary>,
    pub selected: Option<InputSnapshot>,
    pub error: Option<String>,
}

impl std::fmt::Debug for InputDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputDetails")
            .field("entries", &self.entries.len())
            .field("error", &self.error)
            .finish()
    }
}
