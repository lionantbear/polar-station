//! Shared Dictate contracts. Background execution is opt-in for Host builds.
pub mod api;
pub use api::{Request, Response};
mod model;
pub mod store;
pub use model::*;
pub use store::DictateStore;
#[cfg(feature = "runtime")]
pub mod runtime;

mod settings;
pub use settings::{ShortcutMode, ShortcutModeChoice, TranscriptionMode, TranscriptionModeChoice, VoiceSettings};

mod audio;
pub use audio::pcm_wav;

#[cfg(feature = "runtime")]
mod transcription;

#[cfg(feature = "runtime")]
mod cleanup;

#[cfg(feature = "runtime")]
mod segmentation;
