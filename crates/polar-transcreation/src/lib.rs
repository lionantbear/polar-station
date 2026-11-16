//! Isolated text editing shared by Dictate and existing spoken corrections.
//! Hosts supply a runner; this crate never resolves a user chat or credentials.
pub mod learning;
mod policy;
mod service;
pub use policy::{
    default_prompt, prepare, EditKind, EditRequest, EditResult, ExecutionConfig, PreparedEdit, RefineMode,
};
pub use polar_inhouse_contract::{InhouseInputSnapshot, InhouseTurnEventHandler};
pub use service::{edit_in_session_observed, edit, edit_in_session, prepare_execution, EditSession};

mod prompts;
pub use prompts::PromptOverrides;
