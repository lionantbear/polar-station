//! Durable format composition over the ordinary copy, pack and tree publisher.
use super::{
    limits::{OciLayerLimits, OciRenderLimits},
    OciArtifactRef, OciLimits, OciPackRequest,
};
use crate::{
    canonical::canonical_json_bytes,
    operation::{CopyInput, CopyRequest, FailureCode, OperationState},
    pack::PackOptions,
    projection::{
        export::{ProjectionReceipt, ProjectionRequest},
        ProjectionLayout, ProjectionProfile, ProjectionTree,
    },
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
pub use oci_spec::image::Platform;
use serde::{Deserialize, Serialize};

pub const OCI_PROJECTION_SCHEMA: &str = "polar.cas.oci-projection-plan.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciProjectionRequest {
    pub request_key: RequestKey,
    pub root: OciArtifactRef,
    pub selected_manifest: ContentRef,
    pub platform: Platform,
    pub local: LocationId,
    pub destination: LocationId,
    pub target: SourceLocator,
    pub layout: ProjectionLayout,
    pub profile: ProjectionProfile,
    pub algorithm: DigestAlgorithm,
    pub max_candidates: u32,
    pub limits: OciLimits,
    pub layers: OciLayerLimits,
    pub render: OciRenderLimits,
}
impl OciProjectionRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "project-oci-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest ID")
    }
    pub fn normalized(&self) -> Result<Self> {
        self.root.validate()?;
        self.limits.validate()?;
        self.layers.validate()?;
        self.render.validate()?;
        if self.selected_manifest.id.algorithm() != DigestAlgorithm::Sha256 {
            return Err(Error::UnsupportedCapability);
        }
        if self.root.content.size_bytes > self.limits.max_document_bytes
            || self.selected_manifest.size_bytes > self.limits.max_document_bytes
        {
            return Err(Error::TooLarge);
        }
        if self
            .platform
            .features()
            .as_ref()
            .is_some_and(|f| !f.is_empty())
        {
            return Err(Error::UnsupportedCapability);
        }
        let mut request = self.clone();
        // OS features are a set, as in the existing image validator. Keep the
        // upstream type; this is CAS request JSON, not OCI document rewriting.
        let mut features = request.platform.os_features().clone().unwrap_or_default();
        features.sort();
        features.dedup();
        request.platform.set_os_features(if features.is_empty() {
            None
        } else {
            Some(features)
        });
        let dummy = ContentRef {
            id: ObjectId::for_bytes_with(self.algorithm, b""),
            size_bytes: 0,
        };
        request.layout = request.directory_request(&dummy)?.normalized()?.layout;
        if canonical_json_bytes(&request)
            .map_err(|_| Error::Integrity)?
            .len()
            > 900 * 1024
        {
            return Err(Error::TooLarge);
        }
        Ok(request)
    }
    pub fn digest(&self) -> Result<ObjectId> {
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(OCI_PROJECTION_SCHEMA, self.normalized()?))
                .map_err(|_| Error::Integrity)?,
        ))
    }
    fn key(&self, role: &str, content: Option<&ContentRef>) -> Result<RequestKey> {
        let bytes =
            canonical_json_bytes(&(OCI_PROJECTION_SCHEMA, self.operation_id(), role, content))
                .map_err(|_| Error::Integrity)?;
        RequestKey::new(format!(
            "oci-tree-{role}-{}",
            ObjectId::for_bytes(&bytes).hex()
        ))
    }
    pub fn metadata_request(&self) -> Result<OciPackRequest> {
        Ok(OciPackRequest {
            writer: None,
            request_key: self.key("metadata", None)?,
            root: self.root.clone(),
            destination: self.local.clone(),
            primary_algorithm: self.algorithm,
            max_candidates: self.max_candidates,
            limits: self.limits,
        })
    }
    pub fn blob_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        if content.id.algorithm() != DigestAlgorithm::Sha256 {
            return Err(Error::UnsupportedCapability);
        }
        self.copy_request(
            "input",
            content,
            self.layers
                .max_blob_bytes
                .max(self.limits.max_document_bytes),
        )
    }
    pub fn file_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        self.copy_request("file", content, self.render.archives.max_file_bytes.max(1))
    }
    fn copy_request(
        &self,
        role: &str,
        content: &ContentRef,
        max_object_bytes: u64,
    ) -> Result<CopyRequest> {
        if content.size_bytes > max_object_bytes {
            return Err(Error::TooLarge);
        }
        Ok(CopyRequest {
            writer: None,
            request_key: self.key(role, Some(content))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.max_candidates,
            },
            destination: self.local.clone(),
            primary_algorithm: self.algorithm,
            max_object_bytes,
        })
    }
    pub fn node_options(&self, content: &ContentRef) -> Result<PackOptions> {
        Ok(PackOptions {
            request_key: self.key("node", Some(content))?,
            destination: self.local.clone(),
            algorithm: self.algorithm,
            max_candidates: self.max_candidates,
            limits: self.render.tree.tree,
        })
    }
    pub fn directory_request(&self, derived: &ContentRef) -> Result<ProjectionRequest> {
        Ok(ProjectionRequest {
            request_key: self.key("directory", None)?,
            root: derived.clone(),
            local: self.local.clone(),
            destination: self.destination.clone(),
            target: self.target.clone(),
            layout: self.layout.clone(),
            profile: self.profile.clone(),
            algorithm: self.algorithm,
            max_candidates: self.max_candidates,
            limits: self.render.tree,
        })
    }
    /// The renderer supplies a checked, complete Preserve tree. Planning the
    /// caller's shape reuses the ordinary projection validator and encoding.
    pub fn plan_tree(&self, rendered: &ProjectionTree) -> Result<ProjectionTree> {
        if rendered.layout() != &ProjectionLayout::Preserve
            || rendered.input() != rendered.output().root()
            || rendered.algorithm() != self.algorithm
            || rendered.limits() != self.render.tree
        {
            return Err(Error::Conflict);
        }
        ProjectionTree::plan(
            rendered.output().root(),
            rendered.output().directories(),
            &self.layout,
            self.algorithm,
            self.render.tree,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciProjectionPlan {
    pub schema: String,
    pub request: OciProjectionRequest,
    pub rendered: Option<ContentRef>,
    pub output: Option<ContentRef>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciProjectionReceipt {
    pub operation_id: OperationId,
    pub artifact: OciArtifactRef,
    pub selected_manifest: ContentRef,
    pub rendered: ContentRef,
    pub projection: ProjectionReceipt,
    pub completed_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciProjectionOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: OciProjectionPlan,
    pub receipt: Option<OciProjectionReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginOciProjection {
    Started(Box<OciProjectionOperation>),
    Replay(Box<OciProjectionReceipt>),
}
