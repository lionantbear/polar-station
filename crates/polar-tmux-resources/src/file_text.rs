//! Extract a reference range from text. Filesystem syntax is delegated to
//! SourcePath, Markdown to pulldown-cmark, and URIs to the existing parser.
use crate::{
    FileOpenFailure, FileOpenStage, FileReference, FileReferenceInput, ParsedFileReference,
    PathFlavor, SourcePath, TextRange,
};
use pulldown_cmark::{Event, Parser, Tag};

pub fn parse_source_reference(text: &str, flavor: PathFlavor) -> Option<FileReference> {
    let value = text.trim();
    let markdown_target = if value.starts_with('[') || value.starts_with("![") {
        Parser::new(value).find_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                Some(dest_url.into_string())
            }
            _ => None,
        })
    } else {
        None
    };
    let decoded_target = markdown_target
        .as_deref()
        .filter(|v| !v.starts_with("file:"))
        .map(|v| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8()
                .map(|s| s.into_owned())
        })
        .transpose()
        .ok()?;
    let represented = decoded_target
        .as_deref()
        .or(markdown_target.as_deref())
        .unwrap_or(value);
    let mut reference =
        crate::file_reference::parse_file_reference_for_flavor(represented, flavor)?;
    let uri_text = represented.trim_matches(['\'', '"', '`']);
    if flavor == PathFlavor::Windows && uri_text.starts_with("file:") {
        let uri = url::Url::parse(uri_text).ok()?;
        if let Some(host) = uri.host_str().filter(|h| *h != "localhost") {
            reference.path = format!(r"\\{}{}", host, reference.path.replace('/', r"\"));
        } else if reference.path.starts_with('/') && reference.path.as_bytes().get(2) == Some(&b':')
        {
            reference.path.remove(0);
        }
    }
    SourcePath::new(flavor, &reference.path).ok()?;
    Some(reference)
}

/// Pure parsing/reconstruction after the source adapter has declared its syntax.
pub fn parse_file_input(
    input: &FileReferenceInput,
    flavor: PathFlavor,
) -> Result<Vec<FileReference>, FileOpenFailure> {
    input.validate()?;
    let logical = logical_capture(input);
    let input = logical.as_ref().unwrap_or(input);
    let mut refs = match input {
        FileReferenceInput::NativeMatch {
            text, hit, matched, ..
        } => {
            let mut refs: Vec<_> = parse_reference_range(text, *matched, flavor)
                .map(|r| r.reference)
                .into_iter()
                .collect();
            for r in crate::file_continuation::recover_display_candidates(text, *hit, flavor) {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
            refs
        }
        FileReferenceInput::TerminalHit { text, hit, .. } => {
            crate::file_continuation::recover_display_candidates(text, *hit, flavor)
        }
        FileReferenceInput::Selection { text } => {
            let joined: String = text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
            let mut refs: Vec<_> = parse_source_reference(&joined, flavor)
                .into_iter()
                .collect();
            if text.contains(['\r', '\n']) {
                let unpadded: String = text.lines().map(|r| r.trim_matches(' ')).collect();
                if let Some(r) = parse_source_reference(&unpadded, flavor) {
                    if !refs.contains(&r) {
                        refs.push(r);
                    }
                }
            }
            refs
        }
    };
    refs.dedup();
    if refs.is_empty() {
        return Err(FileOpenFailure::new(
            FileOpenStage::Parse,
            "file_reference_missing",
            "No file reference was found at this position.",
        ));
    }
    if refs.len() > crate::MAX_FILE_CANDIDATES
        || refs.iter().map(|r| r.path.len()).sum::<usize>() > crate::MAX_FILE_CANDIDATE_BYTES
    {
        return Err(FileOpenFailure::new(
            FileOpenStage::Parse,
            "file_reference_limit",
            "The file reference is too large; select the complete path.",
        ));
    }
    Ok(refs)
}

/// The exact native match uses this same representation parser. This does not
/// expand its range or inspect neighboring rows.
pub fn parse_reference_range(
    text: &str,
    range: TextRange,
    flavor: PathFlavor,
) -> Option<ParsedFileReference> {
    Some(ParsedFileReference {
        range,
        reference: parse_source_reference(range.slice(text)?, flavor)?,
    })
}

pub(crate) fn reference_at(
    text: &str,
    hit: usize,
    flavor: PathFlavor,
) -> Option<ParsedFileReference> {
    if hit >= text.len() || !text.is_char_boundary(hit) {
        return None;
    }
    // OffsetIter retains the original text range; decoded Markdown targets are
    // not guessed from an outer prose prefix or from rendered labels.
    for (event, range) in Parser::new(text).into_offset_iter() {
        if !range.contains(&hit) {
            continue;
        }
        let reference = match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                let target = if dest_url.starts_with("file:") {
                    dest_url.into_string()
                } else if dest_url.contains("://") {
                    continue;
                } else {
                    percent_encoding::percent_decode_str(&dest_url)
                        .decode_utf8()
                        .ok()?
                        .into_owned()
                };
                parse_source_reference(&target, flavor)
            }
            Event::Code(value) if file_shaped(&value, flavor) => {
                parse_source_reference(&value, flavor)
            }
            _ => None,
        };
        if let Some(reference) = reference {
            return Some(ParsedFileReference {
                range: TextRange {
                    start: range.start,
                    end: range.end,
                },
                reference,
            });
        }
    }
    let start = text[..hit].rfind('\n').map_or(0, |i| i + 1);
    let end = text[hit..].find('\n').map_or(text.len(), |i| hit + i);
    let line = &text[start..end];
    let mut cursor = 0;
    while cursor < line.len() {
        let c = line[cursor..].chars().next()?;
        if matches!(c, '"' | '\'' | '`') {
            let after = cursor + c.len_utf8();
            if let Some(length) = line[after..].find(c) {
                let stop = after + length + c.len_utf8();
                let range = TextRange {
                    start: start + cursor,
                    end: start + stop,
                };
                if range.contains(hit) {
                    return parse_reference_range(text, range, flavor);
                }
                cursor = stop;
                continue;
            }
        }
        if boundary(c) {
            cursor += c.len_utf8();
            continue;
        }
        let begin = cursor;
        let mut depth = 0usize;
        while cursor < line.len() {
            let c = line[cursor..].chars().next()?;
            if c == '(' {
                depth += 1;
            } else if c == ')' && depth > 0 {
                depth -= 1;
            } else if boundary(c) {
                break;
            }
            cursor += c.len_utf8();
        }
        let range = TextRange {
            start: start + begin,
            end: start + cursor,
        };
        if range.contains(hit) && file_shaped(&line[begin..cursor], flavor) {
            return parse_reference_range(text, range, flavor);
        }
    }
    None
}

fn boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '<' | '>' | '"' | '\'' | '`' | '[' | ']' | '(' | ')' | ',' | ';' | '│'
        )
}
fn file_shaped(text: &str, flavor: PathFlavor) -> bool {
    text.contains('/')
        || text.contains('.')
        || text.starts_with('~')
        || (flavor == PathFlavor::Windows && text.contains(['\\', ':']))
        || matches!(
            text,
            "README" | "LICENSE" | "Makefile" | "Dockerfile" | "justfile" | "Justfile"
        )
}

fn logical_capture(input: &FileReferenceInput) -> Option<FileReferenceInput> {
    let (text, hit, matched, rows) = match input {
        FileReferenceInput::NativeMatch {
            text,
            hit,
            matched,
            rows,
        } => (text, *hit, Some(*matched), rows),
        FileReferenceInput::TerminalHit { text, hit, rows } => (text, *hit, None, rows),
        _ => return None,
    };
    let mut remove = vec![false; text.len()];
    for row in rows.iter().filter(|r| r.soft_wrap) {
        let end = row.end;
        if end > 0 && text.as_bytes()[end - 1] == b'\n' {
            remove[end - 1] = true;
            if end > 1 && text.as_bytes()[end - 2] == b'\r' {
                remove[end - 2] = true;
            }
        }
    }
    if !remove.iter().any(|v| *v) {
        return None;
    }
    let offset = |old: usize| old - remove[..old].iter().filter(|v| **v).count();
    let value = String::from_utf8(
        text.bytes()
            .enumerate()
            .filter_map(|(i, b)| (!remove[i]).then_some(b))
            .collect(),
    )
    .ok()?;
    let hit = offset(hit);
    Some(match matched {
        Some(range) => FileReferenceInput::NativeMatch {
            text: value,
            hit,
            matched: TextRange {
                start: offset(range.start),
                end: offset(range.end),
            },
            rows: vec![],
        },
        None => FileReferenceInput::TerminalHit {
            text: value,
            hit,
            rows: vec![],
        },
    })
}


