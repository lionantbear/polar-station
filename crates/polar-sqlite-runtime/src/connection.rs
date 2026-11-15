//! Pre-publication SQLite maintenance owned by the runtime boundary.
//!
//! Compatibility work that must happen before migrations are published uses
//! this narrowly scoped connection owner. Stores supply only a maintenance
//! callback; they never open a second SQLite connection themselves.

use std::path::Path;

use rusqlite::Connection;

use crate::SqliteRuntimeError;

/// Backend-native maintenance that must complete before a runtime is opened.
pub type SqliteMaintenance = fn(&Connection) -> rusqlite::Result<()>;

/// Opens a short-lived pre-publication maintenance connection for a file-backed
/// target. The connection is never published to adapters or callers.
pub fn run_sqlite_maintenance(
    path: impl AsRef<Path>,
    maintenance: SqliteMaintenance,
) -> Result<(), SqliteRuntimeError> {
    let path = path.as_ref();
    if path == Path::new("sqlite::memory:") {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|_| SqliteRuntimeError::Closed)?;
    }
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    maintenance(&connection)?;
    Ok(())
}
