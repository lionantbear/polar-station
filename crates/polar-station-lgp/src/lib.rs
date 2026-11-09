//! Shared Station presentation state. No PTY, network runtime, SQLite history,
//! authority, signing, OS geometry, or toolkit belongs here. Native adapters
//! execute bounded effects; canonical data arrives through existing Polar SDK.
mod cards;
mod window;
mod record;
mod text;
pub mod presentation;
pub mod state;
pub use state::{Effect, Engine, Input, Scope};
