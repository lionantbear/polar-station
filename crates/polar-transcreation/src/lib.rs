//! Isolated text editing shared by Dictate and existing spoken corrections.
//! Hosts supply a runner; this crate never resolves a user chat or credentials.
pub mod learning;
mod policy;
mod service;
pub use policy::{
    prepare, EditKind, EditRequest, EditResult, ExecutionConfig, PreparedEdit, RefineMode,
};
pub use service::{edit, edit_in_session, prepare_execution, EditSession};
