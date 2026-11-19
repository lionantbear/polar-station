//! Candidate reconstruction is pure; only the source Host can prove existence.
use crate::{FileReference, FileReferenceInput, PathFlavor};
use serde::{Deserialize, Serialize};
pub const FILE_CONTEXT_PREFIX: &str = "polar-station-file-context:";
pub const MAX_FILE_CANDIDATES: usize = 128;
pub const MAX_FILE_CANDIDATE_BYTES: usize = 65536;
#[derive(Debug, Serialize, Deserialize)]
pub struct FileClickContext {
    pub text: String,
    pub hit: usize,
}

/// Native text includes hard row separators and a byte hit from Ghostty's pin map.
/// Search both directions; no dependency on which fragment contains an extension.
pub fn click_file_candidates(context: &FileClickContext) -> Vec<FileReference> {
    crate::parse_file_input(
        &FileReferenceInput::TerminalHit {
            text: context.text.clone(),
            hit: context.hit,
            rows: vec![],
        },
        PathFlavor::Posix,
    )
    .unwrap_or_default()
}

pub fn file_open_candidates(value: &str) -> Vec<FileReference> {
    FileReferenceInput::from_capture(value.into())
        .ok()
        .and_then(|input| crate::parse_file_input(&input, PathFlavor::Posix).ok())
        .unwrap_or_default()
}

