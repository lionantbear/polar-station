//! Isolated text editing shared by Dictate and existing spoken corrections.
//! Hosts supply a runner; this crate never resolves a user chat or credentials.
mod policy;
mod service;
pub use policy::{EditKind, EditRequest, EditResult, ExecutionConfig, PreparedEdit, prepare};
pub use service::{edit, edit_in_session, EditSession};
