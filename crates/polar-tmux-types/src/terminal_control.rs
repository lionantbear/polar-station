//! Leaf terminal control state shared by tmux core and the wire protocol.
use serde::{Deserialize, Serialize};
use crate::TerminalSizeV1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalControlCauseV1 {
    #[default]
    Selection,
    Viewport,
    Recovery,
    Blur,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalControlPhaseV1 {
    #[default]
    Deferred,
    Requested,
    Applied,
    Observed,
    Superseded,
    Failed,
}

/// A tmux format snapshot. Pane geometry is logical, not a program TTY ioctl.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalGeometryV1 {
    pub window_id: String,
    pub window_columns: u16,
    pub window_rows: u16,
    pub client_columns: u16,
    pub client_rows: u16,
    pub status_lines: u16,
    pub window_policy: String,
}
impl TerminalGeometryV1 {
    pub fn matches(&self, size: TerminalSizeV1) -> bool {
        let status = if self.client_rows <= self.status_lines {
            0
        } else {
            self.status_lines
        };
        self.window_policy == "latest"
            && self.client_columns == size.columns
            && self.client_rows == size.rows
            && self.window_columns == size.columns.max(1)
            && self.window_rows == size.rows.saturating_sub(status).max(1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalControlStatusV1 {
    pub sequence: u64,
    pub binding_generation: u64,
    pub viewport_revision: u64,
    pub phase: TerminalControlPhaseV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<TerminalGeometryV1>,
}
