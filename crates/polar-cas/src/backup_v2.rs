//! Independent CAS backup planning. No Node identity, business DB, or I/O.
//! A plan is not a completed backup; source states are historical observations.
pub mod shared;
use crate::{
    canonical::canonical_json_bytes,
    catalog::ReplicaState,
    source::{LocationId, LocationSpec, SourceLocator, SourceState},
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
pub use shared::{SharedCatalogBackup, SharedProjectionBackup};
use std::{collections::BTreeSet, path::PathBuf};

pub const CATALOG_SNAPSHOT_SCHEMA: &str = "polar.cas.catalog-snapshot.v2";
pub const POLICY_SCHEMA: &str = "polar.cas.backup-policy.v2";
pub const PLAN_SCHEMA: &str = "polar.cas.backup-plan.v2";
pub const MAX_PAGE_SIZE: u32 = 1024;
pub const MAX_LOCATIONS: u32 = 1024;

/// Caller-authorized output; this local path never becomes a portable identity.
#[derive(Clone, Debug)]
pub struct CatalogSnapshotRequest {
    pub destination: PathBuf,
    pub expected_catalog_id: String,
}

/// Metadata must come from a trusted enclosing receipt, not the file being read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub schema: String,
    pub catalog_id: String,
    pub database: ContentRef,
    pub schema_digest: ObjectId,
}
impl CatalogSnapshot {
    pub fn validate(&self) -> Result<()> {
        if self.schema != CATALOG_SNAPSHOT_SCHEMA || self.database.size_bytes == 0 {
            return Err(Error::Invalid(
                "unsupported or empty CAS catalog snapshot".into(),
            ));
        }
        LocationId::new(self.catalog_id.clone())?;
        self.database.id.require_sha256()?;
        self.schema_digest.require_sha256()
    }
}

/// Snapshot-local pagination key. Never use the row key in a portable restore.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupCursor {
    pub snapshot_id: ObjectId,
    pub after_key: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupObject {
    pub content: ContentRef,
    /// All proven aliases, including content.id; claims must not enter this list.
    pub aliases: Vec<ObjectId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupInventoryPage {
    pub objects: Vec<BackupObject>,
    pub next: Option<BackupCursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupLocation {
    pub spec: LocationSpec,
    pub enabled: bool,
    pub unbound_sources: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "state",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BackupPlacementState {
    Replica(ReplicaState),
    Source(SourceState),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPlacement {
    pub location: LocationId,
    pub locator: SourceLocator,
    pub enabled: bool,
    pub observation: BackupPlacementState,
}
impl BackupPlacement {
    /// A candidate, NOT a fresh read/hash/retention guarantee.
    pub fn is_read_candidate(&self) -> bool {
        self.enabled
            && matches!(
                &self.observation,
                BackupPlacementState::Replica(ReplicaState::Ready)
                    | BackupPlacementState::Source(SourceState::Verified)
            )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupInventorySummary {
    pub objects: u64,
    pub logical_bytes: u64,
    pub locations: u64,
    pub sources: u64,
    pub replicas: u64,
    pub objects_without_digests: u64,
    pub unresolved_manifest_children: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupAction {
    EnsureCopy { destination: LocationId },
    KeepExternal { reason: String },
    Exclude { reason: String },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupRule {
    pub location: LocationId,
    pub action: BackupAction,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupPolicy {
    pub schema: String,
    pub policy_id: String,
    pub rules: Vec<CasBackupRule>,
}
impl CasBackupPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.schema != POLICY_SCHEMA || self.rules.len() > MAX_LOCATIONS as usize {
            return Err(Error::Invalid("unsupported CAS backup policy".into()));
        }
        LocationId::new(self.policy_id.clone())?;
        let mut seen = BTreeSet::new();
        for rule in &self.rules {
            if !seen.insert(&rule.location) {
                return Err(Error::Invalid("duplicate CAS backup location rule".into()));
            }
            match &rule.action {
                BackupAction::KeepExternal { reason } | BackupAction::Exclude { reason }
                    if reason.trim().is_empty() || reason.len() > 1024 || reason.contains('\0') =>
                {
                    return Err(Error::Invalid(
                        "explicit bounded backup policy reason required".into(),
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<ObjectId> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.rules.sort_by(|a, b| a.location.cmp(&b.location));
        let bytes = canonical_json_bytes(&canonical)
            .map_err(|_| Error::Invalid("backup policy encoding failed".into()))?;
        Ok(ObjectId::for_bytes(&bytes))
    }

    pub fn action(&self, location: &LocationId) -> Result<&BackupAction> {
        self.rules
            .iter()
            .find(|r| &r.location == location)
            .map(|r| &r.action)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "backup coverage unknown: unclassified location {}",
                    location.as_str()
                ))
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedBackupObject {
    pub object: BackupObject,
    /// Original locations whose bytes must be materialized on restore.
    pub protected_locations: Vec<LocationId>,
    pub copy_destinations: Vec<LocationId>,
    pub external_dependencies: Vec<BackupPlacement>,
    pub excluded_locations: Vec<LocationId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPlanLimits {
    pub page_size: u32,
    pub max_objects: u32,
    pub max_placements_per_object: u32,
    pub max_plan_bytes: usize,
}
impl Default for BackupPlanLimits {
    fn default() -> Self {
        Self {
            page_size: 256,
            max_objects: 100_000,
            max_placements_per_object: 1024,
            max_plan_bytes: 64 * 1024 * 1024,
        }
    }
}
impl BackupPlanLimits {
    pub fn validate(&self) -> Result<()> {
        validate_page_size(self.page_size)?;
        validate_page_size(self.max_placements_per_object)?;
        if self.max_objects == 0
            || self.max_objects > 1_000_000
            || self.max_plan_bytes == 0
            || self.max_plan_bytes > 256 * 1024 * 1024
        {
            return Err(Error::Invalid(
                "bounded CAS backup plan limits required".into(),
            ));
        }
        Ok(())
    }
}

/// P2 planning only: neither payload I/O nor managed-store coverage is proven.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_catalog: Option<SharedCatalogBackup>,
    pub schema: String,
    pub catalog: CatalogSnapshot,
    pub policy_digest: ObjectId,
    pub inventory: BackupInventorySummary,
    pub objects: Vec<PlannedBackupObject>,
    pub excluded_unbound_sources: u64,
    pub payload_verified: bool,
    pub managed_coverage_verified: bool,
}

pub fn validate_page_size(limit: u32) -> Result<()> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(Error::Invalid("backup page size must be 1..=1024".into()));
    }
    Ok(())
}

pub const BACKUP_SCHEMA: &str = "polar.cas.backup.v2";

/// Public, portable publication identity; location IDs and credentials are not
/// sufficient recovery locators and must not replace this description.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPublication {
    pub content: ContentRef,
    pub storage: crate::manifest::bundle::StorageDescriptor,
    pub locator: SourceLocator,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupCreate {
    pub request_key: crate::source::RequestKey,
    pub policy: CasBackupPolicy,
    pub metadata_destination: LocationId,
    pub publication_destination: LocationId,
    pub publication_target: SourceLocator,
    pub completion_target: SourceLocator,
    pub publication_storage: crate::manifest::bundle::StorageDescriptor,
    pub storages: Vec<crate::publish::ManifestExportStorage>,
    pub limits: BackupPlanLimits,
    pub bundle_limits: crate::manifest::bundle::BundleLimits,
    pub max_object_bytes: u64,
    pub max_candidates: u32,
}
impl CasBackupCreate {
    pub fn validate(&self) -> Result<()> {
        self.policy.validate()?;
        self.limits.validate()?;
        self.publication_storage.validate()?;
        self.publication_target.validate()?;
        self.completion_target.validate()?;
        if self.completion_target == self.publication_target
            || self.completion_target.revision.is_some()
            || self.completion_target.key.contains('\\')
            || self
                .completion_target
                .key
                .split('/')
                .any(|p| matches!(p, "" | "." | ".."))
        {
            return Err(Error::Invalid(
                "distinct immutable completion key required".into(),
            ));
        }
        validate_page_size(self.max_candidates)?;
        if self.max_object_bytes == 0
            || self.max_object_bytes > i64::MAX as u64
            || self.publication_target.revision.is_some()
            || self.publication_target.key.starts_with('/')
            || self.publication_target.key.contains('\\')
            || self
                .publication_target
                .key
                .split('/')
                .any(|v| matches!(v, "" | "." | ".."))
        {
            return Err(Error::Invalid("bounded backup target required".into()));
        }
        let mut names = BTreeSet::new();
        let mut locations = BTreeSet::new();
        for storage in &self.storages {
            storage.descriptor.validate()?;
            if !names.insert(&storage.descriptor.name)
                || !locations.insert(&storage.location)
                || storage.routes != [crate::publish::ManifestRoutePolicy::Durable]
            {
                return Err(Error::Invalid(
                    "unique durable backup routes required".into(),
                ));
            }
        }
        if !locations.contains(&self.metadata_destination)
            || self.storages.is_empty()
            || self.storages.len() > self.bundle_limits.max_storages
            || self.bundle_limits.max_bundle_bytes == 0
            || self.bundle_limits.max_bundle_bytes > 256 * 1024 * 1024
            || self.bundle_limits.max_sources > 100_000
        {
            return Err(Error::TooLarge);
        }
        for rule in &self.policy.rules {
            if let BackupAction::EnsureCopy { destination } = &rule.action {
                if !locations.contains(destination) {
                    return Err(Error::AccessMaterialUnavailable);
                }
                // A copy to its source storage is not an independent backup.
                if destination == &rule.location {
                    return Err(Error::Conflict);
                }
            }
        }
        Ok(())
    }
    pub fn digest(&self, plan: &CasBackupPlan) -> Result<ObjectId> {
        self.validate()?;
        if plan.policy_digest != self.policy.digest()? {
            return Err(Error::Conflict);
        }
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(self, plan)).map_err(|_| Error::Integrity)?,
        ))
    }
}

/// Receipt is emitted only after payload and publication readback. Excluded
/// content and KeepExternal dependencies remain visible in the frozen plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupDescriptor {
    pub schema: String,
    pub request_digest: ObjectId,
    pub request: CasBackupCreate,
    pub plan: CasBackupPlan,
    pub root: ContentRef,
    pub publication: BackupPublication,
    pub protected_objects: u64,
    pub verified_bytes: u64,
}
impl CasBackupDescriptor {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        canonical_json_bytes(self).map_err(|_| Error::Integrity)
    }
    pub fn from_bytes(bytes: &[u8], expected: &ObjectId, max_bytes: usize) -> Result<Self> {
        expected.require_sha256()?;
        if bytes.len() > max_bytes {
            return Err(Error::TooLarge);
        }
        if ObjectId::for_bytes(bytes) != *expected {
            return Err(Error::Integrity);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Integrity)?;
        if value.canonical_bytes()? != bytes {
            return Err(Error::Integrity);
        }
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        self.plan.catalog.validate()?;
        if self.schema != BACKUP_SCHEMA
            || self.plan.schema != PLAN_SCHEMA
            || self.plan.payload_verified
            || self.plan.managed_coverage_verified
            || self.request.digest(&self.plan)? != self.request_digest
            || self.publication.storage != self.request.publication_storage
            || self.publication.locator != self.request.publication_target
            || self.publication.content.size_bytes
                > self.request.bundle_limits.max_bundle_bytes as u64
            || self.plan.objects.len() as u64 != self.plan.inventory.objects
        {
            return Err(Error::Integrity);
        }
        if let Some(shared) = &self.plan.shared_catalog {
            shared.validate(self.request.limits.max_objects as usize, u64::MAX)?;
            let destination = match self.request.policy.action(&shared.source_location)? {
                BackupAction::EnsureCopy { destination } => destination,
                _ => return Err(Error::Integrity),
            };
            let selected: std::collections::BTreeMap<_, _> = self
                .plan
                .objects
                .iter()
                .map(|object| (object.object.content.id.to_string(), object))
                .collect();
            for file in &shared.files {
                let object = selected.get(&file.id.to_string()).ok_or(Error::Integrity)?;
                if object.object.content != *file
                    || !object.protected_locations.contains(&shared.source_location)
                    || !object.copy_destinations.contains(destination)
                {
                    return Err(Error::Integrity);
                }
            }
        }
        let mut seen = BTreeSet::new();
        let mut count = 0_u64;
        let mut bytes = self.plan.catalog.database.size_bytes;
        for object in &self.plan.objects {
            if !seen.insert(object.object.content.id.to_string()) {
                return Err(Error::Integrity);
            }
            if !object.copy_destinations.is_empty() || !object.external_dependencies.is_empty() {
                count += 1;
                bytes = bytes
                    .checked_add(object.object.content.size_bytes)
                    .ok_or(Error::TooLarge)?;
            }
        }
        if count != self.protected_objects || bytes != self.verified_bytes {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRestoreMapping {
    pub source: LocationId,
    /// Explicit target capability, never a source absolute path from the DB.
    pub target: LocationSpec,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadataRestore {
    pub operation_key: crate::source::RequestKey,
    pub target_catalog_id: String,
    pub mappings: Vec<BackupRestoreMapping>,
    pub max_catalog_bytes: u64,
    pub max_rows: u64,
}
impl CatalogMetadataRestore {
    pub fn digest(&self, snapshot: &CatalogSnapshot) -> Result<ObjectId> {
        snapshot.validate()?;
        LocationId::new(self.target_catalog_id.clone())?;
        if self.max_rows == 0
            || self.max_rows > 10_000_000
            || self.max_catalog_bytes == 0
            || snapshot.database.size_bytes > self.max_catalog_bytes
            || self.mappings.len() > MAX_LOCATIONS as usize
        {
            return Err(Error::TooLarge);
        }
        let mut seen = BTreeSet::new();
        for mapping in &self.mappings {
            mapping.target.validate()?;
            if !seen.insert(&mapping.source) {
                return Err(Error::Conflict);
            }
        }
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(snapshot, self)).map_err(|_| Error::Integrity)?,
        ))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadataRestored {
    pub request_digest: ObjectId,
    pub snapshot_digest: ObjectId,
    pub objects: u64,
    pub replayed: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CasBackupRestored {
    pub backup_digest: ObjectId,
    pub catalog: CatalogMetadataRestored,
    pub copied_replicas: u64,
}
