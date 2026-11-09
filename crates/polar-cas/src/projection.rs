//! Validated projection layout over the existing immutable directory format.
use crate::{
    manifest::bundle::{BundleLimits, DirectoryRecord, ManifestBundle},
    ContentRef, DigestAlgorithm, Error, Result,
};
use serde::{Deserialize, Serialize};
pub mod export;
mod layout;
mod links;
pub mod tree_functions;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    ReadOnly,
    Executable,
}
impl FileMode {
    pub fn bits(self) -> u32 {
        match self {
            Self::ReadOnly => 0o444,
            Self::Executable => 0o555,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOwner {
    pub uid: u32,
    pub gid: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantPolicy {
    ExistingOnly,
    ReflinkAllowed,
    CopyAllowed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMaterialization {
    HardlinkRequired,
    ReflinkAllowed,
    CopyAllowed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionProfile {
    pub files: FileMaterialization,
    pub variants: VariantPolicy,
    pub owner: FileOwner,
}

pub const LOCAL_TREE_SCHEMA: &str = "polar.cas.local-tree.v1";
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilePlacementMethod {
    Hardlink,
    Reflink,
    Copy,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePlacement {
    pub path: String,
    pub method: FilePlacementMethod,
}
/// On-disk description only, not evidence that the described tree still exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeProjectionRecord {
    pub schema: String,
    pub input: ContentRef,
    pub manifest: ContentRef,
    pub layout: ProjectionLayout,
    pub profile: ProjectionProfile,
    pub files: Vec<FilePlacement>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionStatistics {
    pub hardlinked_files: u64,
    pub reflinked_files: u64,
    pub copied_files: u64,
    pub copied_bytes: u64,
}
impl TreeProjectionRecord {
    /// Descriptive placement statistics, not filesystem verification or a
    /// count of later inspection/retry I/O. Native inspection supplies proof.
    pub fn statistics(&self, tree: &ProjectionTree) -> Result<ProjectionStatistics> {
        if self.schema != LOCAL_TREE_SCHEMA
            || self.input != *tree.input()
            || self.manifest != *tree.output().root()
            || self.layout != *tree.layout()
        {
            return Err(Error::Conflict);
        }
        let mut placements = self.files.iter();
        let mut stats = ProjectionStatistics::default();
        for entry in tree.entries() {
            let ProjectionNode::File { content, .. } = &entry.node else {
                continue;
            };
            let placed = placements.next().ok_or(Error::Integrity)?;
            if placed.path != entry.path {
                return Err(Error::Integrity);
            }
            match placed.method {
                FilePlacementMethod::Hardlink => stats.hardlinked_files += 1,
                FilePlacementMethod::Reflink
                    if self.profile.files != FileMaterialization::HardlinkRequired =>
                {
                    stats.reflinked_files += 1
                }
                FilePlacementMethod::Copy
                    if self.profile.files == FileMaterialization::CopyAllowed =>
                {
                    stats.copied_files += 1;
                    stats.copied_bytes = stats
                        .copied_bytes
                        .checked_add(content.size_bytes)
                        .ok_or(Error::TooLarge)?;
                }
                _ => return Err(Error::Integrity),
            }
        }
        if placements.next().is_some() {
            return Err(Error::Integrity);
        }
        Ok(stats)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMapping {
    pub source: String,
    pub destination: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "entries",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProjectionLayout {
    #[default]
    Preserve,
    Select(Vec<ProjectionMapping>),
}
impl ProjectionLayout {
    pub fn normalized(&self, limits: BundleLimits) -> Result<Self> {
        layout::normalize(self, limits)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionNode<C = ContentRef> {
    Directory,
    File { content: C, executable: bool },
    Symlink { target: String },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionEntry<C = ContentRef> {
    pub path: String,
    pub node: ProjectionNode<C>,
}

impl<C> ProjectionNode<C> {
    /// Substitute only file content slots, retaining authoritative tree metadata.
    pub fn try_map_content<D>(self, map: impl FnOnce(C) -> Result<D>) -> Result<ProjectionNode<D>> {
        Ok(match self {
            Self::Directory => ProjectionNode::Directory,
            Self::File {
                content,
                executable,
            } => ProjectionNode::File {
                content: map(content)?,
                executable,
            },
            Self::Symlink { target } => ProjectionNode::Symlink { target },
        })
    }
}

/// Sealed, fully validated tree; planning does not create payload replicas.
#[derive(Clone, Debug)]
pub struct ProjectionTree {
    input: ContentRef,
    layout: ProjectionLayout,
    output: ManifestBundle,
    entries: Vec<ProjectionEntry>,
    limits: BundleLimits,
    algorithm: DigestAlgorithm,
}
impl ProjectionTree {
    /// Build an explicitly described tree with the same directory encoding,
    /// size/path bounds and link resolution as ordinary projection planning.
    /// Entries must include the root and every parent directory; no I/O or
    /// payload availability is inferred. Used by format adapters after merge.
    pub fn from_entries(
        entries: impl IntoIterator<Item = ProjectionEntry>,
        algorithm: DigestAlgorithm,
        limits: BundleLimits,
    ) -> Result<Self> {
        layout::from_entries(entries, algorithm, limits)
    }
    pub fn plan(
        root: &ContentRef,
        directories: &[DirectoryRecord],
        layout: &ProjectionLayout,
        algorithm: DigestAlgorithm,
        limits: BundleLimits,
    ) -> Result<Self> {
        layout::plan(root, directories, layout, algorithm, limits)
    }
    pub fn input(&self) -> &ContentRef {
        &self.input
    }
    pub fn layout(&self) -> &ProjectionLayout {
        &self.layout
    }
    pub fn output(&self) -> &ManifestBundle {
        &self.output
    }
    /// Includes the root directory at the empty path; sorted by relative path.
    pub fn entries(&self) -> &[ProjectionEntry] {
        &self.entries
    }
    pub fn limits(&self) -> BundleLimits {
        self.limits
    }
    pub fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }
}
