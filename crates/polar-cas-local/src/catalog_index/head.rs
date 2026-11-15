//! Group-shared Catalog pointer, following the existing shared-index publication
//! protocol. Immutable records live in the injected CAS, not this directory.
use crate::shared_index::binary::{Decoder, Encoder};
use cap_std::fs::Dir;
use polar_cas::{canonical::canonical_json_bytes, ContentRef, Error, Result};
use rustix::fs::{openat, Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

pub(crate) struct Head {
    directory: Dir,
    binding: Option<(crate::LocalCas, std::path::PathBuf)>,
    
}

impl Head {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::Invalid(
                "absolute Catalog index directory required".into(),
            ));
        }
        std::fs::create_dir_all(path)?;
        let directory = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        Ok(Self {
            directory,
            binding: None,
            
        })
    }
    pub(crate) fn open_bound(storage: &crate::LocalCas, path: &Path) -> Result<Self> {
        let _acquisition = crate::shared_index::lifecycle::acquire(storage)?;
        let mut head = Self::open(path)?;
        let path = path.canonicalize()?;
        crate::shared_index::lifecycle::register_head(storage, &path, head.current()?)?;
        head.binding = Some((storage.clone(), path));
        Ok(head)
    }
    pub(crate) fn current_at(path: &Path) -> Result<Option<ContentRef>> {
        Self {
            directory: Dir::open_ambient_dir(path, cap_std::ambient_authority())?,
            binding: None,
            
        }
        .current()
    }
    fn open_file(&self, name: &str, flags: OFlags) -> std::io::Result<File> {
        Ok(File::from(openat(
            &self.directory,
            name,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )?))
    }
    /// Hold only during compare-and-publish, never for a reader's lifetime.
    pub fn writer(&self) -> Result<File> {
        
        let writer = self.open_file("writer.lock", OFlags::RDWR | OFlags::CREATE)?;
        writer.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => Error::Conflict,
            std::fs::TryLockError::Error(e) => Error::Io(e),
        })?;
        ;
        Ok(writer)
    }
    /// First projection selection waits for this same short publication lock.
    /// Ordinary expected-head updates retain the nonblocking conflict contract.
    pub fn initialization_writer(&self) -> Result<File> {
        let writer = self.open_file("writer.lock", OFlags::RDWR | OFlags::CREATE)?;
        writer.lock()?;
        Ok(writer)
    }
    pub fn current(&self) -> Result<Option<ContentRef>> {
        let file = match self.open_file("current", OFlags::RDONLY) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if !file.metadata()?.is_file() || file.metadata()?.len() > 1024 {
            return Err(Error::Integrity);
        }
        let mut bytes = Vec::new();
        file.take(1025).read_to_end(&mut bytes)?;
        decode_current(&bytes).map(Some)
    }

    /// Caller holds `writer()`. A visible pointer after a failed rename/sync
    /// cycle is not a durable acknowledgement until the directory sync succeeds.
    pub fn confirm(&self, reference: &ContentRef) -> Result<()> {
        if self.current()?.as_ref() != Some(reference) {
            return Err(Error::Conflict);
        }
        self.sync_parent()
    }
    fn sync_parent(&self) -> Result<()> {
        
        self.directory
            .open(".")
            .and_then(|directory| directory.sync_all())
            .map_err(|_| Error::OutcomeUncertain)
    }
    
    pub fn publish(&self, reference: &ContentRef) -> Result<()> {
        let _acquisition = self
            .binding
            .as_ref()
            .map(|(store, _)| crate::shared_index::lifecycle::acquire(store))
            .transpose()?;
        if let Some((store, path)) = &self.binding {
            crate::shared_index::lifecycle::selecting(store, path, reference)?;
        }

        // One locked writer owns this staging name; never interpret an
        // interrupted pending file as a successful publication.
        match self.directory.remove_file(".pending-current") {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let mut pending = self.open_file(
            ".pending-current",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        )?;
        reference.id.require_sha256()?;
        let mut writer = Encoder::new(b"PCASHEA2", 1024)?;
        writer.content(reference)?;
        let bytes = writer.finish();
        
        pending.write_all(&bytes)?;
        pending.sync_all()?;
        ;
        drop(pending);
        self.directory
            .rename(".pending-current", &self.directory, "current")?;
        self.sync_parent()
    }
}

/// Decode the same current pointer for readers and capability-based audits.
pub(crate) fn decode_current(bytes: &[u8]) -> Result<ContentRef> {
    if bytes.len() > 1024 {
        return Err(Error::TooLarge);
    }
    let reference: ContentRef = if bytes.starts_with(b"PCASHEA2") {
        let mut reader = Decoder::new(bytes, b"PCASHEA2", 1024)?;
        let reference = reader.content()?;
        reader.finish()?;
        reference
    } else {
        let reference: ContentRef = serde_json::from_slice(bytes).map_err(|_| Error::Integrity)?;
        if canonical_json_bytes(&reference).map_err(|_| Error::Integrity)? != bytes {
            return Err(Error::Integrity);
        }
        reference
    };
    reference.id.require_sha256()?;
    Ok(reference)
}


