//! Preview action semantics shared by both native renderers.
use super::{Entry, Phase, valid_text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewAction {
    InsertOriginal,
    InsertRefined,
    SendOriginal,
    SendRefined,
}
impl PreviewAction {
    pub fn refines(self) -> bool {
        matches!(self, Self::InsertRefined | Self::SendRefined)
    }
    pub fn sends(self) -> bool {
        matches!(self, Self::SendOriginal | Self::SendRefined)
    }
}

pub(super) fn failure_copy(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "voice.audio_connection_timeout" => Some((
            "Audio connection timed out",
            "The audio connection could not be established.",
        )),
        "voice.audio_connection_lost" => Some((
            "Audio connection closed",
            "The audio connection closed before the recording was sent.",
        )),
        "voice.final_transcript_timeout" => Some((
            "Final transcript timed out",
            "Audio was sent. The final text did not arrive.",
        )),
        "voice.timeout" => Some((
            "Recording processing timed out",
            "The voice service did not finish in time.",
        )),
        _ => None,
    }
}

pub fn dismissible(entry: &Entry) -> bool {
    entry.phase == Phase::Failed && !valid_text(&entry.text) && entry.saved_samples > 0
}
pub fn actions(
    entry: &Entry,
    chosen_mode: Option<polar_dictate::DictateOutputMode>,
    enabled: bool,
) -> serde_json::Value {
    let reason = if entry.phase == Phase::Submitting {
        "Sending is in progress"
    } else if entry.inserted {
        "Already inserted; use the target's Send control"
    } else if entry.phase == Phase::Delivering {
        "Insertion is in progress"
    } else if entry.phase == Phase::Failed {
        "Complete transcription is unavailable; retry the saved recording"
    } else {
        "Finish the active operation before using this result"
    };
    let refine = if chosen_mode.or(entry.output_mode)
        == Some(polar_dictate::DictateOutputMode::Transcreate)
    {
        "Transcreate"
    } else {
        "Refine"
    };
    serde_json::json!([
        {"id":"copy","icon":"copy","label":"Copy","enabled":valid_text(&entry.text),"disabled_reason":"No preview text yet"},
        {"id":"insert_original","icon":"insert","label":"Skip and Insert","enabled":enabled,"disabled_reason":reason},
        {"id":"insert_refined","icon":"refine_insert","label":format!("{refine} and Insert"),"enabled":enabled,"disabled_reason":reason},
        {"id":"send_original","icon":"send","label":"Skip and Send · ⌘Enter / Super+Enter","enabled":enabled,"disabled_reason":reason},
        {"id":"send_refined","icon":"refine_send","label":format!("{refine} and Send"),"enabled":enabled,"disabled_reason":reason}
    ])
}
