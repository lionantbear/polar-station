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
    let text = &context.text;
    if text.len() > 16384
        || context.hit >= text.len()
        || !text.is_char_boundary(context.hit)
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n'))
    {
        return vec![];
    }
    let lines: Vec<(usize, &str)> = text
        .split_inclusive('\n')
        .scan(0, |offset, row| {
            let start = *offset;
            *offset += row.len();
            Some((start, row.trim_end_matches(['\r', '\n'])))
        })
        .collect();
    let Some(hit_row) = lines
        .iter()
        .position(|(start, row)| context.hit >= *start && context.hit <= start + row.len())
    else {
        return vec![];
    };
    let mut result = vec![];
    for first in hit_row.saturating_sub(8)..=hit_row {
        for last in hit_row..lines.len().min(hit_row + 9) {
            if lines[first..=last]
                .iter()
                .any(|(_, row)| row.trim().is_empty())
            {
                continue;
            }
            let (start, first_row) = lines[first];
            let starts: Vec<usize> = std::iter::once(0)
                .chain(first_row.char_indices().filter_map(|(i, c)| {
                    (i > 0
                        && matches!(c, '/' | '.' | '"' | '\'' | '`' | '[')
                        && first_row[..i].ends_with([' ', '\t', '(']))
                    .then_some(i)
                }))
                .collect();
            for begin in starts {
                if first == hit_row && start + begin > context.hit {
                    continue;
                }
                // Both literal spaces and terminal padding are candidates. The
                // source filesystem, not a whitespace guess, chooses the file.
                for trim_padding in [false, true] {
                    let mut joined = String::new();
                    let mut joined_hit = None;
                    for (index, (row_start, row)) in lines[first..=last].iter().enumerate() {
                        let base = if index == 0 { begin } else { 0 };
                        let raw = &row[base..];
                        let part = if trim_padding {
                            raw.trim_matches(' ')
                        } else {
                            raw
                        };
                        let leading = raw.len() - raw.trim_start_matches(' ').len();
                        let removed = if trim_padding { leading } else { 0 };
                        if first + index == hit_row {
                            let offset = context.hit.saturating_sub(row_start + base + removed);
                            if context.hit >= row_start + base + removed && offset < part.len() {
                                joined_hit = Some(joined.len() + offset);
                            }
                        }
                        joined.push_str(part);
                    }
                    let Some(hit) = joined_hit else {
                        continue;
                    };
                    let mut endings = vec![joined.len()];
                    for (i, c) in joined.char_indices() {
                        if i > hit && matches!(c, ' ' | ')' | ']' | ',' | ';' | '│') {
                            endings.push(i);
                        }
                    }
                    for end in endings {
                        let raw = &joined[..end];
                        let value = raw.trim();
                        let leading = raw.len() - raw.trim_start().len();
                        if hit < leading || hit >= leading + value.len() {
                            continue;
                        }
                        if value.contains("://") && !value.starts_with("file:") {
                            continue;
                        }
                        if !value.contains('/')
                            && !value.contains('.')
                            && !matches!(
                                value,
                                "README"
                                    | "LICENSE"
                                    | "Makefile"
                                    | "Dockerfile"
                                    | "justfile"
                                    | "Justfile"
                            )
                        {
                            continue;
                        }
                        if let Some(reference) = parse_selected_file_reference(value) {
                            if !result.contains(&reference) {
                                result.push(reference);
                            }
                        }
                        if result.len() > MAX_FILE_CANDIDATES
                            || result.iter().map(|r| r.path.len()).sum::<usize>()
                                > MAX_FILE_CANDIDATE_BYTES
                        {
                            return vec![];
                        }
                    }
                }
            }
        }
    }
    result
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
    parse_selected_file_reference(value).into_iter().collect()
}

