//! Foreground terminal control, independent of recovery and native layout.
use crate::{
    validate_stable_identifier, ProtocolValidationError, TerminalBridgeHelloV1, TerminalSizeV1,
    TerminalControlCauseV1,
};
use serde::{Deserialize, Serialize};

pub const TERMINAL_CONTROL_API_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalBridgeHelloV2 {
    pub base: TerminalBridgeHelloV1,
    pub terminal_control_version: u16,
}
impl TerminalBridgeHelloV2 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.base.validate()?;
        if self.terminal_control_version != TERMINAL_CONTROL_API_VERSION {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "terminal control version",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalControlIntentV1 {
    pub native_instance_id: String,
    pub sequence: u64,
    pub binding_generation: u64,
    pub viewport_revision: u64,
    pub focused: bool,
    pub cause: TerminalControlCauseV1,
    pub size: TerminalSizeV1,
}
impl TerminalControlIntentV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.native_instance_id, "native control instance")?;
        if self.sequence == 0 || self.binding_generation == 0 || self.viewport_revision == 0 {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "terminal control revision",
            ));
        }
        self.size.validate()
    }
}


