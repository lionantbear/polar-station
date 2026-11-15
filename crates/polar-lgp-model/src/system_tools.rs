use crate::ToolFamily;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemToolExecutionClass {
    LiveSafe,
    StopRequired,
}

impl SystemToolExecutionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LiveSafe => "live_safe",
            Self::StopRequired => "stop_required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemToolSpec {
    pub tool_id: &'static str,
    pub family: ToolFamily,
    pub execution_class: SystemToolExecutionClass,
    pub llm_callable: bool,
    pub slash_callable: bool,
}
