//! IDEA-CAVE description core. No filesystem, network, Vault or execution I/O.
//!
//! A digest calculated here is a candidate identity, not CAS admission. The
//! runtime must require admitted content BEFORE treating any reference as a
//! determined entity or consuming it in a native projection. In particular,
//! pure filesystem lowering deliberately retains the existing CAS metadata
//! semantics; it does not mint an admission or authorize a source path.
pub mod composition;
pub mod documents;
pub mod etree;
pub mod facts;
pub mod history;
pub mod itree;
pub mod native;
pub mod publication;
pub mod reference;
pub mod search;
pub mod workspace;

pub use itree::{decode_cas, encode_idea, parse_idea, EncodedIdea, IdeaTree, Limits};
pub use reference::{NodeRef, Reference, TreeFormat};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid IDEA-CAVE structure at {path}: {reason}")]
    Invalid { path: String, reason: &'static str },
    #[error("IDEA-CAVE resource limit: {0}")]
    Limit(&'static str),
    #[error("IDEA-CAVE reference identity mismatch")]
    IdentityMismatch,
    #[error("CAS description is not canonical")]
    NonCanonical,
    #[error("this definition requires an unavailable handler at {0}")]
    MissingHandler(String),
    #[error("unresolved source at {0}")]
    Unresolved(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("CAS: {0}")]
    Cas(#[from] polar_cas::Error),
}
pub(crate) fn invalid(path: impl Into<String>, reason: &'static str) -> Error {
    Error::Invalid {
        path: path.into(),
        reason,
    }
}
pub mod content;
