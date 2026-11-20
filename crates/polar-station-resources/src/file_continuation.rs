//! Reconstruct only the adjoining text around a hit. No path candidates or I/O.
use crate::{DisplayRowBoundary, PathFlavor, TextRange};

pub(crate) struct LogicalText {
    pub text: String,
    pub hit: usize,
    offsets: Vec<usize>,
    pub continued: bool,
    pub first_row: usize,
    pub last_row: usize,
}
impl LogicalText {
    pub fn ranges(&self, range: TextRange) -> Vec<TextRange> {
        let mut result: Vec<TextRange> = Vec::new();
        for &offset in &self.offsets[range.start..range.end] {
            if let Some(last) = result.last_mut().filter(|r| r.end == offset) {
                last.end += 1;
            } else {
                result.push(TextRange {
                    start: offset,
                    end: offset + 1,
                });
            }
        }
        result
    }
}

struct Row<'a> {
    start: usize,
    text: &'a str,
    soft: bool,
}

/// Native soft wraps are authoritative. Hard rows use adjoining reference
/// boundaries; no permutations of first/last rows and no original-message search.
pub(crate) fn reconstruct(
    text: &str,
    hit: usize,
    boundaries: &[DisplayRowBoundary],
    selection: bool,
    flavor: PathFlavor,
) -> Option<LogicalText> {
    let mut start = 0;
    let mut boundary = boundaries.iter().peekable();
    let rows: Vec<_> = text
        .split_inclusive('\n')
        .map(|part| {
            let end = start + part.len();
            while boundary.peek().is_some_and(|b| b.end < end) {
                boundary.next();
            }
            let soft = boundary.peek().is_some_and(|b| b.end == end && b.soft_wrap);
            let row = Row {
                start,
                text: part.trim_end_matches(['\r', '\n']),
                soft,
            };
            start = end;
            row
        })
        .collect();
    let clicked = rows
        .iter()
        .position(|r| r.start <= hit && hit < r.start + r.text.len())?;
    let (mut first, mut last) = (clicked, clicked);
    while first > 0 && (selection || connects(&rows[first - 1], &rows[first], flavor)) {
        first -= 1;
    }
    while last + 1 < rows.len() && (selection || connects(&rows[last], &rows[last + 1], flavor)) {
        last += 1;
    }
    let mut joined = String::new();
    let mut offsets = Vec::new();
    for index in first..=last {
        let row = &rows[index];
        let mut part = row.text;
        // Only display-edge whitespace is removed. Preserve row-internal spaces,
        // and all spaces on a known soft wrap.
        if index > first && !rows[index - 1].soft {
            part = part.trim_start_matches([' ', '\t']);
        }
        if index < last && !row.soft {
            part = part.trim_end_matches([' ', '\t']);
        }
        let begin = part.as_ptr() as usize - row.text.as_ptr() as usize;
        joined.push_str(part);
        offsets.extend((0..part.len()).map(|i| row.start + begin + i));
    }
    let mapped_hit = offsets.iter().position(|&p| p == hit)?;
    Some(LogicalText {
        text: joined,
        hit: mapped_hit,
        offsets,
        continued: first != last,
        first_row: first + 1,
        last_row: last + 1,
    })
}

fn connects(previous: &Row<'_>, next: &Row<'_>, flavor: PathFlavor) -> bool {
    if previous.soft {
        return true;
    }
    let left = previous.text.trim_end();
    let right = next.text.trim();
    if left.trim().is_empty() || right.is_empty() || independent_row(right) {
        return false;
    }
    // A trailing outer delimiter closes the reference; a balanced pair inside
    // a filename does not. The lexer makes that distinction on the physical row.
    if let Some((last, _)) = left.char_indices().next_back() {
        if let Some(found) = crate::file_matcher::reference_at(left, last, flavor) {
            if left.starts_with(['\'', '"', '`', '[']) && found.range.end == left.len() {
                return false;
            }
        } else if left.ends_with([')', ']', '"', '\'', '`']) {
            return false;
        }
    }
    if completes_parenthesized_reference(left, right, flavor) {
        return true;
    }
    // Ordinary multiword prose is a paragraph boundary. A filename can have
    // spaces, so a path-shaped trailing word remains part of the same chain.
    let last_word = right.split_whitespace().next_back().unwrap_or(right);
    if right.split_whitespace().count() > 1 && !last_word.contains(['/', '\\', '.']) {
        return false;
    }
    true
}

fn completes_parenthesized_reference(left: &str, right: &str, flavor: PathFlavor) -> bool {
    let mut depth = 0;
    let opening = left.char_indices().rev().find_map(|(index, c)| match c {
        ')' => {
            depth += 1;
            None
        }
        '(' if depth == 0 => Some(index),
        '(' => {
            depth -= 1;
            None
        }
        _ => None,
    });
    let Some(opening) = opening else { return false };
    let prefix = &left[opening + 1..];
    if prefix.is_empty() {
        return false;
    }
    depth = 0;
    let closing = right.char_indices().find_map(|(index, c)| match c {
        '(' => {
            depth += 1;
            None
        }
        ')' if depth == 0 => Some(index),
        ')' => {
            depth -= 1;
            None
        }
        _ => None,
    });
    let Some(closing) = closing.filter(|index| *index > 0) else { return false };
    let joined = format!("{prefix}{}", &right[..closing]);
    crate::file_matcher::reference_at(&joined, 0, flavor)
        .is_some_and(|found| found.range.end == joined.len())
}
fn independent_row(text: &str) -> bool {
    if text.starts_with(['•', '│', '>'])
        || text.starts_with("# ")
        || text.starts_with("- ")
        || text.starts_with("* ")
        || text.starts_with("$ ")
    {
        return true;
    }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && (text[digits..].starts_with(". ") || text[digits..].starts_with(") "))
}
