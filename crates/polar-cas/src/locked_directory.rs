//! Persistent physical-directory evidence. Not business identity, ACL or a seal
//! capability. fs-verity digests and ordinary CAS digests are intentionally separate.
use crate::{
    canonical::canonical_json_bytes,
    manifest::bundle::BundleLimits,
    native_fs::filesystem_member_locator,
    projection::{ProjectionEntry, ProjectionNode, ProjectionTree},
    source::{LocationId, LocationSpec, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const LOCKED_DIRECTORY_SCHEMA: &str = "polar.cas.locked-directory.v1";
pub const LOCKED_DIRECTORY_ADAPTER: &str = "locked-directory";
pub const MAX_LOCKED_DIRECTORY_RECORD_BYTES: u64 = 64 << 20;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerityWitness {
    pub algorithm: u32,
    /// Lowercase hex of the kernel's fs-verity digest, NOT the ordinary SHA-256.
    pub digest: String,
}
impl VerityWitness {
    pub fn validate(&self) -> Result<()> {
        let length = match self.algorithm { 1 => 64, 2 => 128, _ => return Err(Error::Integrity) };
        if self.digest.len() != length || !self.digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InodeWitness {
    pub device: u64,
    pub inode: u64,
    pub link_count: u64,
    pub size_bytes: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_seconds: i64,
    pub mtime_nanos: u32,
    pub ctime_seconds: i64,
    pub ctime_nanos: u32,
    pub verity: Option<VerityWitness>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedEntry {
    pub path: String,
    pub node: ProjectionNode,
    pub inode: InodeWitness,
}

/// Serializable observation. Deserializing or validating this structure does
/// NOT grant read authority or create a verified replica. Reopen must compare
/// all entries to live kernel evidence; Catalog must anchor its admitted origin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDirectoryRecord {
    pub schema: String,
    pub tree: ContentRef,
    pub entries: Vec<LockedEntry>,
}
impl LockedDirectoryRecord {
    pub fn validate(&self, limits: BundleLimits) -> Result<ProjectionTree> {
        if self.schema != LOCKED_DIRECTORY_SCHEMA || self.entries.is_empty() {
            return Err(Error::Integrity);
        }
        if self.entries.len() > crate::catalog::MAX_REGISTER_SOURCES { return Err(Error::TooLarge); }
        let mut previous: Option<&str> = None;
        let root_device = self.entries[0].inode.device;
        let mut inodes = BTreeSet::new();
        for e in &self.entries {
            if previous.is_some_and(|p| p >= e.path.as_str()) { return Err(Error::Integrity); }
            previous = Some(&e.path);
            let w = &e.inode;
            if !inodes.insert((w.device, w.inode)) || w.device != root_device || w.inode == 0 || w.link_count == 0 ||
                w.mtime_nanos >= 1_000_000_000 || w.ctime_nanos >= 1_000_000_000 ||
                w.size_bytes > i64::MAX as u64 {
                return Err(Error::Integrity);
            }
            match &e.node {
                ProjectionNode::Directory => {
                    if w.mode & 0o170000 != 0o040000 || w.verity.is_some() { return Err(Error::Integrity); }
                }
                ProjectionNode::File { content, executable } => {
                    content.id.require_sha256()?;
                    if w.mode & 0o170000 != 0o100000 || w.link_count != 1 ||
                        w.size_bytes != content.size_bytes || (w.mode & 0o111 != 0) != *executable {
                        return Err(Error::Integrity);
                    }
                    w.verity.as_ref().ok_or(Error::Integrity)?.validate()?;
                }
                ProjectionNode::Symlink { target } => {
                    if w.mode & 0o170000 != 0o120000 || w.link_count != 1 ||
                        w.size_bytes != target.len() as u64 || w.verity.is_some() { return Err(Error::Integrity); }
                }
            }
        }
        let tree = ProjectionTree::from_entries(
            self.entries.iter().map(|e| ProjectionEntry { path: e.path.clone(), node: e.node.clone() }),
            DigestAlgorithm::Sha256, limits,
        )?;
        if tree.output().root() != &self.tree { return Err(Error::Integrity); }
        Ok(tree)
    }
    pub fn canonical_bytes(&self, limits: BundleLimits) -> Result<Vec<u8>> {
        self.validate(limits)?;
        let bytes = canonical_json_bytes(self).map_err(|_| Error::Integrity)?;
        if bytes.len() as u64 > MAX_LOCKED_DIRECTORY_RECORD_BYTES { return Err(Error::TooLarge); }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], expected: &ContentRef, limits: BundleLimits) -> Result<Self> {
        if bytes.len() as u64 > MAX_LOCKED_DIRECTORY_RECORD_BYTES { return Err(Error::TooLarge); }
        if expected.size_bytes != bytes.len() as u64 || ObjectId::for_bytes(bytes) != expected.id {
            return Err(Error::Integrity);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Integrity)?;
        if value.canonical_bytes(limits)? != bytes { return Err(Error::Integrity); }
        Ok(value)
    }
    pub fn member(&self, locator: &SourceLocator) -> Result<(usize, &ContentRef)> {
        locator.validate()?;
        if locator.revision.is_some() { return Err(Error::UnsupportedCapability); }
        for (i, entry) in self.entries.iter().enumerate() {
            if let ProjectionNode::File { content, .. } = &entry.node {
                if filesystem_member_locator(&entry.path)? == *locator { return Ok((i, content)); }
            }
        }
        Err(Error::NotFound)
    }
}

/// Public physical binding, interpreted only through the host-injected source.
/// No absolute host path, token, or directory hash masquerading as file bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDirectoryBinding {
    pub record: ContentRef,
    pub tree: ContentRef,
    pub source: LocationId,
    pub locator: SourceLocator,
}
impl LockedDirectoryBinding {
    pub fn to_location(&self) -> Result<LocationSpec> {
        self.record.id.require_sha256()?;
        self.tree.id.require_sha256()?;
        self.locator.validate()?;
        if self.record.size_bytes == 0 || self.record.size_bytes > MAX_LOCKED_DIRECTORY_RECORD_BYTES ||
            self.locator.revision.is_some() || self.locator.key.starts_with('/') ||
            self.locator.key.contains('\\') || self.locator.key.split('/').any(|s| matches!(s, "" | "." | "..")) {
            return Err(Error::Invalid("invalid locked directory binding".into()));
        }
        let key = ObjectId::for_bytes(&canonical_json_bytes(&(LOCKED_DIRECTORY_SCHEMA, self)).map_err(|_| Error::Integrity)?).hex();
        let location = LocationSpec {
            id: LocationId::new(format!("locked-dir-{key}"))?,
            adapter: LOCKED_DIRECTORY_ADAPTER.into(),
            storage_identity: format!("polar-locked-directory-v1:{key}"),
            binding: BTreeMap::from([
                ("schema".into(), LOCKED_DIRECTORY_SCHEMA.into()),
                ("record".into(), self.record.id.to_string()),
                ("record_bytes".into(), self.record.size_bytes.to_string()),
                ("tree".into(), self.tree.id.to_string()),
                ("tree_bytes".into(), self.tree.size_bytes.to_string()),
                ("source".into(), self.source.as_str().into()),
                ("key".into(), self.locator.key.clone()),
            ]),
            credential_ref: None,
        };
        location.validate()?;
        Ok(location)
    }
    pub fn from_location(spec: &LocationSpec) -> Result<Self> {
        spec.validate()?;
        if spec.adapter != LOCKED_DIRECTORY_ADAPTER || spec.credential_ref.is_some() || spec.binding.len() != 7 {
            return Err(Error::UnsupportedCapability);
        }
        let get = |s: &str| spec.binding.get(s).ok_or(Error::Integrity);
        let count = |s: &str| -> Result<u64> {
            let text = get(s)?;
            let n = text.parse::<u64>().map_err(|_| Error::Integrity)?;
            if &n.to_string() != text { return Err(Error::Integrity); }
            Ok(n)
        };
        if get("schema")? != LOCKED_DIRECTORY_SCHEMA { return Err(Error::Integrity); }
        let result = Self {
            record: ContentRef { id: ObjectId::try_from(get("record")?.clone())?, size_bytes: count("record_bytes")? },
            tree: ContentRef { id: ObjectId::try_from(get("tree")?.clone())?, size_bytes: count("tree_bytes")? },
            source: LocationId::new(get("source")?.clone())?,
            locator: SourceLocator { key: get("key")?.clone(), revision: None },
        };
        if result.to_location()? != *spec { return Err(Error::Integrity); }
        Ok(result)
    }
}

/// Trusted-adapter evidence for one atomic Catalog admission. No Deserialize.
/// This follows VerifiedPackedMembers: the constructor belongs at a boundary
/// which actually observed locks AND hashed the same bytes, never on wire input.
#[derive(Clone, Debug)]
pub struct VerifiedLockedDirectory {
    binding: LockedDirectoryBinding,
    record: LockedDirectoryRecord,
}
impl VerifiedLockedDirectory {
    pub fn from_trusted_observation(binding: LockedDirectoryBinding, record: LockedDirectoryRecord, limits: BundleLimits) -> Result<Self> {
        let bytes = record.canonical_bytes(limits)?;
        binding.to_location()?;
        if binding.tree != record.tree || binding.record.id != ObjectId::for_bytes(&bytes) ||
            binding.record.size_bytes != bytes.len() as u64 { return Err(Error::Integrity); }
        Ok(Self { binding, record })
    }
    pub fn binding(&self) -> &LockedDirectoryBinding { &self.binding }
    pub fn record(&self) -> &LockedDirectoryRecord { &self.record }
}


/// Explicit in-place admission; no pack/profile or destination payload copies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDirectoryAdmissionRequest {
    pub request_key: crate::source::RequestKey,
    pub source: crate::source::SourceSpec,
    pub metadata_destination: LocationId,
    pub limits: BundleLimits,
    pub max_candidates: u32,
    pub max_source_bytes: u64,
    pub max_record_bytes: u64,
}
impl LockedDirectoryAdmissionRequest {
    pub fn validate(&self) -> Result<()> {
        self.source.locator.validate()?;
        if self.source.access_ref.is_some() || self.source.locator.revision.is_some() ||
            !(1..=1024).contains(&self.max_candidates) || self.max_source_bytes == 0 ||
            self.max_source_bytes > i64::MAX as u64 || self.max_record_bytes == 0 ||
            self.max_record_bytes > MAX_LOCKED_DIRECTORY_RECORD_BYTES {
            return Err(Error::Invalid("invalid in-place directory admission request".into()));
        }
        Ok(())
    }
}
