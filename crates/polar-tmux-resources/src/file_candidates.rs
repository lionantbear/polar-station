//! Candidate reconstruction is pure; only the source Host can prove existence.
use crate::{parse_selected_file_reference, FileReference};
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
    crate::file_continuation::recover_display_candidates(
        &context.text,
        context.hit,
        crate::PathFlavor::Posix,
    )
}

pub fn file_open_candidates(value: &str) -> Vec<FileReference> {
    if value.len() > 65536 {
        return vec![];
    }
    if let Some(encoded) = value.strip_prefix(FILE_CONTEXT_PREFIX) {
        return serde_json::from_str(encoded)
            .ok()
            .map(|c| click_file_candidates(&c))
            .unwrap_or_default();
    }
    let mut result: Vec<_> = parse_selected_file_reference(value).into_iter().collect();
    if value.contains(['\r', '\n']) {
        let unpadded: String = value
            .split(['\r', '\n'])
            .map(|row| row.trim_matches(' '))
            .collect();
        if let Some(reference) = parse_selected_file_reference(&unpadded) {
            if !result.contains(&reference) {
                result.push(reference);
            }
        }
    }
    result
}

