//! Shared Rust text boundary matcher. Ghostty 332b2aef src/config/url.zig is
//! the behavioral baseline (MIT, Copyright 2024 Mitchell Hashimoto and Ghostty
//! contributors; license retained in the existing Ghostty third-party bundle).
//! Regex identifies word forms; a forward scan owns delimiters and whitespace.
use crate::{parse_source_reference, FileReference, ParsedFileReference, PathFlavor, TextRange};
use pulldown_cmark::{Event, Parser, Tag};
use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn reference_at(
    text: &str,
    hit: usize,
    flavor: PathFlavor,
) -> Option<ParsedFileReference> {
    if hit >= text.len() || !text.is_char_boundary(hit) {
        return None;
    }
    for (event, range) in Parser::new(text).into_offset_iter() {
        if !range.contains(&hit) {
            continue;
        }
        let reference = match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                if dest_url.contains("://") && !dest_url.starts_with("file:") {
                    return None;
                }
                let value = if dest_url.starts_with("file:") {
                    dest_url.into_string()
                } else {
                    percent_encoding::percent_decode_str(&dest_url)
                        .decode_utf8()
                        .ok()?
                        .into_owned()
                };
                parse_source_reference(&value, flavor)
            }
            Event::Code(value) if file_shaped(&value, flavor) => {
                parse_source_reference(&value, flavor)
            }
            _ => None,
        };
        if let Some(reference) = reference {
            return Some(ParsedFileReference {
                reference,
                range: TextRange {
                    start: range.start,
                    end: range.end,
                },
            });
        }
    }
    bare_at(text, hit, flavor)
}

fn bare_at(text: &str, hit: usize, flavor: PathFlavor) -> Option<ParsedFileReference> {
    let mut cursor = 0;
    while cursor < text.len() {
        let c = text[cursor..].chars().next()?;
        if matches!(c, '\'' | '"' | '`') {
            let after = cursor + c.len_utf8();
            if let Some(end) = text[after..].find(c).map(|i| after + i + c.len_utf8()) {
                if (cursor..end).contains(&hit) {
                    let reference = parse_source_reference(&text[cursor..end], flavor)?;
                    return file_shaped(&reference.path, flavor).then_some(ParsedFileReference {
                        reference,
                        range: TextRange { start: cursor, end },
                    });
                }
                cursor = end;
                continue;
            }
        }
        if boundary(c) {
            cursor += c.len_utf8();
            continue;
        }
        let begin = cursor;
        let mut depth = 0usize;
        while cursor < text.len() {
            let c = text[cursor..].chars().next()?;
            if c == '(' {
                depth += 1;
            } else if c == ')' && depth > 0 {
                depth -= 1;
            } else if c == ' ' && space_continues(&text[begin..cursor], &text[cursor + 1..]) {
            } else if boundary(c) {
                break;
            }
            cursor += c.len_utf8();
        }
        // A URI is a single token, including its internal slash characters.
        let token = text[begin..cursor].trim_end_matches([',', '.', ';', '!', '?']);
        let end = begin + token.len();
        if (begin..end).contains(&hit) {
            let reference = parse_source_reference(token, flavor)?;
            if !file_shaped(&reference.path, flavor) {
                return None;
            }
            return Some(ParsedFileReference {
                reference,
                range: TextRange { start: begin, end },
            });
        }
    }
    None
}
fn space_continues(prefix: &str, rest: &str) -> bool {
    if !prefix.contains(['/', '\\']) || prefix.contains("://") {
        return false;
    }
    // A complete file-like final component ends at unquoted whitespace.
    // Otherwise two neighboring references would become one path. Explicit
    // quotes still preserve arbitrary spaces, including dotted names.
    if prefix
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.contains('.'))
    {
        return false;
    }
    let next = rest
        .split(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '"' | '\'' | '`' | '<' | '>'))
        .next()
        .unwrap_or("");
    if next.is_empty()
        || next.contains("://")
        || next.starts_with(['/', '\\'])
        || next.starts_with("./")
        || next.starts_with("../")
    {
        return false;
    }
    // Ghostty's dotted path rule consumes another segment only when it looks
    // like a path segment, not ordinary following prose.
    next.contains(['/', '\\', '.'])
}
fn boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '<' | '>' | '"' | '\'' | '`' | '[' | ']' | '(' | ')' | ',' | ';' | '│'
        )
}
pub(crate) fn file_shaped(text: &str, flavor: PathFlavor) -> bool {
    if text.starts_with("file:") {
        return true;
    }
    if text.contains("://") || text.contains('@') && !text.contains(['/', '\\']) {
        return false;
    }
    if flavor == PathFlavor::Windows
        && (text.contains('\\') || text.as_bytes().get(1) == Some(&b':'))
    {
        return true;
    }
    static SHAPE: OnceLock<Regex> = OnceLock::new();
    let regex = SHAPE.get_or_init(|| Regex::new(r"(?x)^(?: / | \.{1,2}/ | ~/ | [\w_.+-]+/ | [\w+-][\w.+-]*\.[\p{L}_] | \.[\p{L}_] | (?:README|LICENSE|Makefile|Dockerfile|Justfile|justfile)$ )").expect("static path shape"));
    if !regex.is_match(text) {
        return false;
    }
    let numeric = text.strip_prefix('v').unwrap_or(text);
    !numeric
        .split('.')
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

pub(crate) fn exact(text: &str, flavor: PathFlavor) -> Option<FileReference> {
    parse_source_reference(text, flavor)
}
