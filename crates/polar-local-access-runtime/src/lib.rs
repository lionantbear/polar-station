//! Bounded local transport and connection execution, independent of Station/Kernel.
pub mod codec;
pub mod local_extensions;
pub mod server;

pub mod listener;

pub fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}
