//! Immutable M/native-device dependencies plus replaceable packed redirect
//! coverage. This bounded exchange request is not a membership or mount proof.
use super::HybridProjection;
use crate::{
    canonical::canonical_json_bytes,
    manifest::bundle::{DirectoryRecord, ManifestBundle},
    native_fs::{
        transfer::{image_transfer_parts, NativeCopyObject, NativeTransferOptions},
        ErofsPackBinding,
    },
    operation::{CopyInput, CopyReceipt, CopyRequest},
    projection::{ProjectionLayout, ProjectionNode, ProjectionTree},
    source::RequestKey,
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const HYBRID_TRANSFER_SCHEMA: &str = "polar.cas.hybrid-transfer.v1";
pub const MAX_REDIRECT_PACKS: usize = 128;
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
pub type HybridTransferOptions = NativeTransferOptions;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridTransferRequest {
    pub schema: String,
    pub projection: HybridProjection,
    pub directories: Vec<DirectoryRecord>,
    /// Claims only. The decoder must reconstruct the EXACT same requirement
    /// set from M/T; no receiver trusts a sender-supplied has_redirects boolean.
    pub redirects: Vec<ContentRef>,
    /// Already-admitted self-contained pack identities. No loose payload copy
    /// children. Build/admit a pack explicitly before transfer when necessary.
    pub redirect_packs: Vec<ContentRef>,
    pub options: HybridTransferOptions,
}
impl HybridTransferRequest {
    pub fn from_bytes(bytes: &[u8], max_bytes: usize) -> Result<Self> {
        if max_bytes == 0 || max_bytes > MAX_REQUEST_BYTES || bytes.len() > max_bytes {
            return Err(Error::TooLarge);
        }
        serde_json::from_slice(bytes)
            .map_err(|e| Error::Invalid(format!("hybrid transfer encoding: {e}")))
    }
}

#[derive(Clone, Debug)]
pub struct HybridTransferPlan {
    request: HybridTransferRequest,
    tree: ManifestBundle,
    objects: Vec<NativeCopyObject>,
    bundle_bytes: Vec<u8>,
    physical_bytes: u64,
    digest: ObjectId,
}
impl HybridTransferPlan {
    pub fn new(mut request: HybridTransferRequest) -> Result<Self> {
        if request.schema != HYBRID_TRANSFER_SCHEMA {
            return Err(Error::Invalid("hybrid transfer schema".into()));
        }
        request.projection.validate()?;
        if request.redirect_packs.len() > MAX_REDIRECT_PACKS
            || request.redirects.len() > super::MAX_HYBRID_FILES
        {
            return Err(Error::TooLarge);
        }
        if request.redirects.is_empty() != request.redirect_packs.is_empty()
            || request.redirects.is_empty() == request.projection.has_redirects()
        {
            return Err(Error::Invalid(
                "redirect coverage descriptors missing or unused".into(),
            ));
        }
        // Provider-set order is canonicalized; native device ABI order is NOT.
        request.redirect_packs.sort_by_key(|p| p.id.to_string());
        request.redirects.sort_by_key(|c| c.id.to_string());
        let mut providers = BTreeSet::new();
        for p in &request.redirect_packs {
            ErofsPackBinding::native(p.clone())?;
            if p.id == request.projection.meta.id || !providers.insert(p.id.to_string()) {
                return Err(Error::Conflict);
            }
        }
        let mut required = BTreeMap::new();
        for c in &request.redirects {
            c.id.require_sha256()?;
            if c.size_bytes > i64::MAX as u64 {
                return Err(Error::TooLarge);
            }
            if required.insert(c.id.to_string(), c).is_some() {
                return Err(Error::Conflict);
            }
        }
        let parts = image_transfer_parts(
            &request.projection.tree,
            &mut request.directories,
            &request.options,
            request
                .projection
                .native_packs
                .iter()
                .chain(&request.redirect_packs)
                .chain(std::iter::once(&request.projection.meta)),
        )?;
        let known: BTreeMap<_, _> = parts
            .tree
            .closure()
            .files()
            .iter()
            .map(|c| (c.id.to_string(), c))
            .collect();
        for (key, c) in &required {
            if known.get(key) != Some(c) {
                return Err(Error::Integrity);
            }
        }
        let limits = bundle_limits(&request.options);
        let tree = ProjectionTree::plan(
            parts.tree.root(),
            parts.tree.directories(),
            &ProjectionLayout::Preserve,
            parts.tree.root().id.algorithm(),
            limits,
        )?;
        let (mut native, mut redirected) = (0u64, 0u64);
        for entry in tree.entries() {
            if let ProjectionNode::File { content, .. } = &entry.node {
                if required.contains_key(&content.id.to_string()) {
                    redirected += 1;
                } else {
                    native += 1;
                }
            }
        }
        if native != request.projection.native_file_count
            || redirected != request.projection.redirect_file_count
        {
            return Err(Error::Integrity);
        }
        let bytes = canonical_json_bytes(&request).map_err(|e| Error::Invalid(e.to_string()))?;
        if bytes.len() > request.options.max_metadata_bytes {
            return Err(Error::TooLarge);
        }
        let plan = Self {
            request,
            tree: parts.tree,
            objects: parts.objects,
            bundle_bytes: parts.bundle_bytes,
            physical_bytes: parts.physical_bytes,
            digest: ObjectId::for_bytes(&bytes),
        };
        for i in 0..plan.objects.len() {
            plan.object_request(i)?.digest()?;
        }
        Ok(plan)
    }
    pub fn request(&self) -> &HybridTransferRequest {
        &self.request
    }
    pub fn projection(&self) -> &HybridProjection {
        &self.request.projection
    }
    pub fn tree(&self) -> &ManifestBundle {
        &self.tree
    }
    pub fn redirects(&self) -> &[ContentRef] {
        &self.request.redirects
    }
    pub fn redirect_packs(&self) -> &[ContentRef] {
        &self.request.redirect_packs
    }
    pub fn objects(&self) -> &[NativeCopyObject] {
        &self.objects
    }
    pub fn digest(&self) -> &ObjectId {
        &self.digest
    }
    pub fn physical_bytes(&self) -> u64 {
        self.physical_bytes
    }
    pub fn inline_bytes(&self, index: usize) -> Result<Option<&[u8]>> {
        let o = self
            .objects
            .get(index)
            .ok_or_else(|| Error::Invalid("hybrid object index".into()))?;
        Ok(o.is_manifest_bundle()
            .then_some(self.bundle_bytes.as_slice()))
    }
    pub fn manifest_bundle(&self) -> &ContentRef {
        self.objects[0].content()
    }
    /// Unique physical P set in copy order. This does NOT replace the ordered
    /// native list encoded by M or the canonicalized redirect provider set.
    pub fn packs(&self) -> impl Iterator<Item = &ContentRef> {
        self.objects
            .iter()
            .skip(1)
            .take(self.objects.len().saturating_sub(2))
            .map(|o| o.content())
    }
    pub fn object_request(&self, index: usize) -> Result<CopyRequest> {
        let o = self
            .objects
            .get(index)
            .ok_or_else(|| Error::Invalid("hybrid object index".into()))?;
        self.child_request("polar.cas.hybrid-transfer-child.v1", o.content())
    }
    pub fn directory_request(&self, index: usize) -> Result<CopyRequest> {
        let d = self
            .request
            .directories
            .get(index)
            .ok_or_else(|| Error::Invalid("hybrid directory index".into()))?;
        self.child_request("polar.cas.hybrid-local-directory.v1", &d.content)
    }
    fn child_request(&self, kind: &str, content: &ContentRef) -> Result<CopyRequest> {
        let bytes = canonical_json_bytes(&(kind, &self.digest, content))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(CopyRequest {
            writer: None,
            request_key: RequestKey::new(format!("hybrid-{}", ObjectId::for_bytes(&bytes).hex()))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.request.options.max_candidates,
            },
            destination: self.request.options.destination.clone(),
            primary_algorithm: content.id.algorithm(),
            max_object_bytes: self.request.options.max_object_bytes,
        })
    }
}
pub fn bundle_limits(options: &HybridTransferOptions) -> crate::manifest::bundle::BundleLimits {
    crate::manifest::bundle::BundleLimits {
        tree: options.tree_limits,
        max_bundle_bytes: options.max_metadata_bytes,
        max_directories: options.tree_limits.max_entries.saturating_add(1),
        max_storages: 0,
        max_sources: 0,
        max_routes_per_object: 0,
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridTransferReceipt {
    pub request_digest: ObjectId,
    pub objects: Vec<CopyReceipt>,
}
impl HybridTransferReceipt {
    pub fn validate_against(&self, plan: &HybridTransferPlan) -> Result<()> {
        if self.request_digest != *plan.digest() || self.objects.len() != plan.objects.len() {
            return Err(Error::Integrity);
        }
        for (i, r) in self.objects.iter().enumerate() {
            if r.operation_id != plan.object_request(i)?.operation_id()
                || r.replica.content != *plan.objects[i].content()
                || r.replica.location != plan.request.options.destination
            {
                return Err(Error::Integrity);
            }
        }
        Ok(())
    }
}
