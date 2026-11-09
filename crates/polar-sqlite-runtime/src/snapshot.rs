//! Standalone, consistent SQLite images. No Node, CAS or product semantics.
//!
//! `seal_connection` must be called by the existing connection owner (for
//! example inside its serialized maintenance closure). It never opens a second
//! owner or migrates/checkpoints the source database.
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags, MAIN_DB};
use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct SealedSqliteFile {
    pub path: PathBuf,
    pub size_bytes: u64,
}

fn checked_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "absolute file path required"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "parent traversal refused"
    );
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "SQLite snapshot path is a symlink"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The caller must exclude live writers/owners when using the file entry point.
/// An online owner must use `seal_connection` instead.
pub fn seal_file(source: &Path, destination: &Path) -> Result<SealedSqliteFile> {
    checked_path(source)?;
    ensure!(
        std::fs::symlink_metadata(source)?.is_file(),
        "snapshot source must be a regular file"
    );
    let input = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let result = seal_connection(&input, destination)?;
    input.close().map_err(|(_, error)| error)?;
    Ok(result)
}

/// Produce one complete DELETE-journal image including committed WAL pages.
/// No overwrite; temporary sidecars and unpublished output belong to this call.
pub fn seal_connection(input: &Connection, destination: &Path) -> Result<SealedSqliteFile> {
    checked_path(destination)?;
    ensure!(
        input.is_autocommit(),
        "snapshot requires an owner connection outside a transaction"
    );
    match std::fs::symlink_metadata(destination) {
        Ok(_) => anyhow::bail!("snapshot destination already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = destination.parent().context("snapshot parent required")?;
    ensure!(parent.is_dir(), "snapshot parent must already exist");
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let sidecars = TemporarySidecars(temporary.path().to_owned());
    input.backup(MAIN_DB, temporary.path(), None)?;
    let output = Connection::open(temporary.path())?;
    let mode: String = output.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
    ensure!(mode == "delete", "snapshot could not leave WAL mode");
    let mut statement = output.prepare("PRAGMA integrity_check")?;
    let results = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(results == ["ok"], "snapshot integrity check failed");
    drop(statement);
    output.close().map_err(|(_, error)| error)?;
    for path in sidecars.paths() {
        ensure!(!path.try_exists()?, "snapshot is not sealed");
    }
    temporary.as_file().sync_all()?;
    let size_bytes = temporary.as_file().metadata()?.len();
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(SealedSqliteFile {
        path: destination.to_owned(),
        size_bytes,
    })
}

struct TemporarySidecars(PathBuf);
impl TemporarySidecars {
    fn paths(&self) -> [PathBuf; 3] {
        ["-wal", "-shm", "-journal"].map(|suffix| {
            let mut name = self.0.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        })
    }
}
impl Drop for TemporarySidecars {
    fn drop(&mut self) {
        for path in self.paths() {
            let _ = std::fs::remove_file(path);
        }
    }
}


