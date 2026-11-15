//! Shared terminal dimensions used by leaf geometry and the transport protocol.
use serde::{Deserialize, Serialize};
use crate::ProtocolValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSizeV1 {
    pub columns: u16,
    pub rows: u16,
    #[serde(default)]
    pub pixel_width: u16,
    #[serde(default)]
    pub pixel_height: u16,
}

impl TerminalSizeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.columns == 0 || self.rows == 0 {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "terminal dimensions",
            ));
        }
        Ok(())
    }
}
