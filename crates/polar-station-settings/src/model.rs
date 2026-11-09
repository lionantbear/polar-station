use serde::{Deserialize, Serialize};

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub const MIN_LAYOUT_MAP_WIDTH: u16 = 200;
pub const MAX_LAYOUT_MAP_WIDTH: u16 = 390;
pub const MIN_INBOX_WIDTH: u16 = 300;
pub const MAX_INBOX_WIDTH: u16 = 520;
pub const MIN_INTERFACE_TEXT_SCALE_PERCENT: u16 = 80;
pub const MAX_INTERFACE_TEXT_SCALE_PERCENT: u16 = 200;
pub const MIN_SUMMARY_TIMEOUT_MS: u64 = 1_000;
pub const MAX_SUMMARY_TIMEOUT_MS: u64 = 60_000;
pub const MIN_SUMMARY_QUIET_PERIOD_MS: u64 = 1_000;
pub const MAX_SUMMARY_QUIET_PERIOD_MS: u64 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub appearance: AppearanceSettingsV1,
    pub attention: AttentionSettingsV1,
    #[serde(default)]
    pub monitoring: MonitoringSettingsV1,
    pub session_summary: SessionSummarySettingsV1,
}

impl Default for SettingsV1 {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            revision: 0,
            appearance: AppearanceSettingsV1::default(),
            attention: AttentionSettingsV1::default(),
            monitoring: MonitoringSettingsV1::default(),
            session_summary: SessionSummarySettingsV1::default(),
        }
    }
}

impl SettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchema(self.schema_version));
        }
        self.appearance.validate()?;
        self.monitoring.validate()?;
        self.session_summary.validate()?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDeliveryModeV1 {
    #[default]
    Insert,
    Send,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSendModeV1 { #[default] Enter, ModifiedEnter }

fn default_true() -> bool {
    true
}

const fn default_interface_text_scale_percent() -> u16 {
    100
}

const fn default_summary_quiet_period_ms() -> u64 {
    5_000
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceSettingsV1 {
    #[serde(default)]
    pub theme: crate::ThemeSettingsV1,
    #[serde(default)]
    pub lgp_cache: polar_lgp_reader::CachePolicy,
    /// None retains the adaptive native width.
    pub layout_map_width: Option<u16>,
    #[serde(default = "default_true")]
    pub layout_map_visible: bool,
    #[serde(default = "default_inbox_visible")]
    pub inbox_visible: bool,
    #[serde(default)]
    pub inbox_width: Option<u16>,
    #[serde(default)]
    pub terminal_delivery_mode: TerminalDeliveryModeV1,
    #[serde(default)]
    pub chat_send_mode: ChatSendModeV1,
    #[serde(default = "default_interface_text_scale_percent")]
    pub interface_text_scale_percent: u16,
}

impl Default for AppearanceSettingsV1 {
    fn default() -> Self {
        Self {
            theme: Default::default(),
            lgp_cache: Default::default(),
            layout_map_width: None,
            layout_map_visible: true,
            inbox_visible: default_inbox_visible(),
            inbox_width: None,
            terminal_delivery_mode: TerminalDeliveryModeV1::Insert,
            chat_send_mode: ChatSendModeV1::Enter,
            interface_text_scale_percent: default_interface_text_scale_percent(),
        }
    }
}

impl AppearanceSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.theme.validate()?;
        self.lgp_cache.validate().map_err(ValidationError::CachePolicy)?;
        if let Some(width) = self.layout_map_width {
            if !(MIN_LAYOUT_MAP_WIDTH..=MAX_LAYOUT_MAP_WIDTH).contains(&width) {
                return Err(ValidationError::LayoutMapWidth(width));
            }
        }
        if let Some(width) = self.inbox_width {
            if !(MIN_INBOX_WIDTH..=MAX_INBOX_WIDTH).contains(&width) {
                return Err(ValidationError::InboxWidth(width));
            }
        }
        if !(MIN_INTERFACE_TEXT_SCALE_PERCENT..=MAX_INTERFACE_TEXT_SCALE_PERCENT)
            .contains(&self.interface_text_scale_percent)
            || !self.interface_text_scale_percent.is_multiple_of(10)
        {
            return Err(ValidationError::InterfaceTextScale(
                self.interface_text_scale_percent,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionSettingsV1 {
    pub system_notifications_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitoringSettingsV1 {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_summary_quiet_period_ms")]
    pub quiet_period_ms: u64,
    #[serde(default = "default_true")]
    pub codex_title_detection: bool,
}

impl Default for MonitoringSettingsV1 {
    fn default() -> Self {
        Self {
            enabled: true,
            quiet_period_ms: default_summary_quiet_period_ms(),
            codex_title_detection: true,
        }
    }
}

impl MonitoringSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(MIN_SUMMARY_QUIET_PERIOD_MS..=MAX_SUMMARY_QUIET_PERIOD_MS)
            .contains(&self.quiet_period_ms)
        {
            return Err(ValidationError::SummaryQuietPeriod(self.quiet_period_ms));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummarySettingsV1 {
    pub enabled: bool,
    pub command: String,
    pub model: Option<String>,
    pub timeout_ms: u64,
    #[serde(default = "default_summary_quiet_period_ms")]
    pub quiet_period_ms: u64,
}

impl Default for SessionSummarySettingsV1 {
    fn default() -> Self {
        Self {
            enabled: false,
            command: "codex".to_owned(),
            model: None,
            timeout_ms: 15_000,
            quiet_period_ms: default_summary_quiet_period_ms(),
        }
    }
}

impl SessionSummarySettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_single_line("session summary command", &self.command, 1, 1024)?;
        if let Some(model) = &self.model {
            validate_single_line("session summary model", model, 1, 128)?;
        }
        if !(MIN_SUMMARY_TIMEOUT_MS..=MAX_SUMMARY_TIMEOUT_MS).contains(&self.timeout_ms) {
            return Err(ValidationError::SummaryTimeout(self.timeout_ms));
        }
        if !(MIN_SUMMARY_QUIET_PERIOD_MS..=MAX_SUMMARY_QUIET_PERIOD_MS)
            .contains(&self.quiet_period_ms)
        {
            return Err(ValidationError::SummaryQuietPeriod(self.quiet_period_ms));
        }
        Ok(())
    }
}

fn validate_single_line(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ValidationError> {
    if value.len() < minimum
        || value.len() > maximum
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(ValidationError::InvalidText(field));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsSectionUpdateV1 {
    Appearance(AppearanceSettingsV1),
    Attention(AttentionSettingsV1),
    Monitoring(MonitoringSettingsV1),
    SessionSummary(SessionSummarySettingsV1),
}

impl SettingsSectionUpdateV1 {
    pub(crate) fn apply(self, settings: &mut SettingsV1) -> Result<(), ValidationError> {
        match self {
            Self::Appearance(value) => {
                value.validate()?;
                settings.appearance = value;
            }
            Self::Attention(value) => settings.attention = value,
            Self::Monitoring(value) => {
                value.validate()?;
                settings.session_summary.quiet_period_ms = value.quiet_period_ms;
                settings.monitoring = value;
            }
            Self::SessionSummary(value) => {
                value.validate()?;
                settings.monitoring.quiet_period_ms = value.quiet_period_ms;
                settings.session_summary = value;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("{0}")]
    Voice(String),
    #[error("{0}")]
    CachePolicy(String),
    #[error("unsupported settings schema version {0}")]
    UnsupportedSchema(u32),
    #[error("layout map width {0} is outside 200 through 390")]
    LayoutMapWidth(u16),
    #[error("Inbox width {0} is outside 300 through 520")]
    InboxWidth(u16),
    #[error("Layout and Inbox cannot both be hidden")]
    NoMonitoringView,
    #[error("interface text scale {0} percent is not a 10-percent step from 80 through 200")]
    InterfaceTextScale(u16),
    #[error("{0} must be trimmed, single-line, and bounded")]
    InvalidText(&'static str),
    #[error("session summary timeout {0} ms is outside 1000 through 60000")]
    SummaryTimeout(u64),
    #[error("session summary quiet period {0} ms is outside 1000 through 30000")]
    SummaryQuietPeriod(u64),
}

fn default_inbox_visible() -> bool {
    false
}
