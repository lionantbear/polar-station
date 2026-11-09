//! Local immutable mode variants; no candidate or authorization semantics.
use crate::{
    link::{file_variant_path, link_verified_file, LinkOutcome},
    LocalCas,
};
use cap_std::{
    ambient_authority,
    fs::{Dir, MetadataExt as _},
};
use polar_cas::{
    source::SourceLocator, verification::VerifiedContent, ContentRef, Error, ObjectId, Result,
};
use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};
mod backend;
pub(crate) mod files;
mod placement;
use files::{check_file, open_absolute, open_relative, verify_file, FileStamp};

pub use backend::LocalAdmissionBackend;
pub use polar_cas::projection::{FileMode, FileOwner, VariantPolicy};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Preparation {
    Reused,
    Hardlink,
    Reflink,
    Copy,
}

/// In-process trusted local material, never a deserializable content claim.
#[derive(Clone)]
pub struct LocalFile {
    path: PathBuf,
    content: VerifiedContent,
    file: Arc<File>,
    stamp: FileStamp,
}
impl LocalFile {
    /// Inspect caller-proven immutable bytes without reading payload or writing
    /// a CAS directory. The witness must originate from trusted admission, not
    /// an untrusted content claim. Keeps the opened inode alive for projection
    /// verification and rejects path, mode, owner or size drift.
    pub fn inspect_trusted(
        path: &Path,
        content: &VerifiedContent,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<Self> {
        let (_, _, file) = open_absolute(path)?;
        let metadata = check_file(&file, content.size_bytes(), mode, owner)?;
        let inspected = Self {
            path: path.to_owned(),
            content: content.clone(),
            file: Arc::new(file),
            stamp: FileStamp::of(&metadata),
        };
        inspected.revalidate()?;
        Ok(inspected)
    }

    pub(crate) fn mode(&self) -> FileMode {
        if self.stamp.executable() {
            FileMode::Executable
        } else {
            FileMode::ReadOnly
        }
    }
    pub(crate) fn validate_for(
        &self,
        content: &ContentRef,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<()> {
        self.revalidate()?;
        if self.content.reference(&content.id)? != *content {
            return Err(Error::Integrity);
        }
        check_file(&self.file, content.size_bytes, mode, owner)?;
        Ok(())
    }
    pub(crate) fn sync(&self) -> Result<()> {
        self.revalidate()?;
        self.file.sync_all()?;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn content(&self) -> &VerifiedContent {
        &self.content
    }
    /// Strict same-inode linking, distinct from content deduplication.
    /// Caller owns the destination directory and its publication lifecycle.
    pub fn link_to(&self, destination: &Path) -> Result<LinkOutcome> {
        self.revalidate()?;
        files::absolute_parent(destination)?;
        let outcome = link_verified_file(&self.path, destination)?;
        self.verify_link(destination)?;
        Ok(outcome)
    }
    pub fn verify_link(&self, destination: &Path) -> Result<()> {
        self.revalidate()?;
        let (_, _, file) = open_absolute(destination)?;
        if FileStamp::of(&file.metadata()?) != self.stamp {
            return Err(Error::Integrity);
        }
        Ok(())
    }
    fn revalidate(&self) -> Result<()> {
        let (_, _, file) = open_absolute(&self.path)?;
        if FileStamp::of(&file.metadata()?) != self.stamp
            || FileStamp::of(&self.file.metadata()?) != self.stamp
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
pub struct AdmissionReceipt {
    pub file: LocalFile,
    pub preparation: Preparation,
    pub read_bytes: u64,
    pub copied_bytes: u64,
}
impl LocalCas {
    /// Caller proves these bytes are immutable and verified. Never hashes,
    /// copies, changes source mode/owner, or invents additional digest aliases.
    pub async fn admit_trusted(
        &self,
        source: &Path,
        content: &VerifiedContent,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<AdmissionReceipt> {
        let (store, source, content) = (self.clone(), source.to_owned(), content.clone());
        tokio::task::spawn_blocking(move || {
            let id = content.preferred().id;
            store.admit_path(&source, &id, content, mode, owner)
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }
    /// Verify local bytes and explicitly prepare an immutable mode variant.
    pub async fn prepare_file(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
        mode: FileMode,
        owner: FileOwner,
        policy: VariantPolicy,
    ) -> Result<AdmissionReceipt> {
        super::native::validate_artifact(locator)?;
        let (store, content, locator) = (self.clone(), content.clone(), locator.clone());
        tokio::task::spawn_blocking(move || store.prepare(&content, &locator, mode, owner, policy))
            .await
            .map_err(|_| Error::OutcomeUncertain)?
    }

    fn variant_key(&self, id: &ObjectId, mode: FileMode) -> Result<PathBuf> {
        Ok(file_variant_path(&self.root, id, mode.bits())?
            .strip_prefix(&self.root)
            .map_err(|_| Error::Integrity)?
            .to_owned())
    }

    fn native_variant(&self, locator: &SourceLocator) -> Result<(ObjectId, FileMode)> {
        super::native::validate_artifact(locator)?;
        let parts: Vec<_> = locator.key.split('/').collect();
        let ["blobs", algorithm, digest, mode] = parts.as_slice() else {
            return Err(Error::Conflict);
        };
        let mode = match *mode {
            "mode-0444" => FileMode::ReadOnly,
            "mode-0555" => FileMode::Executable,
            _ => return Err(Error::Conflict),
        };
        let id = ObjectId::try_from(format!("{algorithm}:{digest}"))?;
        if self.variant_key(&id, mode)? != Path::new(&locator.key) {
            return Err(Error::Conflict);
        }
        Ok((id, mode))
    }

    fn source_variant_id(&self, expected: &ContentRef, locator: &SourceLocator) -> ObjectId {
        ObjectId::from_content_key(&locator.key)
            .or_else(|_| self.native_variant(locator).map(|(id, _)| id))
            // Other confined source paths are not content-address keys. Their
            // bytes still must prove the expected content in prepare/inspect.
            .unwrap_or_else(|_| expected.id.clone())
    }

    fn admit_path(
        &self,
        source: &Path,
        variant_id: &ObjectId,
        content: VerifiedContent,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<AdmissionReceipt> {
        content.reference(variant_id)?;
        let (dir, name, file) = open_absolute(source)?;
        self.admit_open((&dir, &name, file), variant_id, content, mode, owner)
    }

    fn admit_open(
        &self,
        (source, name, file): (&Dir, &std::ffi::OsStr, File),
        variant_id: &ObjectId,
        content: VerifiedContent,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<AdmissionReceipt> {
        let metadata = check_file(&file, content.size_bytes(), mode, owner)?;
        let key = self.variant_key(variant_id, mode)?;
        self.ensure_variant_parent(&key)?;
        if self.dir.dir_metadata()?.dev() != metadata.dev() {
            return Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices).into());
        }
        // Stage the hardlink privately, then prove it is the opened immutable
        // source. A path substitution cannot publish an unrelated inode.
        let stage = tempfile::Builder::new()
            .prefix(".cas-admit-")
            .tempdir_in(&self.root)?;
        let private = Dir::open_ambient_dir(stage.path(), ambient_authority())?;
        source.hard_link(name, &private, "file")?;
        let (_, _, staged) = open_relative(&private, Path::new("file"))?;
        if FileStamp::of(&staged.metadata()?) != FileStamp::of(&metadata)
            || FileStamp::of(&file.metadata()?) != FileStamp::of(&metadata)
        {
            return Err(Error::Integrity);
        }
        let reused = match private.hard_link("file", &self.dir, &key) {
            Ok(()) => false,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => true,
            Err(e) => return Err(e.into()),
        };
        let (_, _, target) = open_relative(&self.dir, &key)?;
        let target_metadata = check_file(&target, content.size_bytes(), mode, owner)?;
        if !reused && FileStamp::of(&target_metadata) != FileStamp::of(&metadata) {
            return Err(Error::Integrity);
        }
        // Trusted admission can deduplicate different source inodes. The
        // controlled immutable CAS and supplied content witness are its trust
        // boundary; ordinary prepare_file rehashes existing variants instead.
        target.sync_all()?;
        self.sync_parents(
            key.to_str()
                .ok_or_else(|| Error::Invalid("variant key".into()))?,
        )?;
        let local = LocalFile {
            path: self.root.join(key),
            content,
            file: Arc::new(target),
            stamp: FileStamp::of(&target_metadata),
        };
        local.revalidate()?;
        Ok(AdmissionReceipt {
            file: local,
            preparation: if reused {
                Preparation::Reused
            } else {
                Preparation::Hardlink
            },
            read_bytes: 0,
            copied_bytes: 0,
        })
    }

    fn ensure_variant_parent(&self, key: &Path) -> Result<()> {
        let parent = key.parent().ok_or(Error::Integrity)?;
        let mut dir = self.dir.try_clone()?;
        for component in parent.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(Error::Integrity);
            };
            match dir.create_dir(name) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            files::check_directory(&dir.symlink_metadata(name)?)?;
            dir = dir.open_dir(name)?;
        }
        Ok(())
    }

    pub(crate) fn inspect_variant(
        &self,
        expected: &ContentRef,
        locator: &SourceLocator,
        mode: FileMode,
        owner: FileOwner,
    ) -> Result<Option<AdmissionReceipt>> {
        super::native::validate_artifact(locator)?;
        let variant_id = self.source_variant_id(expected, locator);
        self.inspect_variant_at(expected, &variant_id, mode, owner)
    }

    pub(crate) fn observe_existing_variant(
        &self, expected: &ContentRef, mode: FileMode, owner: FileOwner,
    ) -> Result<Option<LocalFile>> {
        Ok(self.inspect_variant_at_inner(expected, &expected.id, mode, owner, false)?.map(|r| r.file))
    }

    fn inspect_variant_at(
        &self, expected: &ContentRef, variant_id: &ObjectId, mode: FileMode, owner: FileOwner,
    ) -> Result<Option<AdmissionReceipt>> {
        self.inspect_variant_at_inner(expected, variant_id, mode, owner, true)
    }

    fn inspect_variant_at_inner(
        &self, expected: &ContentRef, variant_id: &ObjectId, mode: FileMode, owner: FileOwner,
        durable: bool,
    ) -> Result<Option<AdmissionReceipt>> {
        let key = self.variant_key(variant_id, mode)?;
        let (_, _, mut file) = match open_relative(&self.dir, &key) {
            Ok(value) => value,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = check_file(&file, expected.size_bytes, mode, owner)?;
        let content = verify_file(&mut file, expected, variant_id, None)?;
        if durable {
            file.sync_all()?;
            self.sync_parents(key.to_str().ok_or(Error::Integrity)?)?;
        }
        let local = LocalFile {
            path: self.root.join(key),
            content,
            stamp: FileStamp::of(&metadata),
            file: Arc::new(file),
        };
        local.revalidate()?;
        Ok(Some(AdmissionReceipt {
            file: local,
            preparation: Preparation::Reused,
            read_bytes: expected.size_bytes,
            copied_bytes: 0,
        }))
    }

    pub(crate) fn prepare(
        &self,
        expected: &ContentRef,
        locator: &SourceLocator,
        mode: FileMode,
        owner: FileOwner,
        policy: VariantPolicy,
    ) -> Result<AdmissionReceipt> {
        // An alias reuses the physical replica's variant key after proving
        // both digests; it does not create another copy for the alias.
        let variant_id = self.source_variant_id(expected, locator);
        let key = self.variant_key(&variant_id, mode)?;
        self.ensure_variant_parent(&key)?;
        if let Some(existing) = self.inspect_variant(expected, locator, mode, owner)? {
            return Ok(existing);
        }
        let (source_dir, name, mut source) = open_relative(&self.dir, Path::new(&locator.key))?;
        let metadata = source.metadata()?;
        if !metadata.is_file() || metadata.len() != expected.size_bytes {
            return Err(Error::Integrity);
        }
        if check_file(&source, expected.size_bytes, mode, owner).is_ok() {
            let content = verify_file(&mut source, expected, &variant_id, None)?;
            let mut result = self.admit_open(
                (&source_dir, &name, source),
                &variant_id,
                content,
                mode,
                owner,
            )?;
            result.read_bytes = expected.size_bytes;
            verify_racing_reuse(&mut result, expected, &variant_id)?;
            return Ok(result);
        }
        if policy == VariantPolicy::ExistingOnly {
            return Err(Error::UnsupportedCapability);
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        let reflinked = files::try_reflink(&source, temporary.as_file())?;
        let content = if reflinked {
            verify_file(temporary.as_file_mut(), expected, &variant_id, None)?
        } else if policy == VariantPolicy::CopyAllowed {
            verify_file(
                &mut source,
                expected,
                &variant_id,
                Some(temporary.as_file_mut()),
            )?
        } else {
            return Err(Error::UnsupportedCapability);
        };
        files::set_private_metadata(temporary.as_file(), mode, owner)?;
        temporary.as_file().sync_all()?;
        let (dir, name, file) = open_absolute(temporary.path())?;
        let mut result = self.admit_open((&dir, &name, file), &variant_id, content, mode, owner)?;
        if result.preparation != Preparation::Reused {
            result.preparation = if reflinked {
                Preparation::Reflink
            } else {
                Preparation::Copy
            };
        }
        result.read_bytes = expected.size_bytes;
        result.copied_bytes = if reflinked { 0 } else { expected.size_bytes };
        verify_racing_reuse(&mut result, expected, &variant_id)?;
        Ok(result)
    }
}

fn verify_racing_reuse(
    result: &mut AdmissionReceipt,
    expected: &ContentRef,
    variant_id: &ObjectId,
) -> Result<()> {
    // Ordinary verification cannot inherit the zero-read trusted-admission
    // shortcut when another operation wins creation after our initial probe.
    if result.preparation == Preparation::Reused {
        result.file.revalidate()?;
        result.file.content = verify_file(
            &mut result.file.file.try_clone()?,
            expected,
            variant_id,
            None,
        )?;
        result.read_bytes = result
            .read_bytes
            .checked_add(expected.size_bytes)
            .ok_or(Error::TooLarge)?;
    }
    Ok(())
}
