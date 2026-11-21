//! Authenticated Station IPC clients; no Host execution or provider ownership.
pub mod desktop_client;
pub mod desktop_terminal;
#[cfg(unix)]
pub mod terminal_bridge;
pub mod terminal_content;
pub mod session_client;
pub mod dictate_client;
pub mod dictate_speech;
pub mod station_input_client;
pub mod native_state_file;
pub use polar_local_access_runtime::operation_trace;
pub mod local_ipc {
 pub use polar_local_access_runtime::{effective_uid, codec::{read_json_frame, write_json_frame}, socket_readiness};
 pub use polar_runtime_paths::station::default_socket_path;
}
