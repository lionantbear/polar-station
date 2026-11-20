pub mod client;
mod codex;
mod codex_app_server_bootstrap;
mod codex_app_server_compat;
mod connector_policy;

mod profile_config;
mod profile_runtime;
mod shared_client;
mod thread_lifecycle;

pub use codex::{CodexAgentConfig, CodexAgentRunner, CodexMode};
pub use codex_app_server_bootstrap::native_codex_home;
pub use codex_app_server_bootstrap::validate_conversation_model_metadata;
mod auth_source;
pub use polar_inhouse_contract::CodexRuntimeBackend;
pub use profile_runtime::{CodexBackendRegistry, CodexProfilePaths};

fn env_flag_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name),
        Ok(value)
            if value == "1"
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("yes")
    )
}

pub(crate) fn backend_request_debug_enabled() -> bool {
    env_flag_enabled("POLAR_DEBUG_BACKEND_REQUESTS")
}

pub(crate) fn safe_debug_text(text: &str) -> String {
    const LIMIT: usize = 4096;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}...[truncated {} chars]",
        &text[..end],
        text.len().saturating_sub(end)
    )
}
