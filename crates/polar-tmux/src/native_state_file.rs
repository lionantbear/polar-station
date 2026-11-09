//! Shared native snapshot IO, extracted from the desktop layout worker.
//! Call only on a background worker; callers own schemas and recovery policy.
use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

pub(crate) fn exchange(
    path: &Path,
    document: Option<&str>,
    maximum: usize,
) -> Result<Option<String>> {
    if let Some(document) = document {
        if document.len() > maximum {
            bail!("native state exceeds bound");
        }
        let parent = path.parent().context("native state directory")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(document.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        fs::File::open(parent)?.sync_all()?;
        if fs::read(path)? != document.as_bytes() {
            bail!("native state read-back mismatch");
        }
        Ok(None)
    } else {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut document = String::new();
        file.take(maximum as u64 + 1)
            .read_to_string(&mut document)?;
        if document.len() > maximum {
            bail!("native state exceeds bound");
        }
        Ok(Some(document))
    }
}
