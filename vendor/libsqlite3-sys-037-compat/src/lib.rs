//! Expose the workspace's SQLite bindings to Pi's transitive 0.37 consumer.
//! The real binding crate owns the native SQLite link.

pub use sqlite3_sys::*;
