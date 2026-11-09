//! Metadata-only access under LocalCas's exclusive managed-store contract.
use crate::{
    admission::{
        files::{check_file, open_read_file, FileStamp},
        FileMode, FileOwner,
    },
    LocalCas,
};
use polar_cas::{ports::ManagedReplica, source::SourceLocator, ContentRef, Error, Result};
use std::{fs::File, os::unix::fs::MetadataExt, path::Path};

impl LocalCas {
    pub(crate) async fn open_managed_at(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
        variant: Option<(FileMode, FileOwner)>,
    ) -> Result<Option<ManagedReplica>> {
        let (store, content, locator) = (self.clone(), content.clone(), locator.clone());
        tokio::task::spawn_blocking(move || store.open_managed_sync(content, locator, variant))
            .await
            .map_err(|_| Error::Backend("local managed replica worker failed".into()))?
    }

    /// Synchronous managed access for consumers already on a blocking worker.
    pub fn open_shared_managed(&self, content: &ContentRef) -> Result<Option<ManagedReplica>> {
        if !matches!(self.key_layout, polar_cas::ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        self.open_managed_sync(
            content.clone(),
            SourceLocator {
                key: content.id.content_key(),
                revision: None,
            },
            None,
        )
    }

    fn open_managed_sync(
        &self,
        content: ContentRef,
        locator: SourceLocator,
        variant: Option<(FileMode, FileOwner)>,
    ) -> Result<Option<ManagedReplica>> {
        let key = Path::new(&locator.key);
        let (file, metadata) = match open_read_file(&self.dir, key) {
            Ok(opened) => opened,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        self.managed_entry(content, locator, variant, (file, metadata))
            .map(Some)
    }

    fn managed_entry(
        &self,
        content: ContentRef,
        locator: SourceLocator,
        variant: Option<(FileMode, FileOwner)>,
        entry: (File, std::fs::Metadata),
    ) -> Result<ManagedReplica> {
        let store = self.clone();
        let (file, metadata) = entry;
        // Ordinary commits retain owner-private writable modes (e.g. 0600).
        // Exclusive store ownership, not chmod, promises immutable content.
        // Group/world write or special modes are outside that contract.
        if !metadata.is_file()
            || metadata.len() != content.size_bytes
            || metadata.mode() & 0o7022 != 0
        {
            return Err(Error::Integrity);
        }
        if let Some((mode, owner)) = variant {
            check_file(&file, content.size_bytes, mode, owner)?;
        }
        let stamp = FileStamp::of(&metadata);
        let current_locator = locator.clone();
        ManagedReplica::from_trusted_adapter(file, content, locator, move |pinned: &File| {
            let (_current, current_metadata) =
                open_read_file(&store.dir, Path::new(&current_locator.key)).map_err(|error| {
                    match error {
                        Error::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Error::NotFound
                        }
                        error => error,
                    }
                })?;
            if FileStamp::of(&current_metadata) != stamp
                || FileStamp::of(&pinned.metadata()?) != stamp
            {
                return Err(Error::Integrity);
            }
            Ok(())
        })
    }
}
