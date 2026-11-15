//! Source-backed input descriptions; canonical output remains TreeManifest v1.
use crate::{
    canonical::canonical_json_bytes,
    manifest::{bundle::BundleLimits, validate_name, validate_symlink},
    projection::{ProjectionEntry, ProjectionNode},
    source::SourceSpec,
    ContentRef, Error, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TreeFileInput {
    Source { source: SourceSpec },
    Content { content: ContentRef },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TreeSubtreeInput {
    Directory { source: SourceSpec },
    Stored { tree: ContentRef },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSubtree {
    pub path: String,
    pub input: TreeSubtreeInput,
}

/// Exact input scope. Entries use the same directory/link/executable semantics
/// as ProjectionTree, but external file identities have not been established yet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeDefinition {
    pub entries: Vec<ProjectionEntry<TreeFileInput>>,
    #[serde(default)]
    pub subtrees: Vec<TreeSubtree>,
}

impl TreeDefinition {
    pub fn normalized(&self, limits: BundleLimits) -> Result<Self> {
        if self.entries.len().saturating_add(self.subtrees.len())
            > limits.tree.max_entries.saturating_add(1)
        {
            return Err(Error::TooLarge);
        }
        let mut paths = BTreeSet::new();
        for entry in &self.entries {
            validate_path(&entry.path, limits)?;
            if !paths.insert(&entry.path) {
                return Err(Error::Conflict);
            }
            match &entry.node {
                ProjectionNode::Directory => {}
                ProjectionNode::Symlink { target } => validate_symlink(target)?,
                ProjectionNode::File {
                    content: TreeFileInput::Source { source },
                    ..
                } => source.locator.validate()?,
                ProjectionNode::File {
                    content: TreeFileInput::Content { content },
                    ..
                } => {
                    if content.size_bytes > limits.tree.max_total_bytes {
                        return Err(Error::TooLarge);
                    }
                }
            }
        }
        let mut subtrees = BTreeMap::new();
        for subtree in &self.subtrees {
            validate_path(&subtree.path, limits)?;
            let input = canonical_json_bytes(&subtree.input)
                .map_err(|_| Error::Invalid("subtree definition encoding".into()))?;
            if subtrees
                .insert((subtree.path.clone(), input), subtree.clone())
                .is_some()
            {
                return Err(Error::Conflict);
            }
            if let TreeSubtreeInput::Directory { source } = &subtree.input {
                source.locator.validate()?;
            }
        }
        let mut normalized = self.clone();
        normalized.entries.sort_by(|a, b| a.path.cmp(&b.path));
        normalized.subtrees = subtrees.into_values().collect();
        if canonical_json_bytes(&normalized)
            .map_err(|_| Error::Invalid("tree definition encoding".into()))?
            .len()
            > limits.max_bundle_bytes
        {
            return Err(Error::TooLarge);
        }
        Ok(normalized)
    }
}

fn validate_path(path: &str, limits: BundleLimits) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.len() > limits.max_bundle_bytes
        || path.split('/').count() > limits.tree.max_depth.saturating_add(1)
    {
        return Err(Error::TooLarge);
    }
    for component in path.split('/') {
        validate_name(component)?;
    }
    Ok(())
}
