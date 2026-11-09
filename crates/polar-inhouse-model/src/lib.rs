mod reply_generation;
pub mod role;
pub use reply_generation::*;
pub use role::{
    validate_role_catalog, InhousePolarRoleBinding, InhouseRoleRecord, RoleContractError,
};

use polar_identity::{NodeId, UnixTimestamp};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InhousePolarSystemRole {
    Eve,
    PromptAuthor,
    PromptOptimizer,
    Scheduler,
    Plato,
    Socrates,
    Darwin,
    InquiryGraphAuthor,
}

impl InhousePolarSystemRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eve => "eve",
            Self::PromptAuthor => "prompt_author",
            Self::PromptOptimizer => "prompt_optimizer",
            Self::Scheduler => "scheduler",
            Self::Plato => "plato",
            Self::Socrates => "socrates",
            Self::Darwin => "darwin",
            Self::InquiryGraphAuthor => "inquiry_graph_author",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "eve" | "housekeeper" => Some(Self::Eve),
            "prompt_author" => Some(Self::PromptAuthor),
            "prompt_optimizer" => Some(Self::PromptOptimizer),
            "scheduler" => Some(Self::Scheduler),
            "plato" => Some(Self::Plato),
            "socrates" => Some(Self::Socrates),
            "darwin" => Some(Self::Darwin),
            "inquiry_graph_author" => Some(Self::InquiryGraphAuthor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhouseBackendProfileRecord {
    pub backend_profile_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub backend_driver: String,
    pub backend_config_json: String,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhousePolarRecord {
    pub inhouse_polar_id: String,
    pub polar_name: String,
    pub system_role: Option<InhousePolarSystemRole>,
    pub enabled: bool,
    pub active_backend_profile_id: String,
    pub git_repo_url: Option<String>,
    pub git_default_branch: Option<String>,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Idle,
    Busy,
    Offline,
    Failed,
}

impl InstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Offline => "offline",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "busy" => Self::Busy,
            "offline" => Self::Offline,
            "failed" => Self::Failed,
            _ => Self::Idle,
        }
    }
}

impl FromStr for InstanceStatus {
    type Err = core::convert::Infallible;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(raw))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadBindingStatus {
    Attached,
    Detached,
}

impl ThreadBindingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attached => "attached",
            Self::Detached => "detached",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "detached" => Self::Detached,
            _ => Self::Attached,
        }
    }
}

impl FromStr for ThreadBindingStatus {
    type Err = core::convert::Infallible;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(raw))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InhousePolarInstanceRecord {
    pub instance_id: String,
    pub inhouse_polar_id: String,
    pub backend_profile_id: String,
    pub status: InstanceStatus,
    pub current_session_id: Option<String>,
    pub last_error: Option<String>,
    pub started_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRuntimeBindingRecord {
    pub owner_id: NodeId,
    pub thread_id: String,
    pub inhouse_polar_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub status: ThreadBindingStatus,
    pub resume_prompt: bool,
    pub pending_session_mode: Option<String>,
    pub model_override: Option<String>,
    pub thinking_override: Option<String>,
    pub attached_at: UnixTimestamp,
    pub last_active_at: UnixTimestamp,
    pub detached_at: Option<UnixTimestamp>,
}


pub mod configuration;
