//! File intent and resolution contracts. Native geometry is data, not policy.
use crate::{FileOrigin, FileReference, PathFlavor, ResourceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}
impl TextRange {
    pub fn contains(self, hit: usize) -> bool {
        self.start <= hit && hit < self.end
    }
    pub fn slice(self, text: &str) -> Option<&str> {
        (self.start < self.end)
            .then(|| text.get(self.start..self.end))
            .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayRowBoundary {
    /// Byte immediately after this physical row in the captured UTF-8 text.
    pub end: usize,
    pub soft_wrap: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileReferenceInput {
    NativeMatch {
        text: String,
        hit: usize,
        matched: TextRange,
        #[serde(default)]
        rows: Vec<DisplayRowBoundary>,
    },
    TerminalHit {
        text: String,
        hit: usize,
        #[serde(default)]
        rows: Vec<DisplayRowBoundary>,
    },
    Selection {
        text: String,
    },
}
impl FileReferenceInput {
    /// Compatibility decoder for the old local capture envelope only. It does
    /// not choose a source or parse a filesystem path.
    pub fn from_capture(text: String) -> Result<Self, FileOpenFailure> {
        if let Some(data) = text.strip_prefix(crate::FILE_CONTEXT_PREFIX) {
            if let Ok(input) = serde_json::from_str::<Self>(data) {
                input.validate()?;
                return Ok(input);
            }
            let legacy: crate::FileClickContext = serde_json::from_str(data).map_err(|_| {
                FileOpenFailure::new(
                    FileOpenStage::Capture,
                    "invalid_capture",
                    "The terminal file reference is unavailable.",
                )
            })?;
            let input = Self::TerminalHit {
                text: legacy.text,
                hit: legacy.hit,
                rows: vec![],
            };
            input.validate()?;
            Ok(input)
        } else {
            if text.trim().contains("://")
                && !text.trim().starts_with("file:")
                && !text.trim().starts_with('[')
            {
                return Err(FileOpenFailure::new(
                    FileOpenStage::Capture,
                    "not_a_file_reference",
                    "This link is handled by the terminal's URL handler.",
                ));
            }
            let input = Self::Selection { text };
            input.validate()?;
            Ok(input)
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Self::NativeMatch { text, .. }
            | Self::TerminalHit { text, .. }
            | Self::Selection { text } => text,
        }
    }
    pub fn validate(&self) -> Result<(), FileOpenFailure> {
        let text = self.text();
        if text.is_empty()
            || text.len() > 16384
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
        {
            return Err(FileOpenFailure::new(
                FileOpenStage::Parse,
                "invalid_input",
                "The file reference is empty, too large, or contains control characters.",
            ));
        }
        match self {
            Self::NativeMatch {
                hit, matched, rows, ..
            } => {
                if !matched.contains(*hit) || matched.slice(text).is_none() {
                    return Err(FileOpenFailure::new(
                        FileOpenStage::Capture,
                        "invalid_match",
                        "The terminal file match is unavailable.",
                    ));
                }
                Self::validate_hit(text, *hit, rows)
            }
            Self::TerminalHit { hit, rows, .. } => Self::validate_hit(text, *hit, rows),
            Self::Selection { .. } => Ok(()),
        }
    }
    fn validate_hit(
        text: &str,
        hit: usize,
        rows: &[DisplayRowBoundary],
    ) -> Result<(), FileOpenFailure> {
        if hit >= text.len()
            || !text.is_char_boundary(hit)
            || rows.len() > 32
            || rows.windows(2).any(|v| v[0].end >= v[1].end)
            || rows
                .iter()
                .any(|r| r.end > text.len() || !text.is_char_boundary(r.end))
        {
            Err(FileOpenFailure::new(
                FileOpenStage::Capture,
                "invalid_hit",
                "The terminal file position is unavailable.",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedFileSource {
    /// Reuses the Host target registry configuration revision, never UI state.
    #[serde(default)]
    pub target_configuration_revision: Option<u64>,
    pub origin: FileOrigin,
    pub flavor: PathFlavor,
    pub home: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedFile {
    pub source: ResolvedFileSource,
    pub reference: FileReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileAccessPlan {
    NativeLocal {
        local_path: String,
    },
    MappedReady {
        resource_id: ResourceId,
        local_path: String,
    },
    StartExistingMapping {
        resource_id: ResourceId,
    },
    NeedMapping {
        execution_target_id: String,
        remote_directory: String,
        source_path: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOpenStage {
    Capture,
    Source,
    Parse,
    Verify,
    Access,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOpenFailure {
    pub stage: FileOpenStage,
    pub code: String,
    pub message: String,
}
impl FileOpenFailure {
    pub fn new(stage: FileOpenStage, code: &str, message: impl Into<String>) -> Self {
        Self {
            stage,
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileOpenResolution {
    Resolved {
        file: ResolvedFile,
        access_plan: FileAccessPlan,
    },
    Choices {
        source: ResolvedFileSource,
        files: Vec<FileReference>,
    },
    Failure {
        failure: FileOpenFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFileReference {
    pub range: TextRange,
    pub reference: FileReference,
}
