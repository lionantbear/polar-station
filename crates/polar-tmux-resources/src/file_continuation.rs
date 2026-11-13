//! Display continuation recovery only. Representation parsing is a separate
//! operation; no Cartesian row combinations and no filesystem access.
use crate::{file_text::reference_at, parse_source_reference, FileReference, PathFlavor};

pub(crate) fn recover_display_candidates(
    text: &str,
    hit: usize,
    flavor: PathFlavor,
) -> Vec<FileReference> {
    if text.len() > 16384
        || hit >= text.len()
        || !text.is_char_boundary(hit)
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n'))
    {
        return vec![];
    }
    let rows: Vec<_> = text
        .split_inclusive('\n')
        .scan(0usize, |offset, row| {
            let start = *offset;
            *offset += row.len();
            Some((start, row.trim_end_matches(['\r', '\n'])))
        })
        .collect();
    let Some(index) = rows
        .iter()
        .position(|(start, row)| *start <= hit && hit < start + row.len())
    else {
        return vec![];
    };
    let mut result = Vec::new();
    let exact = reference_at(text, hit, flavor);
    if let Some(found) = &exact {
        result.push(found.reference.clone());
        // A closing wrapper, or further content on the same physical row,
        // bounds the reference. Neighboring list items cannot extend it.
        let row_end = rows[index].0 + rows[index].1.len();
        if found.range.end < row_end && !text[found.range.end..row_end].trim().is_empty() {
            return result;
        }
        let start = found.range.start;
        if text
            .as_bytes()
            .get(start)
            .is_some_and(|b| matches!(b, b'\'' | b'"' | b'`' | b'['))
        {
            return result;
        }
    }
    let row = rows[index].1;
    if row.contains("://") && !row.trim_start().starts_with("file:") {
        return vec![];
    }
    let mut first = index;
    let mut last = index;
    let mut begin = exact.as_ref().map_or_else(
        || row.len() - row.trim_start_matches(' ').len(),
        |m| m.range.start.saturating_sub(rows[index].0),
    );
    if starts_root(row.trim_start_matches(' ')) {
        begin = row.len() - row.trim_start_matches(' ').len();
    }
    // Walk one adjoining chain backwards, stopping at a real path prefix or
    // a display boundary. The first row may have prose before its final token.
    while first > index.saturating_sub(8) {
        let current = &rows[first].1[begin..];
        if (first < index && starts_root(current))
            || begin > rows[first].1.len() - rows[first].1.trim_start_matches(' ').len()
        {
            break;
        }
        let previous = rows[first - 1].1;
        if display_boundary(previous) {
            break;
        }
        let trimmed = previous.trim_end_matches(' ');
        let start = trimmed
            .rfind(|c: char| c.is_whitespace() || matches!(c, '(' | '[' | '`' | '"' | '\'' | '│'))
            .map_or(0, |i| i + trimmed[i..].chars().next().unwrap().len_utf8());
        if start == trimmed.len() {
            break;
        }
        first -= 1;
        begin = start;
    }
    // A continuation begins at the next row's first content cell. List items,
    // empty rows and closed reference delimiters terminate the chain.
    while last + 1 < rows.len().min(index + 9) {
        let current = rows[last].1.trim_end_matches(' ');
        if current.ends_with([')', ']', '`', '"', '\'']) || display_boundary(rows[last + 1].1) {
            break;
        }
        let next = rows[last + 1].1.trim();
        if next.chars().any(char::is_whitespace) && !next.contains(['/', '.', '\\']) {
            break;
        }
        last += 1;
    }
    if first == last {
        return result;
    }
    for strip_padding in [false, true] {
        let joined: String = rows[first..=last]
            .iter()
            .enumerate()
            .map(|(i, (_, row))| {
                let part = if i == 0 { &row[begin..] } else { *row };
                if strip_padding {
                    part.trim_matches(' ')
                } else {
                    part
                }
            })
            .collect();
        if let Some(reference) = parse_source_reference(joined.trim(), flavor) {
            if !result.contains(&reference) {
                result.push(reference);
            }
        }
    }
    result
}

fn starts_root(text: &str) -> bool {
    text.starts_with('/')
        || text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with("~/")
        || text.starts_with("file:")
}
fn display_boundary(row: &str) -> bool {
    let row = row.trim();
    row.is_empty()
        || row.starts_with(['•', '│', '>', '#'])
        || row.starts_with("- ")
        || row.starts_with("* ")
        || row.starts_with("$ ")
        || row.contains("://")
}
