//! Iroh publication contracts without an endpoint, protocol or store dependency.
use crate::{
    canonical::canonical_json_bytes,
    manifest::bundle::{
        AccessMode, BundleDraft, BundleLimits, Completeness, DirectoryRecord, ManifestBundle,
    },
    operation::{CopyInput, CopyRequest, FailureCode, OperationState, ReplicaRef},
    ports::ReplicaCommit,
    source::{LocationId, OperationId, RequestKey},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const IROH_EXPORT_SCHEMA: &str = "polar.cas.iroh-export-plan.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrohExportRequest {
    pub request_key: RequestKey,
    pub root: ContentRef,
    pub destination: LocationId,
    pub max_candidates: u32,
    pub max_object_bytes: u64,
    pub limits: BundleLimits,
}
impl IrohExportRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "export-iroh-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        self.object_request(&self.root)?.digest()?;
        if self.limits.max_bundle_bytes == 0
            || self.limits.max_bundle_bytes as u128 > i64::MAX as u128
            || self.root.size_bytes > self.limits.tree.max_manifest_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(IROH_EXPORT_SCHEMA, self))
                .map_err(|_| Error::Invalid("Iroh export request encoding".into()))?,
        ))
    }
    pub fn object_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        let key = canonical_json_bytes(&(IROH_EXPORT_SCHEMA, self.operation_id(), content))
            .map_err(|_| Error::Invalid("Iroh child request encoding".into()))?;
        Ok(CopyRequest {
            writer: None,
            request_key: RequestKey::new(format!(
                "iroh-object-{}",
                ObjectId::for_bytes(&key).hex()
            ))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.max_candidates,
            },
            destination: self.destination.clone(),
            primary_algorithm: DigestAlgorithm::Blake3,
            max_object_bytes: self.max_object_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrohObject {
    /// Original tree identity, which need not be BLAKE3.
    pub content: ContentRef,
    pub replica: ReplicaRef,
}

/// Validated full graph and exact destination references, not a byte proof.
/// The Catalog must check aliases; the adapter must verify the physical bytes.
/// No caller-supplied collection names or deserialization bypass validation.
#[derive(Clone)]
pub struct IrohExportInput {
    request: IrohExportRequest,
    tree: ManifestBundle,
    objects: Vec<IrohObject>,
    _owner: Arc<dyn Send + Sync>,
}
impl IrohExportInput {
    pub fn new(
        request: IrohExportRequest,
        directories: Vec<DirectoryRecord>,
        objects: Vec<IrohObject>,
    ) -> Result<Self> {
        request.digest()?;
        if objects.len()
            > request
                .limits
                .max_directories
                .saturating_add(request.limits.tree.max_entries)
        {
            return Err(Error::TooLarge);
        }
        let tree = ManifestBundle::new(
            BundleDraft {
                root: request.root.clone(),
                directories,
                storages: vec![],
                sources: vec![],
                access: AccessMode::Durable,
                completeness: Completeness::ReferencesOnly,
            },
            request.limits,
        )?;
        let mut required = BTreeMap::new();
        for content in tree
            .directories()
            .iter()
            .map(|d| &d.content)
            .chain(tree.closure().files())
        {
            request.object_request(content)?.digest()?;
            if required
                .insert(content.id.content_key(), content.size_bytes)
                .is_some_and(|size| size != content.size_bytes)
            {
                return Err(Error::Integrity);
            }
        }
        if required.len() != objects.len() {
            return Err(Error::Conflict);
        }
        for object in &objects {
            if required.remove(&object.content.id.content_key()) != Some(object.content.size_bytes)
                || object.replica.location != request.destination
                || object.replica.content.id.algorithm() != DigestAlgorithm::Blake3
                || object.replica.content.size_bytes != object.content.size_bytes
                || object.replica.locator.revision.is_some()
                || object.replica.locator.key != object.replica.content.id.content_key()
            {
                return Err(Error::Conflict);
            }
        }
        let mut input = Self {
            request,
            tree,
            objects,
            _owner: Arc::new(()),
        };
        let root = &input.request.root;
        input
            .objects
            .sort_by_key(|object| collection_name(root, &object.content));
        Ok(input)
    }
    pub fn request(&self) -> &IrohExportRequest {
        &self.request
    }
    pub fn tree(&self) -> &ManifestBundle {
        &self.tree
    }
    pub fn objects(&self) -> &[IrohObject] {
        &self.objects
    }
    pub fn name(&self, object: &IrohObject) -> String {
        collection_name(&self.request.root, &object.content)
    }
    pub fn retain(mut self, owner: impl Send + Sync + 'static) -> Self {
        self._owner = Arc::new((self._owner, owner));
        self
    }
}
fn collection_name(root: &ContentRef, content: &ContentRef) -> String {
    format!(
        "{}/{}",
        if content == root { "root" } else { "objects" },
        content.id.content_key()
    )
}

/// Compact, non-secret frozen output description. No full directory/URL list.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrohPrepared {
    pub metadata: ContentRef,
    pub collection: ContentRef,
    pub ticket: String,
}
impl IrohPrepared {
    pub fn validate(&self, request: &IrohExportRequest) -> Result<()> {
        request.digest()?;
        if self.metadata.id.algorithm() != DigestAlgorithm::Blake3
            || self.collection.id.algorithm() != DigestAlgorithm::Blake3
        {
            return Err(Error::MissingDigest);
        }
        if self.metadata.size_bytes == 0
            || self.collection.size_bytes == 0
            || self
                .metadata
                .size_bytes
                .checked_add(self.collection.size_bytes)
                .ok_or(Error::TooLarge)?
                > request.limits.max_bundle_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        // Wire-level ticket decoding belongs to the pinned Iroh adapter.
        if self.ticket.is_empty()
            || self.ticket.len() > 8192
            || !self.ticket.bytes().all(|c| c.is_ascii_alphanumeric())
        {
            return Err(Error::Invalid("invalid public Iroh ticket".into()));
        }
        Ok(())
    }
}

/// Only actual adapter writes or complete reads produce these witnesses.
pub struct IrohCommit {
    pub prepared: IrohPrepared,
    pub metadata: ReplicaCommit,
    pub collection: ReplicaCommit,
}
#[async_trait]
pub trait IrohPublisher: Send + Sync {
    fn prepare(&self, input: &IrohExportInput) -> Result<IrohPrepared>;
    async fn publish(&self, input: IrohExportInput, prepared: &IrohPrepared) -> Result<IrohCommit>;
    async fn inspect(
        &self,
        input: IrohExportInput,
        prepared: &IrohPrepared,
    ) -> Result<Option<IrohCommit>>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrohExportPlan {
    pub schema: String,
    pub request: IrohExportRequest,
    pub prepared: Option<IrohPrepared>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrohExportReceipt {
    pub operation_id: OperationId,
    pub input: ContentRef,
    pub metadata: ReplicaRef,
    pub collection: ReplicaRef,
    pub ticket: String,
    pub object_references: u64,
    pub reused: bool,
    pub completed_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrohExportOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: IrohExportPlan,
    pub receipt: Option<IrohExportReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginIrohExport {
    Started(Box<IrohExportOperation>),
    Replay(Box<IrohExportReceipt>),
}
