use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    #[default]
    Realtime,
    Segmented,
}
impl TranscriptionMode {
    pub fn legacy_recording() -> Self { Self::Segmented }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    pub revision: u64,
    #[serde(default)]
    pub transcription_mode: TranscriptionMode,
    pub cleanup: bool,
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
            glossary: String::new(),
            context: true,
            microphone: String::new(),
            shortcut_mode: ShortcutMode::Hold,
            shortcut_mac: "Fn".into(),
            shortcut_linux: "CTRL+Super_L".into(),
        }
    }
}
impl VoiceSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
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
            anyhow::ensure!(parts.len() <= 9 && parts.iter().all(|p| !p.is_empty() && p.len() <= 256 && !p.chars().any(char::is_control)), "voice.invalid_glossary_entry");
            anyhow::ensure!(terms.insert(parts[0].to_lowercase()), "voice.duplicate_glossary_term");
        }
        anyhow::ensure!(terms.len() <= 128, "voice.too_many_glossary_terms");
        Ok(())
    }
}

impl VoiceSettings {
    /// Reference data only; the existing cleanup policy controls interpretation.
    pub fn cleanup_context(&self, context: Option<String>) -> anyhow::Result<Option<String>> {
        self.validate()?;
        let context = if self.context { context } else { None };
        if self.glossary.trim().is_empty() { return Ok(context); }
        let terms = self.glossary.lines().filter(|l| !l.trim().is_empty()).map(|line| {
            let mut parts = line.split('|').map(str::trim);
            serde_json::json!({"canonical":parts.next().unwrap_or_default(),"heard_variants":parts.collect::<Vec<_>>()})
        }).collect::<Vec<_>>();
        let value = serde_json::to_string(&serde_json::json!({"nearby_context":context,"user_terminology_reference":terms}))?;
        anyhow::ensure!(value.len() <= 16 * 1024, "voice.terminology_context_too_large");
        Ok(Some(value))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptionModeChoice {
    pub mode: TranscriptionMode,
    pub label: String,
}
impl TranscriptionMode {
    pub fn choices() -> Vec<TranscriptionModeChoice> {
        vec![
            TranscriptionModeChoice { mode: Self::Realtime, label: "Realtime — transcribe while recording".into() },
            TranscriptionModeChoice { mode: Self::Segmented, label: "Segmented — transcribe at pauses".into() },
        ]
    }
}




#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutMode {
    #[default]
    Hold,
    Toggle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShortcutModeChoice { pub mode: ShortcutMode, pub label: String }
impl ShortcutMode {
    pub fn choices() -> Vec<ShortcutModeChoice> {
        vec![
            ShortcutModeChoice { mode: Self::Hold, label: "Hold to record; release to transcribe".into() },
            ShortcutModeChoice { mode: Self::Toggle, label: "Press to record; press again to transcribe".into() },
        ]
    }
}
