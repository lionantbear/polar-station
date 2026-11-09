//! Physical transfer closure for a native projection. This is a request/plan,
//! not another CAS object, a live mount, or a verified projection witness.
use super::NativeProjection;
use crate::{
    canonical::canonical_json_bytes,
    manifest::{
        bundle::{
            AccessMode, BundleDraft, BundleLimits, Completeness, DirectoryRecord, ManifestBundle,
        },
        TreeLimits,
    },
    operation::{CopyInput, CopyReceipt, CopyRequest},
    source::{LocationId, RequestKey},
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const NATIVE_TRANSFER_SCHEMA: &str = "polar.cas.native-transfer.v1";
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTransferOptions {
    pub request_key: RequestKey,
    pub destination: LocationId,
    pub max_candidates: u32,
    pub max_object_bytes: u64,
    /// Sum of UNIQUE physical objects, not logical uncompressed file lengths.
    pub max_transfer_bytes: u64,
    pub max_metadata_bytes: usize,
    pub tree_limits: TreeLimits,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTransferRequest {
    pub schema: String,
    pub projection: NativeProjection,
    /// Existing canonical OUTPUT TreeManifest closure. No read routes, signed
    /// URLs, credentials or member payloads belong in this request.
    pub directories: Vec<DirectoryRecord>,
    pub options: NativeTransferOptions,
}
impl NativeTransferRequest {
    pub fn from_bytes(bytes: &[u8], max_encoded_bytes: usize) -> Result<Self> {
        if max_encoded_bytes == 0
            || max_encoded_bytes > MAX_METADATA_BYTES
            || bytes.len() > max_encoded_bytes
        {
            return Err(Error::TooLarge);
        }
        serde_json::from_slice(bytes)
            .map_err(|e| Error::Invalid(format!("native transfer encoding: {e}")))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCopyObject {
    content: ContentRef,
    manifest_bundle: bool,
}
impl NativeCopyObject {
    pub fn content(&self) -> &ContentRef {
        &self.content
    }
    pub fn is_manifest_bundle(&self) -> bool {
        self.manifest_bundle
    }
}

#[derive(Clone, Debug)]
pub struct NativeTransferPlan {
    request: NativeTransferRequest,
    tree: ManifestBundle,
    objects: Vec<NativeCopyObject>,
    digest: ObjectId,
    physical_bytes: u64,
    bundle_bytes: Vec<u8>,
}
impl NativeTransferPlan {
    pub fn new(mut request: NativeTransferRequest) -> Result<Self> {
        if request.schema != NATIVE_TRANSFER_SCHEMA {
            return Err(Error::Invalid("native transfer schema".into()));
        }
        request.projection.validate()?;
        let parts = image_transfer_parts(
            &request.projection.tree,
            &mut request.directories,
            &request.options,
            request
                .projection
                .packs
                .iter()
                .chain(std::iter::once(&request.projection.meta)),
        )?;
        let TransferParts {
            tree,
            objects,
            physical_bytes,
            bundle_bytes,
        } = parts;
        let options = &request.options;
        let encoded = canonical_json_bytes(&request).map_err(|e| Error::Invalid(e.to_string()))?;
        if encoded.len() > options.max_metadata_bytes {
            return Err(Error::TooLarge);
        }
        let digest = ObjectId::for_bytes(&encoded);
        let plan = Self {
            request,
            tree,
            objects,
            digest,
            physical_bytes,
            bundle_bytes,
        };
        for index in 0..plan.objects.len() {
            plan.object_request(index)?.digest()?;
        }
        Ok(plan)
    }
    pub fn request(&self) -> &NativeTransferRequest {
        &self.request
    }
    pub fn projection(&self) -> &NativeProjection {
        &self.request.projection
    }
    pub fn tree(&self) -> &ManifestBundle {
        &self.tree
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
        let object = self
            .objects
            .get(index)
            .ok_or_else(|| Error::Invalid("native transfer object index".into()))?;
        Ok(object
            .manifest_bundle
            .then_some(self.bundle_bytes.as_slice()))
    }
    pub fn manifest_bundle(&self) -> &ContentRef {
        &self.objects[0].content
    }
    /// Explicit receiving-Host compatibility step; not part of the network
    /// copy closure. It can materialize canonical manifest JSON locally only.
    pub fn directory_request(&self, index: usize) -> Result<CopyRequest> {
        let directory = self
            .request
            .directories
            .get(index)
            .ok_or_else(|| Error::Invalid("native directory index".into()))?;
        let key = canonical_json_bytes(&(
            "polar.cas.native-local-directory.v1",
            &self.digest,
            &directory.content,
        ))
        .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(CopyRequest {
            writer: None,
            request_key: RequestKey::new(format!(
                "native-directory-{}",
                ObjectId::for_bytes(&key).hex()
            ))?,
            source: CopyInput::Content {
                content: directory.content.clone(),
                max_candidates: self.request.options.max_candidates,
            },
            destination: self.request.options.destination.clone(),
            primary_algorithm: directory.content.id.algorithm(),
            max_object_bytes: self.request.options.max_object_bytes,
        })
    }
    pub fn object_request(&self, index: usize) -> Result<CopyRequest> {
        let object = self
            .objects
            .get(index)
            .ok_or_else(|| Error::Invalid("native transfer object index".into()))?;
        let key = canonical_json_bytes(&(
            "polar.cas.native-transfer-child.v1",
            &self.digest,
            &object.content,
        ))
        .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(CopyRequest {
            writer: None,
            request_key: RequestKey::new(format!(
                "native-object-{}",
                ObjectId::for_bytes(&key).hex()
            ))?,
            source: CopyInput::Content {
                content: object.content.clone(),
                max_candidates: self.request.options.max_candidates,
            },
            destination: self.request.options.destination.clone(),
            primary_algorithm: object.content.id.algorithm(),
            max_object_bytes: self.request.options.max_object_bytes,
        })
    }
}

/// Describes per-object committed copies. Not proof of current availability,
/// native Tree correctness, or a completed mount. No aggregate transaction is
/// claimed: retry the identical plan to resume existing per-object operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTransferReceipt {
    pub request_digest: ObjectId,
    pub objects: Vec<CopyReceipt>,
}
impl NativeTransferReceipt {
    pub fn validate_against(&self, plan: &NativeTransferPlan) -> Result<()> {
        if self.request_digest != *plan.digest() || self.objects.len() != plan.objects().len() {
            return Err(Error::Integrity);
        }
        for (index, receipt) in self.objects.iter().enumerate() {
            let request = plan.object_request(index)?;
            if receipt.operation_id != request.operation_id()
                || receipt.replica.content != *plan.objects()[index].content()
                || receipt.replica.location != plan.request.options.destination
            {
                return Err(Error::Integrity);
            }
        }
        Ok(())
    }
}

/// Shared physical-copy planning for native and hybrid views. No codec, disk IO
/// or new persisted object model. Public v1 request/digest semantics stay intact.
pub(crate) struct TransferParts {
    pub tree: ManifestBundle,
    pub objects: Vec<NativeCopyObject>,
    pub physical_bytes: u64,
    pub bundle_bytes: Vec<u8>,
}
pub(crate) fn image_transfer_parts<'a>(
    root: &ContentRef,
    directories: &mut [DirectoryRecord],
    options: &NativeTransferOptions,
    images: impl IntoIterator<Item = &'a ContentRef>,
) -> Result<TransferParts> {
    if !(1..=1024).contains(&options.max_candidates)
        || options.max_object_bytes == 0
        || options.max_object_bytes > i64::MAX as u64
        || options.max_transfer_bytes == 0
        || options.max_transfer_bytes > i64::MAX as u64
        || options.max_metadata_bytes == 0
        || options.max_metadata_bytes > MAX_METADATA_BYTES
        || options.tree_limits.max_entries > 1_000_000
    {
        return Err(Error::Invalid("native transfer budget".into()));
    }
    if directories.len() > options.tree_limits.max_entries.saturating_add(1) {
        return Err(Error::TooLarge);
    }
    let mut metadata_bytes = 0usize;
    for directory in directories.iter() {
        metadata_bytes = metadata_bytes
            .checked_add(directory.canonical_json.len())
            .ok_or(Error::TooLarge)?;
        if metadata_bytes > options.max_metadata_bytes {
            return Err(Error::TooLarge);
        }
    }
    // Directory input order is not semantic; PACK ORDER IS SEMANTIC.
    directories.sort_by_key(|a| a.content.id.to_string());
    let tree = ManifestBundle::new(
        BundleDraft {
            root: root.clone(),
            directories: directories.to_vec(),
            storages: vec![],
            sources: vec![],
            access: AccessMode::Durable,
            completeness: Completeness::ReferencesOnly,
        },
        BundleLimits {
            tree: options.tree_limits,
            max_bundle_bytes: options.max_metadata_bytes,
            max_directories: options.tree_limits.max_entries.saturating_add(1),
            max_storages: 0,
            max_sources: 0,
            max_routes_per_object: 0,
        },
    )?;
    // Reuse the existing manifest-bundle codec: thousands of directory
    // manifests must NOT become thousands of network objects/requests.
    let bundle_bytes = tree.encode_public()?;
    let bundle = ContentRef {
        id: ObjectId::for_bytes(&bundle_bytes),
        size_bytes: bundle_bytes.len() as u64,
    };
    let mut objects = Vec::new();
    let mut seen = BTreeMap::<String, ContentRef>::new();
    let mut physical_bytes = 0u64;
    {
        let mut add = |content: &ContentRef, manifest_bundle| -> Result<()> {
            if let Some(prior) = seen.get(&content.id.to_string()) {
                if prior != content {
                    return Err(Error::Integrity);
                }
                return Ok(());
            }
            if content.size_bytes > options.max_object_bytes {
                return Err(Error::TooLarge);
            }
            physical_bytes = physical_bytes
                .checked_add(content.size_bytes)
                .ok_or(Error::TooLarge)?;
            if physical_bytes > options.max_transfer_bytes {
                return Err(Error::TooLarge);
            }
            seen.insert(content.id.to_string(), content.clone());
            objects.push(NativeCopyObject {
                content: content.clone(),
                manifest_bundle,
            });
            Ok(())
        };
        add(&bundle, true)?;
        // Data first, Meta last. Regular A is intentionally NEVER expanded.
        for content in images {
            // The same bytes cannot be both a JSON manifest bundle and an
            // EROFS image. Fail rather than collapse transport roles and
            // invalidate the bundle-first / metadata-last contract.
            if content.id == bundle.id {
                return Err(Error::Conflict);
            }
            add(content, false)?;
        }
    }
    Ok(TransferParts {
        tree,
        objects,
        physical_bytes,
        bundle_bytes,
    })
}
