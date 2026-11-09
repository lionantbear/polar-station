//! Runtime UUID allocation shared by Polar producers.
//!
//! UUIDv7 uses uuid's process-wide context and millisecond clock. Sequential
//! allocations are ascending, including within a millisecond; IDs do not order
//! concurrent transaction commits or establish causality across processes.

/// Allocate a UUIDv7 at generation time, independently of business timestamps.
#[allow(clippy::disallowed_methods)]
pub fn new_uuid_v7() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

/// Preserve an existing UUIDv4 nonmessage contract; new message IDs use v7.
#[allow(clippy::disallowed_methods)]
pub fn new_uuid_v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}
