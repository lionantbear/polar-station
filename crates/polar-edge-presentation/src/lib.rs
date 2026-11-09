//! Shared, pure edge projections for formatted content.
//!
//! The domain adapts one immutable source independently for compact displays
//! and natural speech. It owns no chat, connector, provider, device, or UI
//! lifecycle behavior.

use std::{error::Error, fmt};

mod content;
mod screen;
pub use content::{external_link, parse_content, ContentDocument, ContentKind, ContentNode};
pub use screen::{push_run, screen_runs, screen_runs_for_width, BackgroundRole, SourceSpan, TextRun, TextStyle};
use serde::{Deserialize, Serialize};

pub const MAXIMUM_SOURCE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionProfile {
    CompactEpaperV1,
    NaturalSpeechV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRequest {
    source_format: SourceFormat,
    profile: ProjectionProfile,
    source: String,
}

impl ProjectionRequest {
    pub fn new(
        source_format: SourceFormat,
        profile: ProjectionProfile,
        source: impl Into<String>,
    ) -> Self {
        Self {
            source_format,
            profile,
            source: source.into(),
        }
    }

    pub const fn source_format(&self) -> SourceFormat {
        self.source_format
    }

    pub const fn profile(&self) -> ProjectionProfile {
        self.profile
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedContent {
    text: String,
}

impl ProjectedContent {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    EmptySource,
    SourceTooLarge { bytes: usize, maximum: usize },
    EmptyProjection,
    ContentLimit,
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySource => write!(formatter, "projection source is empty"),
            Self::SourceTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "projection source is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::EmptyProjection => write!(formatter, "projection produced no text"),
            Self::ContentLimit => write!(formatter, "content structure exceeds budget"),
        }
    }
}

impl Error for ProjectionError {}

pub fn project(request: &ProjectionRequest) -> Result<ProjectedContent, ProjectionError> {
    if request.source.is_empty() {
        return Err(ProjectionError::EmptySource);
    }
    if request.source.len() > MAXIMUM_SOURCE_BYTES {
        return Err(ProjectionError::SourceTooLarge {
            bytes: request.source.len(),
            maximum: MAXIMUM_SOURCE_BYTES,
        });
    }

    let text = match (request.source_format, request.profile) {
        (SourceFormat::Markdown, ProjectionProfile::CompactEpaperV1) => {
            project_markdown(&request.source, ProjectionProfile::CompactEpaperV1)?
        }
        (SourceFormat::Markdown, ProjectionProfile::NaturalSpeechV1) => {
            project_markdown(&request.source, ProjectionProfile::NaturalSpeechV1)?
        }
    };
    if text.is_empty() {
        return Err(ProjectionError::EmptyProjection);
    }
    Ok(ProjectedContent { text })
}

pub fn project_content(document: &ContentDocument, profile: ProjectionProfile) -> ProjectedContent {
    let mut output = String::new();
    for node in &document.nodes {
        project_node(node, profile, &mut output);
    }
    ProjectedContent {
        text: match profile {
            ProjectionProfile::CompactEpaperV1 => normalize_display(output),
            ProjectionProfile::NaturalSpeechV1 => normalize_speech(output),
        },
    }
}

fn project_markdown(source: &str, profile: ProjectionProfile) -> Result<String, ProjectionError> {
    // The public caller has already checked the source budget.
    let document = parse_content(source, SourceFormat::Markdown)?;
    Ok(project_content(&document, profile).into_text())
}

fn project_node(node: &ContentNode, profile: ProjectionProfile, output: &mut String) {
    use ContentKind::*;
    if node.kind == Image
        || (node.kind == CodeBlock && profile == ProjectionProfile::NaturalSpeechV1)
    {
        return;
    }
    match node.kind {
        CodeBlock => ensure_display_break(output, 2),
        Item if profile == ProjectionProfile::CompactEpaperV1 => {
            ensure_display_break(output, 1);
            output.push_str("• ");
        }
        Text | Code => append_text(output, &node.text, profile),
        SoftBreak => match profile {
            ProjectionProfile::CompactEpaperV1 => ensure_display_break(output, 1),
            ProjectionProfile::NaturalSpeechV1 => append_collapsed_space(output),
        },
        HardBreak | Rule => match profile {
            ProjectionProfile::CompactEpaperV1 => ensure_display_break(output, 1),
            ProjectionProfile::NaturalSpeechV1 => push_speech_boundary(output),
        },
        _ => {}
    }
    for child in &node.children {
        project_node(child, profile, output);
    }
    match node.kind {
        Heading | Paragraph => match profile {
            ProjectionProfile::CompactEpaperV1 => ensure_display_break(output, 2),
            ProjectionProfile::NaturalSpeechV1 => push_speech_boundary(output),
        },
        Item | TableRow => match profile {
            ProjectionProfile::CompactEpaperV1 => ensure_display_break(output, 1),
            ProjectionProfile::NaturalSpeechV1 => push_speech_boundary(output),
        },
        List if profile == ProjectionProfile::CompactEpaperV1 => ensure_display_break(output, 2),
        TableCell if profile == ProjectionProfile::CompactEpaperV1 => output.push_str("  "),
        _ => {}
    }
}

fn append_text(output: &mut String, text: &str, profile: ProjectionProfile) {
    match profile {
        ProjectionProfile::CompactEpaperV1 => output.push_str(text),
        ProjectionProfile::NaturalSpeechV1 => {
            for part in text.split_inclusive(char::is_whitespace) {
                let trimmed = part.trim_end_matches(char::is_whitespace);
                output.push_str(trimmed);
                if trimmed.len() != part.len() {
                    append_collapsed_space(output);
                }
            }
        }
    }
}

fn append_collapsed_space(output: &mut String) {
    if !output.is_empty()
        && !output.ends_with(char::is_whitespace)
        && !output.ends_with(|character: char| is_terminal(character))
    {
        output.push(' ');
    }
}

fn ensure_display_break(output: &mut String, newline_count: usize) {
    while output.ends_with(' ') || output.ends_with('\t') {
        output.pop();
    }
    let existing = output
        .chars()
        .rev()
        .take_while(|character| *character == '\n')
        .count();
    for _ in existing..newline_count {
        output.push('\n');
    }
}

fn push_speech_boundary(output: &mut String) {
    while output.ends_with(char::is_whitespace) {
        output.pop();
    }
    if let Some(last) = output.chars().last() {
        if !is_terminal(last) {
            output.push('。');
        }
    }
}

fn is_terminal(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '；' | '：' | '!' | '?' | ';' | ':' | '.'
    )
}

fn normalize_display(output: String) -> String {
    output.trim_matches(char::is_whitespace).to_string()
}

fn normalize_speech(mut output: String) -> String {
    while output.ends_with(char::is_whitespace) {
        output.pop();
    }
    output
}

#[cfg(all(feature = "wasm-abi", target_arch = "wasm32"))]
mod wasm_abi {
    use super::{project, ProjectionError, ProjectionProfile, ProjectionRequest, SourceFormat};

    const ERROR_INVALID_ARGUMENT: u32 = 1;
    const ERROR_INVALID_UTF8: u32 = 2;
    const ERROR_EMPTY_SOURCE: u32 = 3;
    const ERROR_SOURCE_TOO_LARGE: u32 = 4;
    const ERROR_EMPTY_PROJECTION: u32 = 5;

    #[no_mangle]
    pub extern "C" fn edge_projection_alloc(length: u32) -> *mut u8 {
        let bytes = vec![0_u8; length as usize].into_boxed_slice();
        Box::into_raw(bytes).cast::<u8>()
    }

    /// # Safety
    ///
    /// `pointer` and `length` must describe a slice returned by
    /// `edge_projection_alloc` or by `edge_projection_project`, exactly once.
    #[no_mangle]
    pub unsafe extern "C" fn edge_projection_free(pointer: *mut u8, length: u32) {
        if pointer.is_null() {
            return;
        }
        let slice = std::ptr::slice_from_raw_parts_mut(pointer, length as usize);
        drop(Box::from_raw(slice));
    }

    /// Serialize the same content tree as native Rust. No resources are loaded.
    ///
    /// # Safety
    /// `pointer..pointer + length` must be readable Wasm memory for this call.
    #[no_mangle]
    pub unsafe extern "C" fn edge_content_parse(pointer: *const u8, length: u32) -> u64 {
        if pointer.is_null() {
            return u64::from(ERROR_INVALID_ARGUMENT);
        }
        if length as usize > super::MAXIMUM_SOURCE_BYTES {
            return u64::from(ERROR_SOURCE_TOO_LARGE);
        }
        let bytes = std::slice::from_raw_parts(pointer, length as usize);
        let Ok(source) = std::str::from_utf8(bytes) else {
            return u64::from(ERROR_INVALID_UTF8);
        };
        let document = match super::parse_content(source, SourceFormat::Markdown) {
            Ok(doc) => doc,
            Err(ProjectionError::EmptySource) => return u64::from(ERROR_EMPTY_SOURCE),
            Err(_) => return u64::from(ERROR_SOURCE_TOO_LARGE),
        };
        let Ok(output) = serde_json::to_vec(&document) else {
            return u64::from(ERROR_INVALID_ARGUMENT);
        };
        let length = output.len() as u32;
        let pointer = Box::into_raw(output.into_boxed_slice()).cast::<u8>() as u32;
        (u64::from(length) << 32) | u64::from(pointer)
    }

    /// Returns `(length << 32) | pointer`; a zero length carries an error code
    /// in the low 32 bits.
    ///
    /// # Safety
    ///
    /// `pointer..pointer + length` must be readable Wasm memory for this call.
    #[no_mangle]
    pub unsafe extern "C" fn edge_projection_project(
        pointer: *const u8,
        length: u32,
        source_format: u32,
        profile: u32,
    ) -> u64 {
        if pointer.is_null() || source_format != 1 {
            return u64::from(ERROR_INVALID_ARGUMENT);
        }
        let profile = match profile {
            1 => ProjectionProfile::CompactEpaperV1,
            2 => ProjectionProfile::NaturalSpeechV1,
            _ => return u64::from(ERROR_INVALID_ARGUMENT),
        };
        let source_bytes = std::slice::from_raw_parts(pointer, length as usize);
        let Ok(source) = std::str::from_utf8(source_bytes) else {
            return u64::from(ERROR_INVALID_UTF8);
        };
        let request = ProjectionRequest::new(SourceFormat::Markdown, profile, source);
        let text = match project(&request) {
            Ok(projected) => projected.into_text(),
            Err(ProjectionError::EmptySource) => return u64::from(ERROR_EMPTY_SOURCE),
            Err(ProjectionError::SourceTooLarge { .. }) => {
                return u64::from(ERROR_SOURCE_TOO_LARGE);
            }
            Err(ProjectionError::ContentLimit) => return u64::from(ERROR_SOURCE_TOO_LARGE),
            Err(ProjectionError::EmptyProjection) => {
                return u64::from(ERROR_EMPTY_PROJECTION);
            }
        };
        let output = text.into_bytes().into_boxed_slice();
        let output_length = output.len() as u32;
        let output_pointer = Box::into_raw(output).cast::<u8>() as u32;
        (u64::from(output_length) << 32) | u64::from(output_pointer)
    }
}
