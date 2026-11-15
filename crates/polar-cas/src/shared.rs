//! Local trusted-storage capabilities and complete Pack admission. None of the
//! capability types are wire DTOs; deserializing a claim never grants trust.
use crate::{ContentRef, DigestAlgorithm, Error, ObjectHasher, ObjectId, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct TrustedSharedRoot(PathBuf);
impl TrustedSharedRoot {
    /// Only a local storage adapter promising immutable, no-replace publication
    /// by every participant in this explicitly shared trust domain may call this.
    pub fn from_trusted_adapter(path: PathBuf) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::Invalid("absolute shared root required".into()));
        }
        Ok(Self(path))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub const MAX_PACK_MEMBERS: usize = 1_000_000;

/// Full set of actual members, established by local construction or a complete
/// member audit, never merely by verifying the digest of the opaque image.
#[derive(Clone, Debug)]
pub struct CompletePackIndex {
    pack: ContentRef,
    members: Vec<ContentRef>,
    members_digest: ObjectId,
}
impl CompletePackIndex {
    /// The trusted adapter must establish ALL actual members of the exact
    /// immutable image. Partial Catalog batches do not satisfy this contract.
    pub fn from_trusted_adapter(pack: ContentRef, mut members: Vec<ContentRef>) -> Result<Self> {
        crate::native_fs::ErofsPackBinding::native(pack.clone())?;
        if members.len() > MAX_PACK_MEMBERS {
            return Err(Error::TooLarge);
        }
        members.sort_unstable_by(|a, b| a.id.digest_bytes().cmp(b.id.digest_bytes()));
        let mut previous = None;
        for member in &members {
            member.id.require_sha256()?;
            if member.size_bytes > i64::MAX as u64
                || member.id == pack.id
                || previous == Some(&member.id)
            {
                return Err(Error::Integrity);
            }
            previous = Some(&member.id);
        }
        let mut hash = ObjectHasher::new(DigestAlgorithm::Sha256);
        hash.update(b"polar.cas.members.v1\0");
        hash.update(&(members.len() as u64).to_be_bytes());
        for member in &members {
            hash.update(member.id.digest_bytes());
            hash.update(&member.size_bytes.to_be_bytes());
        }
        Ok(Self {
            pack,
            members,
            members_digest: hash.finish(),
        })
    }
    pub fn pack(&self) -> &ContentRef {
        &self.pack
    }
    pub fn members(&self) -> &[ContentRef] {
        &self.members
    }
    pub fn members_digest(&self) -> &ObjectId {
        &self.members_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackIndexRef {
    pub pack: ContentRef,
    pub index: ObjectId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRun {
    pub index: ObjectId,
    pub rows: u64,
    pub level: u32,
}

/// Physical representation, never a business Catalog row or authorization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PhysicalCandidate {
    Loose,
    PackMember { pack: ContentRef },
}

/// One indexed location. This is metadata, not proof that a payload is readable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocatedContent {
    pub content: ContentRef,
    pub location: PhysicalCandidate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LooseIndexRun {
    pub algorithm: DigestAlgorithm,
    pub run: IndexRun,
}

/// One atomic generation owns both the mapping segments and coverage set.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedIndexGeneration {
    pub version: u32,
    pub runs: Vec<IndexRun>,
    pub covered: Vec<PackIndexRef>,
    pub loose_runs: Vec<LooseIndexRun>,
    pub covered_loose: Vec<ObjectId>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct IndexMaintenanceReceipt {
    /// Elapsed nonblocking lock acquisition and actual exclusive work, in ns.
    pub lock_acquire_ns: u64,
    pub lock_hold_ns: u64,
    pub added_packs: u64,
    pub added_loose_batches: u64,
    pub covered_packs: u64,
    pub segments: u64,
    pub rows_written: u64,
    pub bytes_written: u64,
}
