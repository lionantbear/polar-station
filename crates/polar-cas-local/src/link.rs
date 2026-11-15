//! Explicit local-only zero-copy capability for already trusted material.
//!
//! Callers own content verification, source immutability, destination confinement
//! and mode/ownership policy. This operation never hashes, copies or chmods data.
use polar_cas::{Error, Result};
use std::path::{Path, PathBuf};

/// Local inode variants are a projection/cache concern. Mode is not part of
/// ObjectId: two variants can contain the same content with distinct inodes.
/// Keep the existing on-disk projection layout; no data migration is implied.
pub fn file_variant_path(root: &Path, id: &polar_cas::ObjectId, mode: u32) -> Result<PathBuf> {
    if !root.is_absolute() || mode & !0o777 != 0 {
        return Err(Error::Invalid(
            "invalid local file projection binding".into(),
        ));
    }
    Ok(root
        .join("blobs")
        .join(id.algorithm().as_str())
        .join(id.hex())
        .join(format!("mode-{mode:04o}")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutcome {
    Created,
    Reused,
}

#[cfg(unix)]
pub fn link_verified_file(source: &Path, destination: &Path) -> Result<LinkOutcome> {
    use std::os::unix::fs::MetadataExt;
    let source_metadata = std::fs::symlink_metadata(source)?;
    if !source_metadata.is_file() {
        return Err(Error::Invalid(
            "hardlink source must be a regular file".into(),
        ));
    }
    match std::fs::hard_link(source, destination) {
        Ok(()) => Ok(LinkOutcome::Created),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let target = std::fs::symlink_metadata(destination)?;
            if !target.is_file()
                || source_metadata.dev() != target.dev()
                || source_metadata.ino() != target.ino()
            {
                return Err(Error::Integrity);
            }
            Ok(LinkOutcome::Reused)
        }
        Err(error) => Err(error.into()), // Includes EXDEV: never copy as fallback.
    }
}
