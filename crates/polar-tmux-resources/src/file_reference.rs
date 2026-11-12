//! Filesystem references, independent of file types and native text geometry.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReference {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<FileLocation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLocation {
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

/// Explicit file-open intents may contain terminal display line breaks. This
/// never changes the terminal selection or the strict parser used for links.
pub fn parse_selected_file_reference(value: &str) -> Option<FileReference> {
    if value.len() > 16384 {
        return None;
    }
    let joined: String = value.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    parse_file_reference(&joined)
}

/// Parse one candidate selected by the native link engine or by the user.
/// URI authorities never select an execution target.
pub fn parse_file_reference(value: &str) -> Option<FileReference> {
    if value.len() > 16384 || value.chars().any(char::is_control) {
        return None;
    }
    let mut value = value.trim();
    let markdown = value.starts_with('[') || value.starts_with("![");
    if markdown {
        let (_, target) = value.split_once("](")?;
        value = target.strip_suffix(')')?.trim();
    }
    let literal = value.starts_with(['\'', '"']);
    if let Some(first) = value
        .chars()
        .next()
        .filter(|c| matches!(c, '\'' | '"' | '`'))
    {
        value = value.strip_prefix(first)?.strip_suffix(first)?;
    }
    if value.is_empty() {
        return None;
    }
    let (path, location) = if value.starts_with("file:") {
        let uri = url::Url::parse(value).ok()?;
        if uri.scheme() != "file" || uri.query().is_some() {
            return None;
        }
        let location = match uri.fragment() {
            Some(value) => Some(parse_anchor(value)?),
            None => None,
        };
        let path = percent_encoding::percent_decode_str(uri.path())
            .decode_utf8()
            .ok()?
            .into_owned();
        (path, location)
    } else {
        if value.contains("://") {
            return None;
        }
        let (path, location) = if literal {
            (value, None)
        } else {
            split_location(value)?
        };
        let path = if markdown {
            percent_encoding::percent_decode_str(path)
                .decode_utf8()
                .ok()?
                .into_owned()
        } else {
            path.to_owned()
        };
        (path, location)
    };
    if path.is_empty() || path.chars().any(char::is_control) || path.contains("://") {
        return None;
    }
    // A leading URI scheme is not a filesystem path. Colons inside explicit
    // POSIX paths and quoted filenames remain literal.
    if !path.contains('/') && !literal && path.contains(':') {
        return None;
    }
    // Candidate detection belongs to the native link rules. A user-selected
    // exact filename can be extensionless or contain spaces; parsing must not
    // turn a type/filename list into an admission whitelist.
    Some(FileReference { path, location })
}

fn positive(value: &str) -> Option<u32> {
    let n = value.parse::<u32>().ok()?;
    (n > 0).then_some(n)
}
fn parse_anchor(value: &str) -> Option<FileLocation> {
    let value = value.strip_prefix('L')?;
    if let Some((line, column)) = value.split_once('C') {
        Some(FileLocation {
            line: positive(line)?,
            column: Some(positive(column)?),
        })
    } else {
        Some(FileLocation {
            line: positive(value)?,
            column: None,
        })
    }
}
fn split_location(value: &str) -> Option<(&str, Option<FileLocation>)> {
    if let Some((path, anchor)) = value.rsplit_once('#') {
        if anchor.starts_with('L') && anchor.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
            return Some((path, Some(parse_anchor(anchor)?)));
        }
    }
    if let Some((path, last)) = value.rsplit_once(':') {
        if !last.is_empty() && last.bytes().all(|v| v.is_ascii_digit()) {
            let n = positive(last)?;
            if let Some((path, line)) = path.rsplit_once(':') {
                if !line.is_empty() && line.bytes().all(|v| v.is_ascii_digit()) {
                    return Some((
                        path,
                        Some(FileLocation {
                            line: positive(line)?,
                            column: Some(n),
                        }),
                    ));
                }
            }
            return Some((
                path,
                Some(FileLocation {
                    line: n,
                    column: None,
                }),
            ));
        }
    }
    Some((value, None))
}

/// Oniguruma patterns consumed by Ghostty's existing link matcher. URL/OSC8
/// precedence and cell geometry stay in Ghostty. These are not per-file-type.
pub fn file_link_rules() -> &'static [&'static str] {
    &[
        r#"!?\[[^\]\r\n]*\]\((?:file:[^\r\n)]+|(?!(?:[[:alpha:]][[:alnum:]+.-]*):[^0-9])[^\r\n)]+)\)"#,
        r#"(["'`])(?:/|\./|\.\./)[^\r\n]*?\1"#,
        r#"file://[^\s<>"'`()\[\],;]*[^\s<>"'`()\[\],;.]"#,
        r#"(?<![^\s\[('"`])(?:/|\./|\.\./|[[:alnum:]_.-]+/)[^\s<>"'`()\[\],;]*[^\s<>"'`()\[\],;.!?:]"#,
        r#"(?<![^\s\[('"`])(?:[[:alnum:]_+-][[:alnum:]_+.-]*\.[[:alpha:]][[:alnum:]_+-]*|\.[[:alpha:]_][[:alnum:]_.-]*|README|LICENSE|Makefile|Dockerfile|Justfile|justfile)(?::[0-9]+(?::[0-9]+)?)?(?:#L[0-9]+(?:C[0-9]+)?)?(?![[:alnum:]_/@])"#,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenStrategy {
    #[default]
    SystemDefault,
    Reveal,
}
/// Optional exceptions to the OS association, not an admission whitelist.
pub struct FileTypeRule<'a> {
    pub extensions: &'a [&'a str],
    pub file_names: &'a [&'a str],
    pub strategy: OpenStrategy,
}
pub fn select_opener(reference: &FileReference, rules: &[FileTypeRule<'_>]) -> OpenStrategy {
    let path = std::path::Path::new(&reference.path);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    rules
        .iter()
        .find(|r| {
            r.file_names.contains(&name)
                || r.extensions
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(extension))
        })
        .map(|r| r.strategy)
        .unwrap_or_default()
}


