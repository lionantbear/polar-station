use serde::{Deserialize, Serialize};

/// Exhaustive recovery states shared by both native renderers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRecoveryPhaseV1 {
    #[default]
    Hidden,
    Probing,
    Attaching,
    Waiting,
    Paused,
    Ended,
    Recovered,
    HostUnavailable,
    StatusUnavailable,
}

/// Shared product copy; native adapters only place and render this projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRecoveryViewV1 {
    pub visible: bool,
    pub phase: TerminalRecoveryPhaseV1,
    #[serde(default)]
    pub content_generation: u64,
    pub message: String,
    pub detail: String,
    pub retry_enabled: bool,
    /// Compatibility only; unsent-input feedback was removed. Always false.
    pub input_warning: bool,
    pub last_probe_unix_ms: Option<u64>,
    pub retry_label: String,
    pub details_label: String,
    pub dismiss_label: String,
    pub input_warning_message: String,
    pub probe_time_label: String,
}
impl Default for TerminalRecoveryViewV1 {
    fn default() -> Self {
        Self {
            visible: false,
            phase: TerminalRecoveryPhaseV1::Hidden,
            content_generation: 1,
            message: String::new(),
            detail: String::new(),
            retry_enabled: false,
            input_warning: false,
            last_probe_unix_ms: None,
            retry_label: "Retry now".into(),
            details_label: "Details".into(),
            dismiss_label: "Dismiss".into(),
            input_warning_message: String::new(),
            probe_time_label: "Last checked (UTC)".into(),
        }
    }
}
