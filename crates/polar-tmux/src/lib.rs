//! Reusable Polar Station Host runtime and desktop control client.
//!
//! The `polar-tmux` executable is one standalone composition of this library.
//! The `polar` process may compose the same Host runtime explicitly. Desktop
//! presentation processes use the shared control client; terminal bytes remain
//! on the separate Pane Host bridge.

mod discovery;
mod standalone_runtime;
pub use standalone_runtime::run_standalone_runtime;
mod host;
pub mod host_identity;
#[cfg(unix)]
pub mod session_client;
pub mod host_runtime;
#[cfg(unix)]
pub mod local_ipc;
pub mod operation_trace;
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
pub mod desktop_client;
mod native_state_file;
pub mod desktop_client_ffi;
pub mod native_ui_ffi;
mod dictate_adapter;
pub mod dictate_ffi;
pub mod dictate_speech;
pub mod dictate_host;
#[cfg(unix)]
pub mod dictate_client;
pub mod lgp_client_ffi;
#[cfg(unix)]
mod station_input_client;
pub mod desktop_recovery;

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
