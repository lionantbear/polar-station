//! Shared typed row decoding for SQLite adapters.
//!
//! Adapter operation modules use these helpers instead of reaching directly
//! into positional `rusqlite::Row` access. Keeping the boundary here makes
//! row decoding auditable and gives future schema migrations one codec seam.

use rusqlite::{types::FromSql, Row};

pub fn get<T: FromSql>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    row.get(index)
}

pub fn try_get<T: FromSql>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    row.get(index)
}
