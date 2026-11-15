//! Pure shared LGP presentation. No transport, storage or native UI state.
mod activity;
pub use activity::{tool_state_label, ActivityKind, ActivityPresentation};
mod message;
pub use message::{causal_label, causal_presentation, content_fallback_label, message_attachments, message_content, message_text, message_view_text, message_view_attachments, CausalPresentation, MessagePresentation};

mod author;
pub use author::{author_label, author_presentation, message_view_author, AuthorFacts, AuthorPresentation};
