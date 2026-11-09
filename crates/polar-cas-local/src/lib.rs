//! Local immutable object storage. All object traversal is confined by Dir.
#[cfg(unix)]
pub mod admission;
#[cfg(unix)]
pub mod backup_audit;
pub mod catalog_index;
#[cfg(unix)]
mod index_publication;
pub mod link;
#[cfg(unix)]
mod managed;
#[cfg(unix)]
pub mod materialize;
mod native;
pub mod resource;
#[cfg(unix)]
pub mod shared_index;
#[cfg(unix)]
mod writer;
// Reuse the existing example accounting helpers in library-only benchmarks.


use async_trait::async_trait;
use cap_std::{ambient_authority, fs::Dir};
pub use native::LocalSource;
use polar_cas::{
    CasBackend, ContentRef, Error, ObjectId, ObjectKeyLayout, ObjectRef, Reader, Result,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// An exclusively managed immutable object root. Callers must not edit admitted
/// payload in place or leave writable producer handles live after publication.
/// Ordinary committed files may retain owner-private writable permissions;
/// immutability is the store's ownership contract, not a mode-bit guarantee.
/// `ReplicaWriter::open_managed` relies on that contract plus the caller's Ready
/// Catalog association. Explicit inspection remains necessary to audit storage
/// corruption or changes made outside the managed-store contract.
#[derive(Clone)]
pub struct LocalCas {
    #[cfg(unix)]
    save_binding: Option<Arc<writer::LocalSaveBinding>>,
    root: PathBuf,
    dir: Arc<Dir>,
    key_layout: ObjectKeyLayout,
}
impl LocalCas {
    pub fn open(root: PathBuf) -> Result<Self> {
        Self::open_with_layout(root, ObjectKeyLayout::NamespaceScoped)
    }
    pub fn open_with_layout(root: PathBuf, key_layout: ObjectKeyLayout) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::Invalid(
                "explicit absolute local CAS root required".into(),
            ));
        }
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let dir = Arc::new(Dir::open_ambient_dir(&root, ambient_authority())?);
        Ok(Self {
            #[cfg(unix)]
            save_binding: None,
            root,
            dir,
            key_layout,
        })
    }
    /// Bounded loose-content read, also available without Linux native images.
    /// The caller explicitly owns this immutable SharedContent namespace.
    pub fn read_shared_bounded(&self, content: &ContentRef, max_bytes: u64) -> Result<Vec<u8>> {
        use std::io::Read;
        if content.size_bytes > max_bytes {
            return Err(Error::TooLarge);
        }
        let mut file = self.shared_loose_file(content)?.ok_or(Error::NotFound)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != content.size_bytes || file.metadata()?.len() != content.size_bytes
        {
            return Err(Error::Integrity);
        }
        Ok(bytes)
    }
    pub fn open_shared_reader(&self, content: &ContentRef) -> Result<Option<Reader>> {
        Ok(self
            .shared_loose_file(content)?
            .map(|file| Box::pin(tokio::fs::File::from_std(file)) as Reader))
    }
    pub fn contains_shared(&self, content: &ContentRef) -> Result<bool> {
        Ok(self.shared_loose_file(content)?.is_some())
    }
    fn shared_loose_file(&self, content: &ContentRef) -> Result<Option<std::fs::File>> {
        #[cfg(unix)]
        {
            self.open_shared_file(content)
        }
        #[cfg(not(unix))]
        {
            if self.key_layout != ObjectKeyLayout::SharedContent {
                return Err(Error::UnsupportedCapability);
            }
            let key = content.id.content_key();
            let metadata = match self.dir.symlink_metadata(&key) {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !metadata.is_file() || metadata.len() != content.size_bytes {
                return Err(Error::Integrity);
            }
            let file = self.dir.open(&key)?.into_std();
            if !file.metadata()?.is_file() || file.metadata()?.len() != content.size_bytes {
                return Err(Error::Integrity);
            }
            Ok(Some(file))
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Publish caller-owned bytes to this existing SharedContent store. The
    /// identity is derived here; callers cannot assert unverified file content.
    /// Synchronous owners (such as index generation builders) call this outside
    /// async worker threads. Publication uses the same immutable-file primitive.
    pub fn publish_shared_bytes(&self, bytes: &[u8]) -> Result<ContentRef> {
        let content = self.publish_shared_generated_bytes(bytes)?;
        #[cfg(unix)]
        self.record_shared_save(&content)?;
        Ok(content)
    }

    /// Publish domain-generated bytes without attributing an original user save.
    /// This grants no artifact role or backup exemption. The domain must retain
    /// and include these ordinary CAS bytes in its explicit backup closure.
    pub fn publish_shared_generated_bytes(&self, bytes: &[u8]) -> Result<ContentRef> {
        use std::io::Write;
        if !matches!(self.key_layout, ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        let content = ContentRef {
            id: ObjectId::for_bytes(bytes),
            size_bytes: bytes.len() as u64,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(self.root())?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        self.publish_file(&content, &content.id.content_key(), temporary.path())?;
        Ok(content)
    }

    #[cfg(unix)]
    pub(crate) fn index_temporary(&self) -> Result<tempfile::NamedTempFile> {
        let directory = ".polar-index-artifacts-v1/tmp";
        self.dir.create_dir_all(directory)?;
        Ok(tempfile::NamedTempFile::new_in(self.root.join(directory))?)
    }
    /// Index artifacts remain ordinary addressable CAS bytes. The additional
    /// immutable hard link records their role before making the content visible,
    /// so loose recovery cannot recursively index the index's own output.
    #[cfg(unix)]
    pub(crate) fn publish_index_artifact(&self, object: &ContentRef, staged: &Path) -> Result<()> {
        let _acquisition = shared_index::lifecycle::acquire(self)?;
        let marker = format!(".polar-index-artifacts-v1/{}", object.id.content_key());
        let existing = self.root.join(object.id.content_key());
        let source = if self.contains_shared(object)? {
            if !self.is_index_artifact(&object.id)? {
                shared_index::lifecycle::keep(self, object)?;
            }
            existing.as_path()
        } else {
            staged
        };
        self.link_file(object, &marker, source)?;
        self.sync_parents(&marker)?;
        // Reuse the already published inode on repeated publication as well.
        self.link_file(object, &object.id.content_key(), &self.root.join(marker))?;
        self.sync_parents(&object.id.content_key())?;
        if !self.is_index_artifact(&object.id)? {
            shared_index::lifecycle::keep(self, object)?;
        }
        Ok(())
    }
    #[cfg(unix)]
    pub fn is_index_artifact(&self, id: &ObjectId) -> Result<bool> {
        Self::index_artifact_in(&self.dir, id)
    }
    #[cfg(unix)]
    pub(crate) fn index_artifact_in(root: &Dir, id: &ObjectId) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;
        let marker = format!(".polar-index-artifacts-v1/{}", id.content_key());
        let marker = match admission::files::open_read_file(root, Path::new(&marker)) {
            Ok((file, _)) => file.metadata()?,
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let (file, _) = admission::files::open_read_file(root, Path::new(&id.content_key()))?;
        let actual = file.metadata()?;
        Ok(marker.dev() == actual.dev()
            && marker.ino() == actual.ino()
            && marker.len() == actual.len())
    }

    #[cfg(unix)]
    fn retain_business_collision(&self, object: &ContentRef) -> Result<()> {
        let marker = format!(".polar-index-artifacts-v1/{}", object.id.content_key());
        match admission::files::open_read_file(&self.dir, Path::new(&marker)) {
            Ok(_) => shared_index::lifecycle::keep(self, object),
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn publish_file(&self, object: &ContentRef, key: &str, staged: &Path) -> Result<bool> {
        #[cfg(unix)]
        let _acquisition = if matches!(self.key_layout, ObjectKeyLayout::SharedContent) {
            Some(shared_index::lifecycle::acquire(self)?)
        } else {
            None
        };
        let reused = self.link_file(object, key, staged)?;
        #[cfg(unix)]
        if _acquisition.is_some() {
            self.retain_business_collision(object)?;
        }
        self.sync_parents(key)?;
        Ok(reused)
    }

    fn link_file(&self, object: &ContentRef, key: &str, staged: &Path) -> Result<bool> {
        let parent = Path::new(key)
            .parent()
            .ok_or_else(|| Error::Invalid("object key".into()))?;
        self.dir.create_dir_all(parent)?;
        let source_parent = staged
            .parent()
            .ok_or_else(|| Error::Invalid("staging parent".into()))?;
        let source_name = staged
            .file_name()
            .ok_or_else(|| Error::Invalid("staging name".into()))?;
        let source_dir = Dir::open_ambient_dir(source_parent, ambient_authority())?;
        let metadata = source_dir.symlink_metadata(source_name)?;
        if !metadata.is_file() || metadata.len() != object.size_bytes {
            return Err(Error::Integrity);
        }
        // Only runtime-owned, closed and synchronized staging files reach this
        // port. Reuse their inode on the same filesystem, never an arbitrary
        // mutable imported file. A cross-device ordinary upload uses a private
        // copy; trusted delivery's separate link capability never does.
        let reused = match source_dir.hard_link(source_name, &self.dir, key) {
            Ok(()) => false,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_existing(object, key)?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                let temporary = tempfile::NamedTempFile::new_in(&self.root)?;
                let mut input = source_dir.open(source_name)?;
                let mut output = temporary.as_file();
                if io::copy(&mut input, &mut output)? != object.size_bytes {
                    return Err(Error::Integrity);
                }
                output.sync_all()?;
                let name = temporary
                    .path()
                    .file_name()
                    .ok_or_else(|| Error::Invalid("temporary name".into()))?;
                match self.dir.hard_link(name, &self.dir, key) {
                    Ok(()) => false,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        self.verify_existing(object, key)?;
                        true
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        };
        Ok(reused)
    }

    fn sync_directory(&self, path: &Path) -> Result<()> {
        
        
        self.dir.open(path)?.into_std().sync_all()?;
        Ok(())
    }

    fn sync_parents(&self, key: &str) -> Result<()> {
        let parent = Path::new(key)
            .parent()
            .ok_or_else(|| Error::Invalid("object key".into()))?;
        // Persist each directory link, including newly created namespace/hash
        // directories, before acknowledging local durable completion.
        let mut directory = Some(parent);
        while let Some(path) = directory {
            let path = if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                path
            };
            // Dir may hold an O_PATH descriptor on Linux; fsync requires a
            // readable directory descriptor, not that traversal-only handle.
            self.sync_directory(path)?;
            directory = if path == Path::new(".") {
                None
            } else {
                path.parent()
            };
        }
        Ok(())
    }
    fn verify_existing(&self, object: &ContentRef, key: &str) -> Result<()> {
        #[cfg(unix)]
        if matches!(self.key_layout, ObjectKeyLayout::SharedContent)
            && key == object.id.content_key()
        {
            self.open_shared_file(object)?.ok_or(Error::NotFound)?;
            return Ok(());
        }
        let mut file = self.dir.open(key)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != object.size_bytes {
            return Err(Error::Integrity);
        }
        if ObjectId::for_reader_with(object.id.algorithm(), &mut file)? != object.id {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
#[async_trait]
impl CasBackend for LocalCas {
    async fn publish(&self, object: &ObjectRef, staged_file: &Path) -> Result<()> {
        object.object_id.require_sha256()?;
        let store = self.clone();
        let object = object.clone();
        let staged = staged_file.to_path_buf();
        // Copying/filesystem synchronization must not block a Tokio worker.
        tokio::task::spawn_blocking(move || {
            let key = store.key_layout.key(&object);
            store
                .publish_file(
                    &ContentRef {
                        id: object.object_id,
                        size_bytes: object.size_bytes,
                    },
                    &key,
                    &staged,
                )
                .map(|_| ())
        })
        .await
        .map_err(|_| Error::Backend("local CAS worker failed".into()))?
    }
    async fn open(&self, object: &ObjectRef) -> Result<Reader> {
        object.object_id.require_sha256()?;
        let file = self
            .dir
            .open(self.key_layout.key(object))
            .map_err(map_missing)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != object.size_bytes {
            return Err(Error::Integrity);
        }
        Ok(Box::pin(tokio::fs::File::from_std(file.into_std())))
    }
    async fn stat(&self, object: &ObjectRef) -> Result<Option<u64>> {
        object.object_id.require_sha256()?;
        match self.dir.metadata(self.key_layout.key(object)) {
            Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
            Ok(_) => Err(Error::Integrity),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
fn map_missing(error: io::Error) -> Error {
    if error.kind() == io::ErrorKind::NotFound {
        Error::NotFound
    } else {
        error.into()
    }
}

#[cfg(all(feature = "tree-ingest", target_os = "linux"))]
pub mod observe;

#[cfg(all(feature = "tree-ingest", target_os = "linux"))]
pub mod locked_reader;
#[cfg(all(feature = "tree-ingest", target_os = "linux"))]
pub mod locked_tree;


