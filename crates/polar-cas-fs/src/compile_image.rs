//! Unprivileged image ownership. Compile/read leases never authorize mounting.
use crate::{host::PreparedImage, projection::descriptor, PackConstruction};
use polar_cas::{
    native_fs::VerifiedPackedMembers,
    ports::{ManagedReplica, VerifiedStaging},
    ContentRef, Error, ObjectId, Result,
};
use rustix::fs::{fcntl_add_seals, memfd_create, open, MemfdFlags, Mode, OFlags, SealFlags};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    sync::Arc,
};

/// Conservative per-image and per-compilation memory-backed snapshot limit.
/// Callers preparing multiple leases must also bound their cumulative
/// snapshot_bytes() while acquiring them, rather than only at compilation.
/// Larger untrusted imported images need a configured disk-backed immutable
/// reader; they are never silently copied into an unbounded memfd. Managed CAS
/// and fresh producer leases make no snapshot.
pub const MAX_COMPILE_SNAPSHOT_BYTES: u64 = 256 << 20;

/// Nonserializable ownership of exact readonly CAS bytes for compile/read.
/// A lease retains a managed CAS replica, the original closed job-private
/// producer inode, or an explicitly imported digest-verified sealed snapshot.
/// It is not a mount capability.
pub struct CasImageLease {
    file: File,
    content: ContentRef,
    _owner: Arc<dyn Send + Sync>,
    trusted_members: bool,
    admitted_members: Arc<BTreeMap<[u8; 32], u64>>,
    managed: Option<ManagedReplica>,
    generated_stat: Option<std::fs::Metadata>,
    snapshot_bytes: u64,
}

impl CasImageLease {
    /// Retain the actual constructor's output, without a copy or another hash.
    ///
    /// `staging` must obey VerifiedStaging's exclusive-owner contract: its
    /// original writer and all aliases are closed after seal_generated_file.
    /// This method checks the nofollow opened inode against PackConstruction's
    /// private encoder identity, not merely a supplied hash/length or 0444 mode.
    /// The trusted producer contract does not protect against a hostile owner
    /// reopening the inode to write, or a malicious administrator.
    pub fn from_generated_pack(
        staging: VerifiedStaging,
        construction: &PackConstruction,
    ) -> Result<Self> {
        let pack = construction.pack();
        let content = ContentRef {
            id: ObjectId::from_sha256(pack.sha256),
            size_bytes: pack.size_bytes,
        };
        check_staging(&staging, &content)?;
        let file = open_staging(&staging)?;
        let metadata = file.metadata()?;
        if !construction.matches_generated_inode(&metadata) || metadata.mode() & 0o7777 != 0o444 {
            return Err(Error::Integrity);
        }
        Ok(Self {
            file,
            content,
            _owner: Arc::new(staging),
            trusted_members: true,
            admitted_members: Arc::default(),
            managed: None,
            generated_stat: Some(metadata),
            snapshot_bytes: 0,
        })
    }

    /// Retain an already admitted managed-CAS file without copying, hashing or
    /// imposing a memory-snapshot size cap. The adapter owns the immutability
    /// contract; this is not a receipt or path-based assertion of trust.
    pub fn from_managed_replica(replica: ManagedReplica) -> Result<Self> {
        descriptor(replica.content())?;
        replica.revalidate()?;
        Ok(Self {
            file: replica.file().try_clone()?,
            content: replica.content().clone(),
            _owner: Arc::new(replica.clone()),
            trusted_members: false,
            admitted_members: Arc::default(),
            managed: Some(replica),
            generated_stat: None,
            snapshot_bytes: 0,
        })
    }

    /// Attach exact previously verified/admitted member facts for this image.
    /// Blob identity alone does not establish semantic member digest claims.
    /// Only listed (digest, size) pairs gain trust; other members retain audit.
    pub fn with_admitted_members(mut self, witness: &VerifiedPackedMembers) -> Result<Self> {
        if witness.binding().image != self.content {
            return Err(Error::Integrity);
        }
        let members = Arc::make_mut(&mut self.admitted_members);
        for content in witness.members() {
            let id = content
                .ids()
                .iter()
                .find(|id| id.algorithm() == polar_cas::DigestAlgorithm::Sha256)
                .ok_or(Error::MissingDigest)?;
            if members
                .get(id.digest_bytes())
                .is_some_and(|size| *size != content.size_bytes())
            {
                return Err(Error::Integrity);
            }
            members.insert(*id.digest_bytes(), content.size_bytes());
        }
        Ok(self)
    }

    /// Conservative explicit untrusted import path. Copies at most
    /// MAX_COMPILE_SNAPSHOT_BYTES into a sealed immutable memfd, then verifies
    /// the snapshot's complete SHA-256. A staging digest is not producer
    /// evidence, so compilation still audits native members on this path.
    pub fn from_verified_staging(staging: VerifiedStaging, expected: ContentRef) -> Result<Self> {
        check_staging(&staging, &expected)?;
        if expected.size_bytes > MAX_COMPILE_SNAPSHOT_BYTES {
            return Err(Error::TooLarge);
        }
        let source = open_staging(&staging)?;
        let before = source.metadata()?;
        if !before.is_file() || before.len() != expected.size_bytes {
            return Err(Error::Integrity);
        }
        let fd = memfd_create(
            "polar-cas-compile",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .map_err(std::io::Error::from)?;
        let mut snapshot = File::from(fd);
        let mut bounded = (&source).take(expected.size_bytes + 1);
        if std::io::copy(&mut bounded, &mut snapshot)? != expected.size_bytes {
            return Err(Error::Integrity);
        }
        let after = source.metadata()?;
        if before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(Error::Integrity);
        }
        fcntl_add_seals(
            &snapshot,
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
        )
        .map_err(std::io::Error::from)?;
        snapshot.seek(SeekFrom::Start(0))?;
        if ObjectId::for_reader(&snapshot)? != expected.id {
            return Err(Error::Integrity);
        }
        // The native adapter intentionally requires an O_RDONLY descriptor.
        let file = File::open(format!("/proc/self/fd/{}", snapshot.as_raw_fd()))?;
        Ok(Self {
            file,
            snapshot_bytes: expected.size_bytes,
            content: expected,
            _owner: Arc::new(staging),
            trusted_members: false,
            admitted_members: Arc::default(),
            managed: None,
            generated_stat: None,
        })
    }

    /// Compatibility conversion. The original immutable inode and Host pin
    /// remain alive; fs-verity alone does not attest the Pack's producer.
    pub fn from_prepared(prepared: PreparedImage) -> Result<Self> {
        Ok(Self {
            file: prepared.file().try_clone()?,
            content: prepared.content().clone(),
            _owner: Arc::new(prepared),
            trusted_members: false,
            admitted_members: Arc::default(),
            managed: None,
            generated_stat: None,
            snapshot_bytes: 0,
        })
    }
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            content: self.content.clone(),
            _owner: self._owner.clone(),
            trusted_members: self.trusted_members,
            admitted_members: self.admitted_members.clone(),
            managed: self.managed.clone(),
            generated_stat: self.generated_stat.clone(),
            snapshot_bytes: self.snapshot_bytes,
        })
    }
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn content(&self) -> &ContentRef {
        &self.content
    }
    /// Memory-backed snapshot retained by this lease. Zero for same-inode
    /// managed/generated/PreparedImage inputs; a clone reports the same size.
    /// Sum unique planned images during preparation to enforce a host budget.
    pub fn snapshot_bytes(&self) -> u64 {
        self.snapshot_bytes
    }
    pub(crate) fn has_trusted_member(&self, content: &ContentRef) -> bool {
        content.id.algorithm() == polar_cas::DigestAlgorithm::Sha256
            && (self.trusted_members
                || self.admitted_members.get(content.id.digest_bytes())
                    == Some(&content.size_bytes))
    }
    /// Cheap retained-owner freshness check. No payload is read or hashed.
    /// This detects ordinary drift, not hostile same-owner/admin tampering.
    pub fn revalidate(&self) -> Result<()> {
        if let Some(replica) = &self.managed {
            replica.revalidate()?;
        }
        if let Some(before) = &self.generated_stat {
            let now = self.file.metadata()?;
            if !now.is_file()
                || before.dev() != now.dev()
                || before.ino() != now.ino()
                || before.len() != now.len()
                || before.mode() != now.mode()
                || before.mtime() != now.mtime()
                || before.mtime_nsec() != now.mtime_nsec()
            {
                return Err(Error::Integrity);
            }
        }
        Ok(())
    }
}

fn check_staging(staging: &VerifiedStaging, expected: &ContentRef) -> Result<()> {
    descriptor(expected)?;
    if staging
        .content()
        .reference(&expected.id)
        .map_err(|_| Error::Integrity)?
        != *expected
    {
        return Err(Error::Integrity);
    }
    Ok(())
}
fn open_staging(staging: &VerifiedStaging) -> Result<File> {
    // NONBLOCK ensures a replaced FIFO cannot block before the regular-file
    // check; NOFOLLOW rejects a path replaced with a symlink.
    let fd = open(
        staging.path(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    Ok(File::from(fd))
}


