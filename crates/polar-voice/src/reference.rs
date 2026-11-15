//! Ephemeral recognition reference data. Callers select its source; Voice never
//! reads a conversation, editor or glossary store and never treats it as speech.
use crate::VoiceError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranscriptionReferenceSupport {
    #[default]
    Unsupported,
    TextAndTerminology,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TranscriptionReference {
    pub surrounding_text: Option<String>,
    pub selected_text: String,
    pub terminology: Vec<TranscriptionTerm>,
}

// Debug/logging must not disclose transient editor or conversation contents.
impl std::fmt::Debug for TranscriptionReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptionReference")
            .field("has_surrounding_text", &self.surrounding_text.is_some())
            .field("selected_bytes", &self.selected_text.len())
            .field("term_count", &self.terminology.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionTerm {
    pub canonical: String,
    #[serde(default)]
    pub heard_variants: Vec<String>,
}

impl TranscriptionReference {
    pub fn is_empty(&self) -> bool {
        self.surrounding_text
            .as_ref()
            .is_none_or(|s| s.trim().is_empty())
            && self.selected_text.trim().is_empty()
            && self.terminology.is_empty()
    }
    pub fn validate(&self) -> Result<(), VoiceError> {
        let text = self.surrounding_text.as_deref().unwrap_or_default();
        let terms_bytes: usize = self
            .terminology
            .iter()
            .map(|t| t.canonical.len() + t.heard_variants.iter().map(String::len).sum::<usize>())
            .sum();
        if text.len() + self.selected_text.len() + terms_bytes > 16 * 1024
            || text.contains('\0')
            || self.selected_text.contains('\0')
            || self.terminology.len() > 128
            || self.terminology.iter().any(|t| {
                t.canonical.trim().is_empty()
                    || t.canonical.len() > 256
                    || t.canonical.chars().any(char::is_control)
                    || t.heard_variants.len() > 8
                    || t.heard_variants.iter().any(|v| {
                        v.trim().is_empty() || v.len() > 256 || v.chars().any(char::is_control)
                    })
            })
        {
            return Err(VoiceError::InvalidOperation);
        }
        Ok(())
    }
    pub(crate) fn require_support(
        &self,
        support: TranscriptionReferenceSupport,
    ) -> Result<(), VoiceError> {
        self.validate()?;
        if !self.is_empty() && support == TranscriptionReferenceSupport::Unsupported {
            return Err(VoiceError::ReferenceUnsupported);
        }
        Ok(())
    }
}
