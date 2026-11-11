//! Product choices shared by capture, recovery, history and both native UIs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictateOutputMode {
    NoRefine,
    #[default]
    Refine,
    Transcreate,
}
impl DictateOutputMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoRefine => "No refine",
            Self::Refine => "Refine",
            Self::Transcreate => "Transcreate",
        }
    }
    pub fn refines(self) -> bool {
        self != Self::NoRefine
    }
    pub fn edit(self, context: Option<String>) -> Option<polar_transcreation::EditKind> {
        match self {
            Self::NoRefine => None,
            Self::Refine => Some(polar_transcreation::RefineMode::Cleanup.edit(context)),
            Self::Transcreate => Some(polar_transcreation::RefineMode::English.edit(context)),
        }
    }
    pub fn choices() -> serde_json::Value {
        serde_json::json!([
            {"mode":"no_refine","label":"No refine","description":"Use the original transcript"},
            {"mode":"refine","label":"Refine","description":"Clean up in the original language"},
            {"mode":"transcreate","label":"Transcreate","description":"Clean up and translate into English"}
        ])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    pub enabled: bool,
    pub days: u32,
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            days: 7,
        }
    }
}
impl RetentionPolicy {
    pub fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.days > 0, "dictate.retention_days_must_be_positive");
        Ok(())
    }
    pub fn duration_ms(self) -> u64 {
        u64::from(self.days) * 86_400_000
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CleanupReport {
    pub eligible: u64,
    pub audio_bytes: u64,
    pub protected: u64,
    pub deleted: u64,
    pub next_at_ms: Option<u64>,
}

impl CleanupReport {
    pub fn presentation(&self) -> serde_json::Value {
        let audio = if self.audio_bytes >= 1_048_576 {
            format!("{:.1} MB", self.audio_bytes as f64 / 1_048_576.0)
        } else {
            format!("{} bytes", self.audio_bytes)
        };
        serde_json::json!({"title":"Clean up by retention rules","summary":format!("{} completed records are eligible ({audio} of audio data). {} protected records will be kept.",self.eligible,self.protected),
            "confirmation":"Delete eligible history and audio?","detail":"This removes only records eligible under your retention rule. Pending, unresolved, uncertain and pinned records are kept. Text already inserted into an input is unchanged.",
            "confirm_label":"Clean up eligible records","cancel_label":"Cancel","can_apply":self.eligible>0,
            "result":format!("Cleaned up {} records. {} protected records were kept.",self.deleted,self.protected)})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupPlan {
    pub policy: RetentionPolicy,
    pub as_of_ms: u64,
}
