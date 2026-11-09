use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolFamily {
    PolarDelivery,
    ChatBridgeAdmin,
    SystemMaintenance,
    PromptManagement,
    LgpThreadQuery,
    ProjectNote,
    Semantic,
    AccessPlaneAdmin,
    InquiryGraphAuthoring,
    McpAdmin,
    PluginManagement,
    GearManagement,
}

impl ToolFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolarDelivery => "polar_delivery",
            Self::ChatBridgeAdmin => "chat_bridge_admin",
            Self::SystemMaintenance => "system_maintenance",
            Self::PromptManagement => "prompt_management",
            Self::LgpThreadQuery => "lgp_thread_query",
            Self::ProjectNote => "project_note",
            Self::Semantic => "semantic",
            Self::AccessPlaneAdmin => "access_plane_admin",
            Self::InquiryGraphAuthoring => "inquiry_graph_authoring",
            Self::McpAdmin => "mcp_admin",
            Self::PluginManagement => "plugin_management",
            Self::GearManagement => "gear_management",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "polar_delivery" => Some(Self::PolarDelivery),
            "chat_bridge_admin" => Some(Self::ChatBridgeAdmin),
            "system_maintenance" => Some(Self::SystemMaintenance),
            "prompt_management" => Some(Self::PromptManagement),
            "lgp_thread_query" => Some(Self::LgpThreadQuery),
            "project_note" => Some(Self::ProjectNote),
            "semantic" => Some(Self::Semantic),
            "access_plane_admin" => Some(Self::AccessPlaneAdmin),
            "inquiry_graph_authoring" => Some(Self::InquiryGraphAuthoring),
            "mcp_admin" => Some(Self::McpAdmin),
            "plugin_management" => Some(Self::PluginManagement),
            "gear_management" => Some(Self::GearManagement),
            _ => None,
        }
    }
}


