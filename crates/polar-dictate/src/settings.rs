use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    #[default]
    Realtime,
    Segmented,
}
impl TranscriptionMode {
    pub fn legacy_recording() -> Self {
        Self::Segmented
    }
}

/// Capture-time reference needs, projected by the Host using actual provider support.
/// Explicit Quote capture and insertion safety checks are independent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencePolicy {
    pub recognition: bool,
    pub refinement: bool,
    pub surrounding: bool,
    pub terminology: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    pub revision: u64,
    #[serde(default)]
    pub transcription_mode: TranscriptionMode,
    /// Compatibility mirror; output_mode is the product choice.
    pub cleanup: bool,
    #[serde(default)]
    pub output_mode: crate::DictateOutputMode,
    #[serde(default)]
    pub retention: crate::RetentionPolicy,
    /// One canonical term per line, with optional pipe-separated heard variants.
    #[serde(default)]
    pub glossary: String,
    pub context: bool,
    /// Empty follows the system default; a nonempty missing device fails closed.
    pub microphone: String,
    #[serde(default)]
    pub shortcut_mode: ShortcutMode,
    pub shortcut_mac: String,
    pub shortcut_linux: String,
}
impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            revision: 0,
            transcription_mode: TranscriptionMode::Realtime,
            cleanup: true,
            output_mode: crate::DictateOutputMode::Refine,
            retention: crate::RetentionPolicy::default(),
            glossary: String::new(),
            context: true,
            microphone: String::new(),
            shortcut_mode: ShortcutMode::Auto,
            shortcut_mac: "Fn".into(),
            shortcut_linux: "CTRL+Super_L".into(),
        }
    }
}
impl VoiceSettings {
    pub fn reference_policy(&self, recognition_supported: bool) -> ReferencePolicy {
        let recognition = recognition_supported;
        let refinement = self.mode().refines();
        ReferencePolicy {
            recognition, refinement,
            surrounding: self.context && (recognition || refinement),
            terminology: recognition || refinement,
        }
    }
    pub fn mode(&self) -> crate::DictateOutputMode {
        if self.cleanup {
            self.output_mode
        } else {
            crate::DictateOutputMode::NoRefine
        }
    }
    pub fn set_output_mode(&mut self, mode: crate::DictateOutputMode) {
        self.output_mode = mode;
        self.cleanup = mode.refines();
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.retention.validate()?;
        anyhow::ensure!(
            self.microphone.len() <= 512 && !self.microphone.chars().any(char::is_control),
            "voice.invalid_microphone"
        );
        anyhow::ensure!(
            matches!(
                self.shortcut_mac.as_str(),
                "Fn" | "Control+Option+Space" | "Control+Shift+Space" | "RightOption"
            ),
            "voice.unsupported_shortcut"
        );
        anyhow::ensure!(
            !self.shortcut_linux.is_empty()
                && self.shortcut_linux.len() <= 128
                && self
                    .shortcut_linux
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+_-".contains(&b)),
            "voice.invalid_shortcut"
        );
        anyhow::ensure!(self.glossary.len() <= 8192, "voice.glossary_too_large");
        let mut terms = std::collections::BTreeSet::new();
        for line in self.glossary.lines().filter(|line| !line.trim().is_empty()) {
            let parts = line.split('|').map(str::trim).collect::<Vec<_>>();
            anyhow::ensure!(
                parts.len() <= 9
                    && parts.iter().all(|p| !p.is_empty()
                        && p.len() <= 256
                        && !p.chars().any(char::is_control)),
                "voice.invalid_glossary_entry"
            );
            anyhow::ensure!(
                terms.insert(parts[0].to_lowercase()),
                "voice.duplicate_glossary_term"
            );
        }
        anyhow::ensure!(terms.len() <= 128, "voice.too_many_glossary_terms");
        Ok(())
    }
}

impl VoiceSettings {
    fn glossary_terms(&self) -> impl Iterator<Item = (&str, Vec<&str>)> {
        self.glossary
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut parts = line.split('|').map(str::trim);
                (parts.next().unwrap_or_default(), parts.collect())
            })
    }
    fn reference_data(
        &self,
        context: Option<String>,
        selected: &str,
    ) -> anyhow::Result<serde_json::Value> {
        self.validate()?;
        anyhow::ensure!(
            context
                .as_ref()
                .is_none_or(|s| s.len() <= 16 * 1024 && !s.contains('\0'))
                && selected.len() <= 16 * 1024
                && !selected.contains('\0'),
            "dictate.reference_context_limit"
        );
        let terms=self.glossary_terms().map(|(canonical,heard_variants)|serde_json::json!({"canonical":canonical,"heard_variants":heard_variants})).collect::<Vec<_>>();
        let selected = &selected[..selected.floor_char_boundary(selected.len().min(4096))];
        let mut data = serde_json::json!({"nearby_context":if self.context{context}else{None},"selected_text_reference":selected,"user_terminology_reference":terms});
        // Removing N raw UTF-8 bytes removes at least N encoded JSON bytes.
        // Trim nearby context first, then the model-only selection excerpt;
        // the recording's complete Quote and glossary are never modified.
        for key in ["nearby_context", "selected_text_reference"] {
            let excess = serde_json::to_vec(&data)?.len().saturating_sub(16 * 1024);
            if excess == 0 {
                break;
            }
            if let Some(text) = data[key].as_str() {
                let keep = text.floor_char_boundary(text.len().saturating_sub(excess));
                data[key] = serde_json::json!(&text[..keep]);
            }
        }
        anyhow::ensure!(
            serde_json::to_vec(&data)?.len() <= 16 * 1024,
            "voice.terminology_context_too_large"
        );
        Ok(data)
    }
    #[cfg(feature = "runtime")]
    pub fn transcription_reference(
        &self,
        context: Option<String>,
        selected_text: String,
    ) -> anyhow::Result<polar_voice::TranscriptionReference> {
        let data = self.reference_data(context, &selected_text)?;
        let reference = polar_voice::TranscriptionReference {
            surrounding_text: data["nearby_context"].as_str().map(str::to_owned),
            selected_text: data["selected_text_reference"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            terminology: serde_json::from_value(data["user_terminology_reference"].clone())?,
        };
        reference.validate()?;
        Ok(reference)
    }
    pub(crate) fn reference_context(
        &self,
        context: Option<String>,
        selected: &str,
    ) -> anyhow::Result<Option<String>> {
        let data = self.reference_data(context, selected)?;
        if self.glossary.trim().is_empty() && selected.is_empty() {
            return Ok(data["nearby_context"].as_str().map(str::to_owned));
        }
        Ok(Some(serde_json::to_string(&data)?))
    }
    /// Reference data only; the existing cleanup policy controls interpretation.
    pub fn cleanup_context(&self, context: Option<String>) -> anyhow::Result<Option<String>> {
        self.reference_context(context, "")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptionModeChoice {
    pub mode: TranscriptionMode,
    pub label: String,
    pub context_label: String,
    pub reference_hint: String,
}
impl TranscriptionMode {
    pub fn choices() -> Vec<TranscriptionModeChoice> {
        vec![
            TranscriptionModeChoice {
                mode: Self::Realtime,
                label: "Realtime — transcribe while recording".into(),
                context_label: "Use nearby context for names and terminology".into(),
                reference_hint: "Realtime can use context and terminology during recognition, including No Refine. Dictate uses these references only with Refine or Transcreate; No Refine skips collection. Selected text stays quoted independently.".into(),
            },
            TranscriptionModeChoice {
                mode: Self::Segmented,
                label: "Dictate — whole short recordings; long recordings segmented".into(),
                context_label: "Use nearby context for names and terminology".into(),
                reference_hint: "Realtime can use context and terminology during recognition, including No Refine. Dictate uses these references only with Refine or Transcreate; No Refine skips collection. Selected text stays quoted independently.".into(),
            },
        ]
    }
}



#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutMode {
    #[default]
    #[serde(alias="hold",alias="toggle")]
    Auto,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShortcutModeChoice {
    pub mode: ShortcutMode,
    pub label: String,
}
impl ShortcutMode {
    pub fn choices() -> Vec<ShortcutModeChoice> {
        vec![ShortcutModeChoice {mode:Self::Auto,label:"Tap to keep recording; hold and release to stop; press again to stop".into()}]
    }
}



