//! One native dispatch envelope, retaining the existing content wire format.
use crate::generic::{Destination, GenericView, Lease};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Input {
    Generic(GenericInput),
    Content(polar_station_lgp::Input),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenericInput {
    ReferenceCollection { enabled:bool },
    DictatedInput { lease:Lease, source:crate::learning::Refinement, text:String },
    RefinedInput { lease:Lease, original:String, text:String, english:bool },
    LearningStored { intent:String, success:bool },
    SelectTerminal {
        window_id: String,
        pane_id: String,
        target_id: String,
        session_id: String,
    },
    SessionCompleted {
        request_id: String,
        #[serde(default)]
        value: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<String>,
    },
    QuoteSelection {
        lease: Lease,
        text: String,
    },
    RefreshInput {
        lease: Lease,
    },
    SubmitInput {
        lease: Lease,
        press_enter: bool,
    },
    SnapshotStored {
        revision: u64,
        success: bool,
    },
    ObserveInput {
        window_id: String,
        pane_id: String,
        draft_key: String,
        destination: Destination,
    },
    EditInput {
        lease: Lease,
        text: String,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Effect {
    Generic(GenericEffect),
    Content(Box<polar_station_lgp::Effect>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GenericEffect {
    LearnRefine { sample:crate::learning::LearningSample },
    InsertText {
        lease: Lease,
        text: String,
    },
    SessionRequest {
        request: Box<crate::terminal::SessionJob>,
    },
    InputBar {
        view: Box<GenericView>,
    },
}
