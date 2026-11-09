//! Shared bounded local-access wire contract; no Station or runtime dependencies.
mod common;
pub use common::*;
pub mod extensions;
pub use extensions::*;
pub mod polar_access_transport;
pub use polar_access_transport::*;
mod config;
pub use config::LocalChatConfig;
