//! Shared Dictate contracts. Background execution is opt-in for Host builds.
pub mod api;
pub use api::{Request, Response};
mod model;
pub mod store;
pub mod inspection;
pub use inspection::{InputDetails, InputSnapshot, InputSummary};
pub use model::*;
pub use store::DictateStore;
#[cfg(feature = "runtime")]
pub mod runtime;
#[cfg(feature = "runtime")]
mod input_runtime;

mod settings;
pub use settings::{
    DEFAULT_REALTIME_INSTRUCTION, ReferencePolicy, ShortcutMode, ShortcutModeChoice, TranscriptionMode, TranscriptionModeChoice, VoiceSettings,
};

mod audio;
pub use audio::pcm_wav;

#[cfg(feature = "runtime")]
mod transcription;

#[cfg(feature = "runtime")]
mod cleanup;

#[cfg(feature = "runtime")]
mod segmentation;

mod policy;
pub use policy::{CleanupPlan, CleanupReport, DictateOutputMode, RetentionPolicy};

pub mod history;
pub use history::{
    DictateEvent, HistoryCursor, HistoryItem, HistoryKind, HistoryPage, HistorySummary,
    ImportReport, ItemChange, LegacyInput, TextEditRecord,
};

pub use polar_transcreation::RefineMode;
