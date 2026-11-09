//! Portable directory closure and explicit, potentially sensitive read routes.
//! This module never opens a location, resolves credentials, or creates replicas.
use super::{TreeLimits, TreeManifest};
use crate::{
    source::{PublicBinding, SourceLocator},
    ContentRef, Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
mod validation;
mod wire;

pub const BUNDLE_SCHEMA: &str = "polar.cas.export.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleLimits {
    pub tree: TreeLimits,
    pub max_bundle_bytes: usize,
    pub max_directories: usize,
    pub max_storages: usize,
    pub max_sources: usize,
    pub max_routes_per_object: usize,
}
impl Default for BundleLimits {
    fn default() -> Self {
        Self {
            tree: TreeLimits::default(),
            max_bundle_bytes: 64 * 1024 * 1024,
            max_directories: 100_000,
            max_storages: 256,
            max_sources: 100_000,
            max_routes_per_object: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    Temporary,
    Durable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Consumable,
    ReferencesOnly,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRecord {
    pub content: ContentRef,
    pub canonical_json: String,
}
impl DirectoryRecord {
    pub fn from_tree(tree: &TreeManifest, algorithm: crate::DigestAlgorithm) -> Result<Self> {
        let bytes = tree.canonical_bytes()?;
        Ok(Self {
            content: ContentRef {
                id: crate::ObjectId::for_bytes_with(algorithm, &bytes),
                size_bytes: bytes.len() as u64,
            },
            canonical_json: String::from_utf8(bytes).map_err(|_| Error::Integrity)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageDescriptor {
    pub name: String,
    pub adapter: String,
    pub config_version: u32,
    #[serde(deserialize_with = "wire::unique_map")]
    pub binding: PublicBinding,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TemporaryRequest {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    expires_at_ms: u64,
}
impl fmt::Debug for TemporaryRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemporaryRequest")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}
impl TemporaryRequest {
    pub fn new(
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        expires_at_ms: u64,
    ) -> Result<Self> {
        let request = Self {
            method,
            url,
            headers,
            expires_at_ms,
        };
        request.validate()?;
        Ok(request)
    }
    pub fn method(&self) -> &str {
        &self.method
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
    fn validate(&self) -> Result<()> {
        validation::temporary_request(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadRoute {
    Durable {
        storage: String,
        locator: SourceLocator,
    },
    Temporary {
        storage: String,
        locator: SourceLocator,
        request: TemporaryRequest,
    },
}
impl ReadRoute {
    pub fn storage(&self) -> &str {
        match self {
            Self::Durable { storage, .. } | Self::Temporary { storage, .. } => storage,
        }
    }
    pub fn locator(&self) -> &SourceLocator {
        match self {
            Self::Durable { locator, .. } | Self::Temporary { locator, .. } => locator,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentSources {
    pub content: ContentRef,
    pub routes: Vec<ReadRoute>,
}

#[derive(Clone, Debug)]
pub struct BundleDraft {
    pub root: ContentRef,
    pub directories: Vec<DirectoryRecord>,
    pub storages: Vec<StorageDescriptor>,
    pub sources: Vec<ContentSources>,
    pub access: AccessMode,
    pub completeness: Completeness,
}

/// Derived facts from the complete embedded directory graph. File references
/// remain claims; this type makes no assertion about bytes at a read route.
#[derive(Clone, Debug)]
pub struct TreeClosure {
    files: Vec<ContentRef>,
    missing_routes: Vec<ContentRef>,
    logical_entry_count: usize,
    logical_file_bytes: u64,
    earliest_expiry_ms: Option<u64>,
}
impl TreeClosure {
    pub fn files(&self) -> &[ContentRef] {
        &self.files
    }
    pub fn missing_routes(&self) -> &[ContentRef] {
        &self.missing_routes
    }
    pub fn logical_entry_count(&self) -> usize {
        self.logical_entry_count
    }
    pub fn logical_file_bytes(&self) -> u64 {
        self.logical_file_bytes
    }
    pub fn earliest_expiry_ms(&self) -> Option<u64> {
        self.earliest_expiry_ms
    }
}

/// Explicit sensitive output boundary. Never persist this in a Catalog or
/// include it in an operation plan, receipt, diagnostics, or public metadata.
pub struct SensitiveBundleBytes(Vec<u8>);
impl SensitiveBundleBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl AsRef<[u8]> for SensitiveBundleBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}
impl fmt::Debug for SensitiveBundleBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SensitiveBundleBytes([REDACTED])")
    }
}

#[derive(Clone)]
pub struct ManifestBundle {
    draft: BundleDraft,
    closure: TreeClosure,
}
impl fmt::Debug for ManifestBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManifestBundle")
            .field("root", &self.draft.root)
            .field("access", &self.draft.access)
            .finish_non_exhaustive()
    }
}
impl ManifestBundle {
    pub(crate) fn validate_limits(&self, limits: BundleLimits) -> Result<()> {
        validation::validate(&self.draft, limits)?;
        if wire::encode(&self.draft)?.len() > limits.max_bundle_bytes {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    pub fn new(draft: BundleDraft, limits: BundleLimits) -> Result<Self> {
        let closure = validation::validate(&draft, limits)?;
        let bundle = Self { draft, closure };
        if wire::encode(&bundle.draft)?.len() > limits.max_bundle_bytes {
            return Err(Error::TooLarge);
        }
        Ok(bundle)
    }
    pub fn from_bytes(bytes: &[u8], limits: BundleLimits) -> Result<Self> {
        if bytes.len() > limits.max_bundle_bytes {
            return Err(Error::TooLarge);
        }
        Self::new(wire::decode(bytes)?, limits)
    }
    pub fn root(&self) -> &ContentRef {
        &self.draft.root
    }
    pub fn encode_public(&self) -> Result<Vec<u8>> {
        if self
            .draft
            .sources
            .iter()
            .flat_map(|s| &s.routes)
            .any(|r| matches!(r, ReadRoute::Temporary { .. }))
        {
            return Err(Error::Invalid(
                "temporary capabilities require sensitive output".into(),
            ));
        }
        wire::encode(&self.draft)
    }
    pub fn encode_sensitive(&self) -> Result<SensitiveBundleBytes> {
        wire::encode(&self.draft).map(SensitiveBundleBytes)
    }
    pub fn directories(&self) -> &[DirectoryRecord] {
        &self.draft.directories
    }
    pub fn storages(&self) -> &[StorageDescriptor] {
        &self.draft.storages
    }
    pub fn sources(&self) -> &[ContentSources] {
        &self.draft.sources
    }
    pub fn access(&self) -> AccessMode {
        self.draft.access
    }
    pub fn completeness(&self) -> Completeness {
        self.draft.completeness
    }
    pub fn closure(&self) -> &TreeClosure {
        &self.closure
    }
}
