//! Portable, immutable directory nodes. Payload availability is independent.
use crate::{canonical::canonical_json_bytes, ContentRef, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const TREE_SCHEMA: &str = "polar.cas.tree.v1";
pub mod bundle;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TreeEntry {
    File {
        name: String,
        content: ContentRef,
        executable: bool,
    },
    Directory {
        name: String,
        manifest: ContentRef,
    },
    Symlink {
        name: String,
        target: String,
    },
}
impl TreeEntry {
    pub fn name(&self) -> &str {
        match self {
            Self::File { name, .. } | Self::Directory { name, .. } | Self::Symlink { name, .. } => {
                name
            }
        }
    }
    pub fn content(&self) -> Option<&ContentRef> {
        match self {
            Self::File { content, .. } => Some(content),
            Self::Directory { manifest, .. } => Some(manifest),
            Self::Symlink { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeLimits {
    pub max_manifest_bytes: usize,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_total_bytes: u64,
}
impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 16 * 1024 * 1024,
            max_entries: 100_000,
            max_depth: 128,
            max_total_bytes: 1 << 40,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TreeManifest {
    schema: String,
    entries: Vec<TreeEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeWire {
    schema: String,
    entries: Vec<TreeEntry>,
}
impl TreeManifest {
    pub fn new(mut entries: Vec<TreeEntry>, limits: TreeLimits) -> Result<Self> {
        entries.sort_by(|a, b| a.name().cmp(b.name()));
        let manifest = Self {
            schema: TREE_SCHEMA.into(),
            entries,
        };
        manifest.validate(limits)?;
        if manifest.canonical_bytes()?.len() > limits.max_manifest_bytes {
            return Err(Error::TooLarge);
        }
        Ok(manifest)
    }
    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self).map_err(|_| Error::Invalid("cannot encode tree manifest".into()))
    }
    /// Require canonical bytes, not a lossy JSON Value round-trip. Derive's
    /// struct visitor rejects repeated fields before any normalization occurs.
    pub fn from_bytes(bytes: &[u8], limits: TreeLimits) -> Result<Self> {
        if bytes.len() > limits.max_manifest_bytes {
            return Err(Error::TooLarge);
        }
        let wire: TreeWire = serde_json::from_slice(bytes)
            .map_err(|_| Error::Invalid("invalid tree manifest encoding".into()))?;
        let manifest = Self {
            schema: wire.schema,
            entries: wire.entries,
        };
        manifest.validate(limits)?;
        if manifest.canonical_bytes()? != bytes {
            return Err(Error::Invalid("noncanonical tree manifest".into()));
        }
        Ok(manifest)
    }
    pub fn validate(&self, limits: TreeLimits) -> Result<()> {
        if self.schema != TREE_SCHEMA {
            return Err(Error::Invalid("unknown tree schema".into()));
        }
        if self.entries.len() > limits.max_entries {
            return Err(Error::TooLarge);
        }
        let mut names = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            validate_name(entry.name())?;
            if !names.insert(entry.name()) || previous.is_some_and(|p| p >= entry.name()) {
                return Err(Error::Invalid("duplicate or unordered tree entry".into()));
            }
            previous = Some(entry.name());
            if let TreeEntry::Symlink { target, .. } = entry {
                validate_symlink(target)?;
            }
        }
        Ok(())
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(Error::Invalid("invalid tree entry name".into()));
    }
    Ok(())
}

/// Lexical check only; root containment and cycles require the full tree and
/// are checked again by the materializer before creating any output.
pub fn validate_symlink(target: &str) -> Result<()> {
    if target.is_empty()
        || target.len() > 4096
        || target.starts_with('/')
        || target
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':'))
        || target.split('/').any(str::is_empty)
    {
        return Err(Error::Invalid("invalid relative symlink target".into()));
    }
    Ok(())
}
