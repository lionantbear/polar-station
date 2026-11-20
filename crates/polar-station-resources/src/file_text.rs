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

/// Only external URI matches may fall through to Ghostty after Rust file recognition.
pub fn is_external_file_link(text: &str) -> bool {
    url::Url::parse(text).is_ok_and(|uri| uri.scheme() != "file" && uri.scheme().len() > 1)
}

/// Compatibility projection: a clicked reference is never a candidate list.
pub fn parse_file_input(
    input: &FileReferenceInput,
    flavor: PathFlavor,
) -> Result<Vec<FileReference>, FileOpenFailure> {
    Ok(vec![recognize_file_input(input, flavor)?.reference])
}

/// Pure, singular recognition shared by hover, activation and legacy ingress.
pub fn recognize_file_input(
    input: &FileReferenceInput,
    flavor: PathFlavor,
) -> Result<crate::RecognizedFileReference, FileOpenFailure> {
    recognize(input, flavor, None)
}

fn recognize(
    input: &FileReferenceInput,
    flavor: PathFlavor,
    mut trace: Option<&mut Option<Box<crate::file_input::FileRecognitionTrace>>>,
) -> Result<crate::RecognizedFileReference, FileOpenFailure> {
    input.validate()?;
    if let FileReferenceInput::Recognized {
        recognized,
        flavor: supplied,
        ..
    } = input
    {
        if *supplied != flavor {
            return Err(FileOpenFailure::new(
                FileOpenStage::Parse,
                "source_syntax_changed",
                "The source path syntax changed.",
            ));
        }
        return Ok(recognized.clone());
    }
    let missing = || {
        FileOpenFailure::new(
            FileOpenStage::Parse,
            "file_reference_missing",
            "No complete file reference was found at this position.",
        )
    };
    let (text, hit, rows, selection) = match input {
        FileReferenceInput::NativeMatch {
            text, hit, rows, ..
        }
        | FileReferenceInput::TerminalHit { text, hit, rows } => {
            (text.as_str(), *hit, rows.as_slice(), false)
        }
        FileReferenceInput::Selection { text } => (
            text.as_str(),
            text.len() - text.trim_start().len(),
            &[][..],
            true,
        ),
        FileReferenceInput::Recognized { .. } => unreachable!(),
    };
    if let Some(trace) = trace.as_mut() {
        **trace = Some(Box::new(crate::file_input::FileRecognitionTrace {
            hit,
            rows: rows.to_vec(),
            logical: None,
        }));
    }
    let logical = crate::file_continuation::reconstruct(text, hit, rows, selection, flavor)
        .ok_or_else(missing)?;
    if let Some(trace) = trace.and_then(|trace| trace.as_mut()) {
        trace.logical = Some(crate::file_input::FileLogicalText {
            text: logical.text.clone(),
            hit: logical.hit,
            first_row: logical.first_row,
            last_row: logical.last_row,
        });
    }
    let parsed = if selection {
        let value = logical.text.trim();
        let start = logical.text.len() - logical.text.trim_start().len();
        ParsedFileReference {
            reference: crate::file_matcher::exact(value, flavor).ok_or_else(missing)?,
            range: TextRange {
                start,
                end: start + value.len(),
            },
        }
    } else {
        crate::file_matcher::reference_at(&logical.text, logical.hit, flavor).ok_or_else(missing)?
    };
    if !parsed.range.contains(logical.hit) {
        return Err(missing());
    }
    Ok(crate::RecognizedFileReference {
        reference: parsed.reference,
        ranges: logical.ranges(parsed.range),
        continued: logical.continued,
    })
}

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

/// Carry the exact shared result to Host; no native parsing and no second scan.
pub fn recognize_file_capture(
    input: FileReferenceInput,
    flavor: PathFlavor,
) -> Result<FileReferenceInput, FileOpenFailure> {
    if matches!(input, FileReferenceInput::Recognized { .. }) {
        recognize_file_input(&input, flavor)?;
        return Ok(input);
    }
    let mut trace = None;
    let recognized = match recognize(&input, flavor, Some(&mut trace)) {
        Ok(value) => value,
        Err(mut failure) => {
            failure.recognition_trace = trace;
            return Err(failure);
        }
    };
    let hit = match &input {
        FileReferenceInput::NativeMatch { hit, .. }
        | FileReferenceInput::TerminalHit { hit, .. }
        | FileReferenceInput::Recognized { hit, .. } => *hit,
        FileReferenceInput::Selection { .. } => recognized.ranges[0].start,
    };
    Ok(FileReferenceInput::Recognized {
        text: input.text().into(),
        hit,
        flavor,
        recognized,
        trace,
    })
}


