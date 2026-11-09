//! Read-only EROFS Location adapter. Remote preparation and credentials stay
//! in the runtime/Host. Construction retains a verified compile/read lease;
//! a serialized receipt or a pathname cannot manufacture that witness.
use crate::{
    async_reader::{BlockingReader, ReadBudget},
    host::PreparedImage,
    projection::{cas_error, descriptor},
    CasImageLease, Image, Member,
};
use async_trait::async_trait;
use polar_cas::{
    native_fs::ErofsPackBinding,
    ports::SourceReader,
    source::{OpaqueAccessRef, SourceLocator},
    Error, ObjectId, Reader, Result,
};
use std::{
    io::{self, Read},
    sync::{Arc, Mutex},
};

/// Metadata only: at most 512 successful member templates per opened Pack.
const MEMBER_CACHE_CAPACITY: usize = 512;
struct CachedImage {
    image: Image,
    members: Option<lru::LruCache<[u8; 32], Member>>,
}
impl CachedImage {
    fn new(image: Image) -> Self {
        Self {
            image,
            members: None,
        }
    }
}
pub struct ErofsSourceReader {
    image: Arc<Mutex<CachedImage>>,
    prepared: Arc<CasImageLease>,
    binding: ErofsPackBinding,
    budget: ReadBudget,
}
impl ErofsSourceReader {
    /// Explicit strict Host-prepared compatibility reader.
    pub async fn new(
        binding: ErofsPackBinding,
        prepared: PreparedImage,
        budget: ReadBudget,
    ) -> Result<Self> {
        Self::from_lease(binding, CasImageLease::from_prepared(prepared)?, budget).await
    }
    /// Unprivileged reader. Exact-image leases open only image metadata;
    /// explicit imported snapshots were already verified at construction.
    pub async fn from_lease(
        binding: ErofsPackBinding,
        prepared: CasImageLease,
        budget: ReadBudget,
    ) -> Result<Self> {
        binding.validate()?;
        if &binding.image != prepared.content() {
            return Err(Error::Integrity);
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| Error::Backend(format!("native reader requires Tokio runtime: {e}")))?;
        let prepared = Arc::new(prepared);
        let owner = prepared.clone();
        let permit = budget.acquire().await?;
        let image = runtime
            .spawn_blocking(move || {
                let _permit = permit;
                Image::open_owned_lease(owner)
            })
            .await
            .map_err(|e| Error::Backend(format!("native image worker: {e}")))??;
        Ok(Self {
            image: Arc::new(Mutex::new(CachedImage::new(image))),
            prepared,
            binding,
            budget,
        })
    }
    pub fn binding(&self) -> &ErofsPackBinding {
        &self.binding
    }
    /// Prepare once for a caller already running on a bounded blocking worker.
    pub fn from_lease_blocking(
        binding: ErofsPackBinding,
        prepared: CasImageLease,
        budget: ReadBudget,
    ) -> Result<Self> {
        binding.validate()?;
        if &binding.image != prepared.content() {
            return Err(Error::Integrity);
        }
        let prepared = Arc::new(prepared);
        let image = Image::open_owned_lease(prepared.clone())?;
        Ok(Self {
            image: Arc::new(Mutex::new(CachedImage::new(image))),
            prepared,
            binding,
            budget,
        })
    }
    /// Memory-only open of a fully cached group. A busy/missing cache returns
    /// None so an async caller never waits for native lookup on its own thread.
    /// The retained immutable lease is the authority, not a new pathname check.
    pub fn try_open_cached_members(
        &self,
        contents: &[polar_cas::ContentRef],
    ) -> Result<Option<Vec<Result<Reader>>>> {
        if contents.len() > 1024 {
            return Err(Error::TooLarge);
        }
        let rows: Vec<_> = contents.iter().map(descriptor).collect::<Result<_>>()?;
        let mut cached = match self.image.try_lock() {
            Ok(cached) => cached,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(_) => return Err(Error::Backend("native image cache poisoned".into())),
        };
        let Some(members) = cached.members.as_mut() else {
            return Ok(None);
        };
        if rows.iter().any(|row| !members.contains(&row.sha256)) {
            return Ok(None);
        }
        let opened = rows
            .iter()
            .map(|row| {
                let member = members.get(&row.sha256).expect("checked cache entry");
                if member.len() != row.size_bytes {
                    return Err(crate::Error {
                        status: polar_cas_fs_sys::PCFS_CORRUPT,
                    });
                }
                member.try_clone()
            })
            .collect();
        drop(cached);
        Ok(Some(self.wrap_members(opened)))
    }
    fn wrap_members(
        &self,
        members: Vec<std::result::Result<Member, crate::Error>>,
    ) -> Vec<Result<Reader>> {
        members
            .into_iter()
            .map(|result| {
                result.map_err(cas_error).and_then(|member| {
                    Ok(Box::pin(BlockingReader::new(
                        PinnedMember { member },
                        self.budget.clone(),
                    )?) as Reader)
                })
            })
            .collect()
    }
    /// Open already-admitted members together. No payload or extent audit;
    /// each returned stream has its own cursor and pins the original image.
    /// Caller holds a ReadBudget permit on a blocking worker.
    pub fn open_members_blocking(
        &self,
        contents: &[polar_cas::ContentRef],
    ) -> Result<Vec<Result<Reader>>> {
        if contents.len() > 1024 {
            return Err(Error::TooLarge);
        }
        let rows: Vec<_> = contents.iter().map(descriptor).collect::<Result<_>>()?;
        let mut cached = self
            .image
            .lock()
            .map_err(|_| Error::Backend("native image cache poisoned".into()))?;
        let mut members: Vec<Option<std::result::Result<Member, crate::Error>>> =
            (0..rows.len()).map(|_| None).collect();
        let mut misses = Vec::new();
        let mut slots = Vec::new();
        for (slot, row) in rows.iter().enumerate() {
            if let Some(member) = cached
                .members
                .as_mut()
                .and_then(|members| members.get(&row.sha256))
            {
                members[slot] = Some(if member.len() == row.size_bytes {
                    member.try_clone()
                } else {
                    Err(crate::Error {
                        status: polar_cas_fs_sys::PCFS_CORRUPT,
                    })
                });
            } else {
                misses.push(*row);
                slots.push(slot);
            }
        }
        // Cached templates already own their immutable lease. Only a cold
        // native lookup observes the image stamp; warm opens perform no I/O.
        let decoded = if misses.is_empty() {
            Vec::new()
        } else {
            cached.image.members(&misses).map_err(cas_error)?
        };
        for ((slot, row), member) in slots.into_iter().zip(misses).zip(decoded) {
            if let Ok(member) = &member {
                // Cache a separate, never-read template. Reads mutate only
                // their own handle's lazy LZ4 fields and independent cursor.
                if let Ok(template) = member.try_clone() {
                    let members = cached.members.get_or_insert_with(|| {
                        lru::LruCache::new(std::num::NonZeroUsize::new(16).unwrap())
                    });
                    if members.len() == members.cap().get() && members.len() < MEMBER_CACHE_CAPACITY
                    {
                        members.resize(
                            std::num::NonZeroUsize::new(
                                (members.len() * 2).min(MEMBER_CACHE_CAPACITY),
                            )
                            .unwrap(),
                        );
                    }
                    members.put(row.sha256, template);
                }
            }
            members[slot] = Some(member);
        }
        drop(cached);
        Ok(self.wrap_members(
            members
                .into_iter()
                .map(|member| member.expect("every row has a result"))
                .collect(),
        ))
    }

    /// Metadata-only freshness check for the retained managed/producer lease.
    pub fn revalidate(&self) -> Result<()> {
        self.prepared.revalidate()
    }
    /// Inspect presence, size and native layout through this already opened
    /// Pack. This does not decode/hash payload and is not a new admission or a
    /// proof that an unknown producer's member bytes match their digest.
    /// Consumers still verify bytes at EOF, or use an explicit deep audit.
    pub async fn check_member(&self, content: &polar_cas::ContentRef) -> Result<()> {
        let expected = descriptor(content)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| Error::Backend(format!("native reader requires Tokio runtime: {e}")))?;
        let image = self.image.clone();
        let owner = self.prepared.clone();
        let permit = self.budget.acquire().await?;
        runtime
            .spawn_blocking(move || {
                let (_permit, _owner) = (permit, owner);
                image
                    .lock()
                    .map_err(|_| Error::Backend("native image lock poisoned".into()))?
                    .image
                    .check_native_member(&expected.sha256, expected.size_bytes)
                    .map_err(cas_error)
            })
            .await
            .map_err(|e| Error::Backend(format!("native member inspection worker: {e}")))?
    }
}
struct PinnedMember {
    member: Member,
    // Member itself retains the immutable Image lease, including after cache eviction.
}
impl Read for PinnedMember {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.member.read(buffer).map_err(|error| {
            if error
                .get_ref()
                .and_then(|e| e.downcast_ref::<crate::Error>())
                .is_some_and(|e| e.status == polar_cas_fs_sys::PCFS_CORRUPT)
            {
                io::Error::new(io::ErrorKind::InvalidData, Error::Integrity)
            } else {
                error
            }
        })
    }
}
fn parse_locator(locator: &SourceLocator, access: Option<&OpaqueAccessRef>) -> Result<[u8; 32]> {
    locator.validate()?;
    if locator.revision.is_some() || access.is_some() {
        return Err(Error::Invalid(
            "EROFS locator has no revision/access material".into(),
        ));
    }
    let id = ObjectId::try_from(locator.key.clone())?;
    if id.algorithm() != polar_cas::DigestAlgorithm::Sha256 {
        return Err(Error::MissingDigest);
    }
    Ok(*id.digest_bytes())
}
#[async_trait]
impl SourceReader for ErofsSourceReader {
    async fn open(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
    ) -> Result<Reader> {
        let digest = parse_locator(locator, access)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| Error::Backend(format!("native reader requires Tokio runtime: {e}")))?;
        let image = self.image.clone();
        let owner = self.prepared.clone();
        let permit = self.budget.acquire().await?;
        let member = runtime
            .spawn_blocking(move || {
                let _permit = permit;
                let member = image
                    .lock()
                    .map_err(|_| Error::Backend("native image lock poisoned".into()))?
                    .image
                    .member_by_digest(&digest)
                    .map_err(cas_error)?;
                drop(owner);
                Ok::<_, Error>(PinnedMember { member })
            })
            .await
            .map_err(|e| Error::Backend(format!("native member worker: {e}")))??;
        Ok(Box::pin(BlockingReader::new(member, self.budget.clone())?))
    }
}


