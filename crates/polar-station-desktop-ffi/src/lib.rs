//! Native ABI adapters for shared Station clients and interaction models.
pub mod desktop_client_ffi;
pub mod native_ui_ffi;
pub mod dictate_ffi;
pub mod dictate_adapter;
pub mod lgp_client_ffi;
pub mod desktop_recovery;
pub use polar_station_client::{desktop_client, dictate_speech, native_state_file, station_input_client};
pub use polar_local_access_runtime::operation_trace;
