//! Reusable Polar Station Host runtime.
//!
//! The `polar-tmux` executable is one standalone composition of this library.
//! The `polar` process may compose the same Host runtime explicitly. Desktop
//! presentation processes use the shared control client; terminal bytes remain
//! on the separate Pane Host bridge.

mod discovery;
mod standalone_runtime;
mod native_account;
pub use standalone_runtime::run_standalone_runtime;
mod host;
pub mod host_identity;
pub mod host_runtime;
#[cfg(unix)]
pub mod local_ipc;
#[cfg(unix)]
pub mod pane_host;
#[cfg(unix)]
mod terminal_io;
pub mod ssh_connection_manager;
mod target_registry;

/// Private adapter used by Codex's `notify` configuration. It intentionally
/// returns only whether a bounded tmux event was recorded.
pub fn record_codex_notify(payload: &str) -> Result<bool, String> {
    host::codex_monitor::record_codex_notify(payload)
}

pub mod local_extensions;
pub mod dictate_host;

/// Run the standalone macOS Host with its Messages adapter on the process main thread.
#[cfg(target_os = "macos")]
pub fn run_with_messages_main_thread<F>(future: F) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    host::imessage_sender::macos::run_on_process_main(move || run_standalone_runtime(future))
}

#[cfg(unix)]
pub mod polar_access_transport;

#[path = "polar_gateway/config.rs"]
mod polar_target_config;
#[cfg(unix)]
mod polar_gateway;

pub use polar_local_access_runtime::operation_trace;

