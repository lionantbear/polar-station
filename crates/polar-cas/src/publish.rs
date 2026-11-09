//! Explicit publication of a tree description, not storage ACLs or serving.
use crate::{
    canonical::canonical_json_bytes,
    manifest::bundle::{AccessMode, BundleLimits, Completeness, ManifestBundle, StorageDescriptor},
    operation::{FailureCode, OperationState, ReplicaRef},
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MANIFEST_EXPORT_SCHEMA: &str = "polar.cas.manifest-export-plan.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestRoutePolicy {
    Durable,
    Temporary { expires_in_seconds: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExportStorage {
    pub location: LocationId,
    pub descriptor: StorageDescriptor,
    pub routes: Vec<ManifestRoutePolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExportRequest {
    pub request_key: RequestKey,
    pub root: ContentRef,
    pub destination: LocationId,
    pub target: SourceLocator,
    pub storages: Vec<ManifestExportStorage>,
    pub access: AccessMode,
    pub completeness: Completeness,
    pub algorithm: DigestAlgorithm,
    pub max_candidates: u32,
    pub limits: BundleLimits,
}
impl ManifestExportRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "export-manifest-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        self.target.validate()?;
        if self.target.revision.is_some()
            || self.target.key.contains('\\')
            || self
                .target
                .key
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(Error::Invalid(
                "manifest output requires an unambiguous relative key".into(),
            ));
        }
        if !(1..=1024).contains(&self.max_candidates) {
            return Err(Error::Invalid(
                "export candidate limit must be 1..=1024".into(),
            ));
        }
        if self.limits.max_bundle_bytes == 0
            || self.limits.max_bundle_bytes as u128 > i64::MAX as u128
            || self.root.size_bytes > self.limits.tree.max_manifest_bytes as u64
            || self.storages.len() > self.limits.max_storages
            || self.storages.len() > crate::catalog::MAX_REGISTER_LOCATIONS
        {
            return Err(Error::TooLarge);
        }
        let mut names = BTreeSet::new();
        let mut locations = BTreeSet::new();
        for storage in &self.storages {
            storage.descriptor.validate()?;
            if !names.insert(&storage.descriptor.name) || !locations.insert(&storage.location) {
                return Err(Error::Conflict);
            }
            if storage.routes.is_empty() || storage.routes.len() > 2 {
                return Err(Error::Invalid(
                    "select one or both manifest route kinds".into(),
                ));
            }
            let mut kinds = BTreeSet::new();
            for route in &storage.routes {
                let temporary = matches!(route, ManifestRoutePolicy::Temporary { .. });
                if !kinds.insert(temporary) {
                    return Err(Error::Conflict);
                }
                if let ManifestRoutePolicy::Temporary { expires_in_seconds } = route {
                    if *expires_in_seconds == 0 || *expires_in_seconds > i64::MAX as u64 / 1000 {
                        return Err(Error::Invalid("invalid temporary route lifetime".into()));
                    }
                }
            }
        }
        Ok(ObjectId::for_bytes(&canonical_json_bytes(self).map_err(
            |_| Error::Invalid("manifest export request encoding".into()),
        )?))
    }

    /// Validate the transient body and derive only non-secret frozen facts.
    /// The Catalog records these before allowing any output side effect.
    pub fn prepare_bundle(&self, bundle: &ManifestBundle) -> Result<PreparedManifestExport> {
        self.digest()?;
        bundle.validate_limits(self.limits)?;
        if bundle.root() != &self.root
            || bundle.access() != self.access
            || bundle.completeness() != self.completeness
            || bundle.storages()
                != self
                    .storages
                    .iter()
                    .map(|s| s.descriptor.clone())
                    .collect::<Vec<_>>()
        {
            return Err(Error::Conflict);
        }
        for sources in bundle.sources() {
            for route in &sources.routes {
                let selected = self
                    .storages
                    .iter()
                    .find(|s| s.descriptor.name == route.storage())
                    .ok_or(Error::Conflict)?;
                let allowed = selected.routes.iter().any(|policy| {
                    matches!(
                        (policy, route),
                        (
                            ManifestRoutePolicy::Durable,
                            crate::manifest::bundle::ReadRoute::Durable { .. }
                        ) | (
                            ManifestRoutePolicy::Temporary { .. },
                            crate::manifest::bundle::ReadRoute::Temporary { .. }
                        )
                    )
                });
                if !allowed {
                    return Err(Error::Conflict);
                }
            }
        }
        let bytes = bundle.encode_sensitive()?;
        Ok(PreparedManifestExport {
            content: ContentRef {
                id: ObjectId::for_bytes_with(self.algorithm, bytes.as_bytes()),
                size_bytes: bytes.as_bytes().len() as u64,
            },
            links_expire_at_ms: bundle.closure().earliest_expiry_ms(),
            missing_routes: bundle.closure().missing_routes().to_vec(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedManifestExport {
    pub content: ContentRef,
    pub links_expire_at_ms: Option<u64>,
    pub missing_routes: Vec<ContentRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExportPlan {
    pub schema: String,
    pub request: ManifestExportRequest,
    pub prepared: Option<PreparedManifestExport>,
    pub stage_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExportReceipt {
    pub operation_id: OperationId,
    pub input: ContentRef,
    pub output: ReplicaRef,
    pub access: AccessMode,
    pub completeness: Completeness,
    pub missing_routes: Vec<ContentRef>,
    pub links_expire_at_ms: Option<u64>,
    pub reused: bool,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExportOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: ManifestExportPlan,
    pub receipt: Option<ManifestExportReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginManifestExport {
    Started(Box<ManifestExportOperation>),
    Replay(Box<ManifestExportReceipt>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "format",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PublishRequest {
    OciTree(Box<crate::oci::projection::OciProjectionRequest>),
    Oci(crate::oci::OciExportRequest),
    Iroh(crate::iroh::IrohExportRequest),
    Manifest(ManifestExportRequest),
    LocalTree(crate::projection::export::ProjectionRequest),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "format",
    content = "receipt",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PublishReceipt {
    OciTree(Box<crate::oci::projection::OciProjectionReceipt>),
    Oci(crate::oci::OciExportReceipt),
    Iroh(crate::iroh::IrohExportReceipt),
    Manifest(ManifestExportReceipt),
    LocalTree(crate::projection::export::ProjectionReceipt),
}
