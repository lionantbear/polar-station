//! External file access on the existing CAS directory capability. Never admits bytes.
use polar_cas::resource::*;
use std::{
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

// Compatibility name for the migrated port; this is the existing confined source.
pub use crate::LocalSource as LocalFileProvider;
use cap_std::fs::{Dir, Metadata};

impl ResourceProvider for crate::LocalSource {
    fn list(
        &self,
        locator: &ResourceLocator,
        request: &ListRequest,
        ctx: &IoContext,
    ) -> Result<ResourcePage, ResourceError> {
        ctx.check(0)?;
        let path = local_path(locator)?;
        let metadata = self.dir.metadata(&path).map_err(map_local_io)?;
        if !metadata.is_dir() {
            return Err(ResourceError::CapabilityMissing);
        }
        let limit = request.limit.clamp(1, MAX_LIST_ENTRIES);
        let mut bounded = std::collections::BTreeMap::new();
        for item in self.dir.read_dir(&path).map_err(map_local_io)? {
            ctx.check(0)?;
            let entry = item.map_err(map_local_io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ResourceError::InvalidLocator)?;
            if request
                .cursor
                .as_ref()
                .is_some_and(|cursor| &name <= cursor)
            {
                continue;
            }
            bounded.insert(name, entry);
            if bounded.len() > limit + 1 {
                bounded.pop_last();
            }
        }
        let more = bounded.len() > limit;
        let mut entries = Vec::with_capacity(limit);
        for (name, entry) in bounded.into_iter().take(limit) {
            let _ = entry;
            let relative = path.join(&name);
            let metadata = self.dir.metadata(&relative).map_err(map_local_io)?;
            let locator = ResourceLocator::local_path(relative);
            entries.push(ResourceEntry {
                name,
                is_file: metadata.is_file(),
                size: metadata.is_file().then_some(metadata.len()),
                version: local_info(&locator, &metadata).version,
                locator,
            });
        }
        let next_cursor = more.then(|| entries.last().expect("nonempty page").name.clone());
        Ok(ResourcePage {
            entries,
            next_cursor,
        })
    }

    fn probe(
        &self,
        locator: &ResourceLocator,
        ctx: &IoContext,
    ) -> Result<ResourceInfo, ResourceError> {
        ctx.check(0)?;
        let path = local_path(locator)?;
        let metadata = self.dir.metadata(path).map_err(map_local_io)?;
        if !metadata.is_file() {
            return Err(ResourceError::NotFile);
        }
        Ok(local_info(locator, &metadata))
    }

    fn open(
        &self,
        locator: &ResourceLocator,
        expected: Option<&ResourceVersion>,
        ctx: &IoContext,
    ) -> Result<Box<dyn ResourceReader>, ResourceError> {
        ctx.check(0)?;
        let path = local_path(locator)?;
        let file = self.dir.open(&path).map_err(map_local_io)?;
        let metadata = file.metadata().map_err(ResourceError::Io)?;
        if !metadata.is_file() {
            return Err(ResourceError::NotFile);
        }
        let info = local_info(locator, &metadata);
        verify_expected_version(&info.version, expected)?;
        ctx.check(0)?;
        Ok(Box::new(LocalFileReader {
            file: file.into_std(),
            root: self.dir.clone(),
            path,
            info,
            cursor: 0,
            closed: false,
        }))
    }
}

struct LocalFileReader {
    file: File,
    root: Arc<Dir>,
    path: PathBuf,
    info: ResourceInfo,
    cursor: u64,
    closed: bool,
}

impl ResourceReader for LocalFileReader {
    fn info(&self) -> &ResourceInfo {
        &self.info
    }

    fn read_next(&mut self, dst: &mut [u8], ctx: &IoContext) -> Result<usize, ResourceError> {
        let read = self.read_at(self.cursor, dst, ctx)?;
        self.cursor = self.cursor.saturating_add(read as u64);
        Ok(read)
    }

    fn read_at(
        &mut self,
        offset: u64,
        dst: &mut [u8],
        ctx: &IoContext,
    ) -> Result<usize, ResourceError> {
        self.ensure_open()?;
        ctx.reserve(dst.len())?;
        verify_local_version(&self.root, &self.path, &self.info.version)?;
        if dst.is_empty() || offset >= self.info.version.size.unwrap_or(0) {
            return Ok(0);
        }
        let before = Metadata::from_file(&self.file).map_err(map_local_io)?;
        verify_expected_version(
            &local_info(&self.info.locator, &before).version,
            Some(&self.info.version),
        )?;
        let amount = read_at_file(&self.file, dst, offset).map_err(ResourceError::Io)?;
        let after = Metadata::from_file(&self.file).map_err(map_local_io)?;
        verify_expected_version(
            &local_info(&self.info.locator, &after).version,
            Some(&self.info.version),
        )?;
        verify_local_version(&self.root, &self.path, &self.info.version)?;
        ctx.check(0)?;
        Ok(amount)
    }

    fn close(&mut self, _ctx: &IoContext) -> Result<(), ResourceError> {
        self.closed = true;
        Ok(())
    }
}

impl LocalFileReader {
    fn ensure_open(&self) -> Result<(), ResourceError> {
        if self.closed {
            Err(ResourceError::Unavailable)
        } else {
            Ok(())
        }
    }
}

fn local_path(locator: &ResourceLocator) -> Result<PathBuf, ResourceError> {
    if locator.scheme() != Some("file") {
        return Err(ResourceError::InvalidLocator);
    }
    let path = locator
        .uri
        .strip_prefix("file://")
        .ok_or(ResourceError::InvalidLocator)?;
    if path.is_empty()
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(ResourceError::InvalidLocator);
    }
    Ok(PathBuf::from(path))
}

fn local_info(locator: &ResourceLocator, metadata: &Metadata) -> ResourceInfo {
    ResourceInfo {
        locator: locator.clone(),
        version: ResourceVersion {
            size: Some(metadata.len()),
            validator: local_identity(metadata),
            modified_ns: modified_ns(metadata),
        },
        caps: ResourceCaps {
            sequential_read: true,
            read_at: true,
            known_size: true,
            conditional_read: true,
        },
    }
}

fn modified_ns(metadata: &Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.into_std().duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
}

fn verify_local_version(
    root: &Dir,
    path: &Path,
    expected: &ResourceVersion,
) -> Result<(), ResourceError> {
    let metadata = root.metadata(path).map_err(map_local_io)?;
    if !metadata.is_file() {
        return Err(ResourceError::NotFile);
    }
    if expected.size != Some(metadata.len())
        || expected.modified_ns != modified_ns(&metadata)
        || expected.validator != local_identity(&metadata)
    {
        return Err(ResourceError::VersionChanged);
    }
    Ok(())
}

fn map_local_io(error: io::Error) -> ResourceError {
    match error.kind() {
        io::ErrorKind::NotFound => ResourceError::NotFound,
        io::ErrorKind::PermissionDenied => ResourceError::PermissionDenied,
        _ => ResourceError::Io(error),
    }
}

#[cfg(unix)]
fn read_at_file(file: &File, dst: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(dst, offset)
}

#[cfg(not(unix))]
fn read_at_file(file: &File, dst: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::io::{Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(dst)
}

#[cfg(unix)]
fn local_identity(metadata: &Metadata) -> Option<String> {
    use cap_std::fs::MetadataExt;
    Some(format!(
        "{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec()
    ))
}
#[cfg(not(unix))]
fn local_identity(_: &Metadata) -> Option<String> {
    None
}
