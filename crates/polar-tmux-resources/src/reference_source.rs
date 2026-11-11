//! Recover terminal fragments from canonical source references, never screen rows.
use crate::{parse_file_reference, resolve_reference_path, FileReference};
use pulldown_cmark::{Event, Parser, Tag};
use std::collections::BTreeSet;

pub fn source_file_references(markdown: &str) -> Vec<FileReference> {
    Parser::new(markdown)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                // Preserve the existing parser's Markdown-destination decoding;
                // a bare POSIX path deliberately keeps literal percent signs.
                parse_file_reference(&format!("[]({dest_url})"))
            }
            // A command/prose span is not evidence of a filename. Whitespace
            // paths need an explicit quoted literal or Markdown destination.
            Event::Code(text)
                if !text.chars().any(char::is_whitespace) || text.starts_with(['\'', '"']) =>
            {
                parse_file_reference(&text)
            }
            _ => None,
        })
        .collect()
}

/// Search visible original messages by filename, including encoded URI targets.
/// Returned snippets still require exact full-item retrieval before parsing.
pub fn source_file_search_terms(fragment: &str) -> Vec<String> {
    let name = fragment.rsplit('/').next().unwrap_or(fragment);
    let ascii_tail = name
        .rsplit(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-'))
        .next()
        .unwrap_or(name);
    let tail = if ascii_tail.is_empty() {
        name.rsplit(|c: char| !c.is_alphanumeric() && !matches!(c, '.' | '_' | '-'))
            .next()
            .unwrap_or(name)
    } else {
        ascii_tail
    };
    let mut terms = Vec::new();
    for part in [name, tail].into_iter().filter(|part| !part.is_empty()) {
        if !terms.iter().any(|term| term == part) {
            terms.push(part.to_owned());
        }
        if let Ok(uri) = url::Url::from_file_path(std::path::Path::new("/").join(part)) {
            let encoded = uri.path().trim_start_matches('/');
            if !terms.iter().any(|term| term == encoded) {
                terms.push(encoded.to_owned());
            }
        }
    }
    terms
}

pub fn complete_source_file_reference(
    fragment: &str,
    cwd: Option<&str>,
    references: &[FileReference],
) -> Result<Option<String>, String> {
    if fragment.is_empty() || fragment.chars().any(char::is_control) {
        return Err("Invalid source file reference".into());
    }
    let exact = resolve_reference_path(
        &FileReference {
            path: fragment.into(),
            location: None,
        },
        cwd,
    )
    .ok();
    let mut matches = BTreeSet::new();
    for reference in references {
        let Ok(path) = resolve_reference_path(reference, cwd) else {
            continue;
        };
        let Some(value) = path.to_str() else { continue };
        if value.ends_with(fragment) || exact.as_ref() == Some(&path) {
            matches.insert(value.to_owned());
        }
    }
    if matches.len() > 1 {
        return Err("This reference matches multiple files in the original Thread. Select the complete path.".into());
    }
    Ok(matches.pop_first())
}


