//! Product-agnostic package-tree validation, hashing, CAS, and hardlink projection.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileDigest(String);

impl FileDigest {
    /// Preserve the package digest spelling when adapting an admitted CAS tree.
    pub fn from_object_id(id: &polar_cas::ObjectId) -> Result<Self> {
        id.require_sha256()?;
        Ok(Self(id.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn for_file(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(Self(polar_cas::ObjectId::for_reader(file)?.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TreeDigest(String);

impl TreeDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub size: u64,
    pub digest: FileDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTree {
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub total_bytes: u64,
    pub digest: TreeDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RootfsEntryKind {
    Directory {
        mode: u32,
    },
    File {
        mode: u32,
        size: u64,
        digest: FileDigest,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootfsEntry {
    pub path: String,
    #[serde(flatten)]
    pub kind: RootfsEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRootfsTree {
    pub root: PathBuf,
    pub entries: Vec<RootfsEntry>,
    pub total_file_bytes: u64,
    pub digest: TreeDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    pub max_compressed_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
    pub max_compression_ratio: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootfsLimits {
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
}

impl Default for RootfsLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 16 * 1024 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024 * 1024,
            max_entries: 1_000_000,
            max_path_bytes: 4_096,
            max_depth: 128,
        }
    }
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: 256 * 1024 * 1024,
            max_expanded_bytes: 1024 * 1024 * 1024,
            max_file_bytes: 256 * 1024 * 1024,
            max_entries: 16_384,
            max_path_bytes: 4_096,
            max_depth: 64,
            max_compression_ratio: 100,
        }
    }
}

pub fn validate_directory(root: &Path) -> Result<ValidatedTree> {
    validate_directory_with_limits(root, ArchiveLimits::default())
}

pub fn validate_directory_with_limits(root: &Path, limits: ArchiveLimits) -> Result<ValidatedTree> {
    let metadata = root
        .symlink_metadata()
        .with_context(|| format!("inspect package tree {}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "package tree root must be a real directory: {}",
            root.display()
        );
    }
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    collect_directory(root, root, 0, limits, &mut entries, &mut total_bytes)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let digest = digest_entries(&entries);
    Ok(ValidatedTree {
        root: root.to_path_buf(),
        entries,
        total_bytes,
        digest,
    })
}

/// Validate an assembled container rootfs without weakening plugin/package
/// rules. Rootfs symlinks are allowed only when their relative target remains
/// inside the same root and resolves in the final tree.
pub fn validate_rootfs_directory(root: &Path) -> Result<ValidatedRootfsTree> {
    validate_rootfs_directory_with_limits(root, RootfsLimits::default())
}

pub fn validate_rootfs_directory_with_limits(
    root: &Path,
    limits: RootfsLimits,
) -> Result<ValidatedRootfsTree> {
    let metadata = root
        .symlink_metadata()
        .with_context(|| format!("inspect rootfs tree {}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "rootfs tree root must be a real directory: {}",
            root.display()
        );
    }
    let mut entries = Vec::new();
    let mut total_file_bytes = 0_u64;
    collect_rootfs_directory(root, root, 0, limits, &mut entries, &mut total_file_bytes)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    if entries.is_empty() {
        bail!("rootfs tree contains no entries");
    }
    let digest = digest_rootfs_entries(&entries);
    Ok(ValidatedRootfsTree {
        root: root.to_path_buf(),
        entries,
        total_file_bytes,
        digest,
    })
}

pub fn extract_zip_file(
    archive_path: &Path,
    destination: &Path,
    limits: ArchiveLimits,
) -> Result<ValidatedTree> {
    let metadata = archive_path
        .symlink_metadata()
        .with_context(|| format!("inspect archive {}", archive_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("archive must be a regular file: {}", archive_path.display());
    }
    if metadata.len() > limits.max_compressed_bytes {
        bail!("archive exceeds compressed size limit");
    }
    if destination.exists() {
        bail!(
            "archive destination already exists: {}",
            destination.display()
        );
    }
    let parent = destination.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "archive destination has no parent: {}",
            destination.display()
        )
    })?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create archive destination parent {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".polar-package-tree-")
        .tempdir_in(parent)
        .context("create archive extraction staging directory")?;
    let staging = temporary.path().join("tree");
    std::fs::create_dir(&staging).context("create archive extraction root")?;

    let file = File::open(archive_path)
        .with_context(|| format!("open archive {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("open zip archive")?;
    if archive.len() > limits.max_entries {
        bail!("archive exceeds entry count limit");
    }
    let mut seen = HashSet::new();
    let mut expanded = 0_u64;
    let mut file_count = 0_usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("read zip entry {index}"))?;
        let relative = safe_zip_path(entry.name_raw(), limits)?;
        if !seen.insert(relative.clone()) {
            bail!("archive contains duplicate path: {}", relative.display());
        }
        validate_zip_mode(entry.unix_mode(), entry.is_dir(), &relative)?;
        if entry.size() > limits.max_file_bytes {
            bail!("archive file exceeds size limit: {}", relative.display());
        }
        if entry.size() > 0 {
            let compressed = entry.compressed_size().max(1);
            if entry.size() / compressed > limits.max_compression_ratio {
                bail!(
                    "archive entry exceeds compression ratio limit: {}",
                    relative.display()
                );
            }
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("archive expanded size overflow"))?;
        if expanded > limits.max_expanded_bytes {
            bail!("archive exceeds expanded size limit");
        }
        let output = staging.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)
                .with_context(|| format!("create archive directory {}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create archive parent {}", parent.display()))?;
        }
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .with_context(|| format!("create archive file {}", output.display()))?;
        let copied = std::io::copy(
            &mut entry.by_ref().take(limits.max_file_bytes + 1),
            &mut target,
        )
        .with_context(|| format!("extract archive file {}", output.display()))?;
        if copied > limits.max_file_bytes || copied != entry.size() {
            bail!(
                "archive entry size changed while extracting: {}",
                relative.display()
            );
        }
        target
            .flush()
            .with_context(|| format!("flush archive file {}", output.display()))?;
        file_count += 1;
    }
    if file_count == 0 {
        bail!("archive contains no files");
    }
    let staged_tree = validate_directory_with_limits(&staging, limits)?;
    std::fs::rename(&staging, destination).with_context(|| {
        format!(
            "atomically publish extracted tree {} -> {}",
            staging.display(),
            destination.display()
        )
    })?;
    Ok(ValidatedTree {
        root: destination.to_path_buf(),
        ..staged_tree
    })
}

#[derive(Debug, Clone)]
pub struct CasStore {
    root: PathBuf,
}

impl CasStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn tree_path(&self, digest: &TreeDigest) -> PathBuf {
        self.root.join(digest_key(digest)).join("rootfs")
    }

    pub fn admit(&self, tree: &ValidatedTree) -> Result<ValidatedTree> {
        let target = self.tree_path(&tree.digest);
        publish_hardlink_tree(tree, &target)
    }
}

#[derive(Debug, Clone)]
pub struct HardlinkProjection {
    root: PathBuf,
}

impl HardlinkProjection {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn publish(&self, tree: &ValidatedTree) -> Result<ValidatedTree> {
        publish_hardlink_tree(tree, &self.root)
    }
}

fn collect_directory(
    root: &Path,
    current: &Path,
    depth: usize,
    limits: ArchiveLimits,
    output: &mut Vec<TreeEntry>,
    total_bytes: &mut u64,
) -> Result<()> {
    if depth > limits.max_depth {
        bail!("package tree exceeds depth limit at {}", current.display());
    }
    let mut entries = std::fs::read_dir(current)
        .with_context(|| format!("read package directory {}", current.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = path
            .symlink_metadata()
            .with_context(|| format!("inspect package path {}", path.display()))?;
        let relative = path.strip_prefix(root)?;
        let canonical = canonical_relative_path(relative, limits)?;
        if metadata.file_type().is_symlink() {
            bail!("package tree rejects symlink: {}", path.display());
        }
        if metadata.is_dir() {
            collect_directory(root, &path, depth + 1, limits, output, total_bytes)?;
        } else if metadata.is_file() {
            if metadata.len() > limits.max_file_bytes {
                bail!("package file exceeds size limit: {}", path.display());
            }
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| anyhow::anyhow!("package tree size overflow"))?;
            if *total_bytes > limits.max_expanded_bytes {
                bail!("package tree exceeds total size limit");
            }
            if output.len() >= limits.max_entries {
                bail!("package tree exceeds entry count limit");
            }
            output.push(TreeEntry {
                path: canonical,
                size: metadata.len(),
                digest: FileDigest::for_file(&path)?,
            });
        } else {
            bail!("package tree rejects special file: {}", path.display());
        }
    }
    Ok(())
}

fn collect_rootfs_directory(
    root: &Path,
    current: &Path,
    depth: usize,
    limits: RootfsLimits,
    output: &mut Vec<RootfsEntry>,
    total_file_bytes: &mut u64,
) -> Result<()> {
    if depth > limits.max_depth {
        bail!("rootfs tree exceeds depth limit at {}", current.display());
    }
    let mut entries = std::fs::read_dir(current)
        .with_context(|| format!("read rootfs directory {}", current.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if output.len() >= limits.max_entries {
            bail!("rootfs tree exceeds entry count limit");
        }
        let path = entry.path();
        let metadata = path
            .symlink_metadata()
            .with_context(|| format!("inspect rootfs path {}", path.display()))?;
        let relative = path.strip_prefix(root)?;
        let canonical =
            canonical_relative_path_bounded(relative, limits.max_depth, limits.max_path_bytes)?;
        let mode = metadata.mode() & 0o7777;
        let kind = if metadata.is_dir() {
            output.push(RootfsEntry {
                path: canonical,
                kind: RootfsEntryKind::Directory { mode },
            });
            collect_rootfs_directory(root, &path, depth + 1, limits, output, total_file_bytes)?;
            continue;
        } else if metadata.is_file() {
            if metadata.len() > limits.max_file_bytes {
                bail!("rootfs file exceeds size limit: {}", path.display());
            }
            *total_file_bytes = total_file_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| anyhow::anyhow!("rootfs tree size overflow"))?;
            if *total_file_bytes > limits.max_total_bytes {
                bail!("rootfs tree exceeds total size limit");
            }
            RootfsEntryKind::File {
                mode,
                size: metadata.len(),
                digest: FileDigest::for_file(&path)?,
            }
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .with_context(|| format!("read rootfs symlink {}", path.display()))?;
            validate_rootfs_symlink_target(root, relative, &target, limits)?;
            RootfsEntryKind::Symlink {
                target: target
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("rootfs symlink target is not valid UTF-8"))?
                    .to_owned(),
            }
        } else {
            bail!("rootfs tree rejects special file: {}", path.display());
        };
        output.push(RootfsEntry {
            path: canonical,
            kind,
        });
    }
    Ok(())
}

fn validate_rootfs_symlink_target(
    root: &Path,
    link: &Path,
    target: &Path,
    limits: RootfsLimits,
) -> Result<()> {
    if target.is_absolute() {
        bail!("rootfs symlink target must be relative: {}", link.display());
    }
    let raw = target
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("rootfs symlink target is not valid UTF-8"))?;
    if raw.is_empty()
        || raw.len() > limits.max_path_bytes
        || raw.contains('\\')
        || raw.contains('\0')
    {
        bail!("rootfs symlink target is unsafe: {}", link.display());
    }
    let mut resolved = link
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => resolved.push(value.to_owned()),
            Component::ParentDir => {
                if resolved.pop().is_none() {
                    bail!("rootfs symlink escapes root: {}", link.display());
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("rootfs symlink target is unsafe: {}", link.display())
            }
        }
    }
    let resolved_path = root.join(resolved.iter().collect::<PathBuf>());
    std::fs::metadata(&resolved_path).with_context(|| {
        format!(
            "rootfs symlink target does not resolve inside the final tree: {}",
            link.display()
        )
    })?;
    Ok(())
}

fn canonical_relative_path(path: &Path, limits: ArchiveLimits) -> Result<String> {
    canonical_relative_path_bounded(path, limits.max_depth, limits.max_path_bytes)
}

fn canonical_relative_path_bounded(
    path: &Path,
    max_depth: usize,
    max_path_bytes: usize,
) -> Result<String> {
    if path.is_absolute() {
        bail!("package path must be relative");
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("package path is not valid UTF-8"))?;
                if value.contains('\\') || value.contains('\0') {
                    bail!("package path contains forbidden characters");
                }
                parts.push(value);
            }
            _ => bail!("package path contains unsafe component"),
        }
    }
    if parts.is_empty() || parts.len() > max_depth {
        bail!("package path is empty or exceeds depth limit");
    }
    let canonical = parts.join("/");
    if canonical.len() > max_path_bytes {
        bail!("package path exceeds length limit");
    }
    Ok(canonical)
}

fn safe_zip_path(raw: &[u8], limits: ArchiveLimits) -> Result<PathBuf> {
    let name = std::str::from_utf8(raw).context("archive path is not valid UTF-8")?;
    if name.contains('\\') || name.contains('\0') || name.starts_with('/') {
        bail!("archive contains unsafe path");
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let canonical = canonical_relative_path(Path::new(trimmed), limits)
        .context("archive contains unsafe path")?;
    Ok(PathBuf::from(canonical))
}

fn validate_zip_mode(mode: Option<u32>, is_dir: bool, path: &Path) -> Result<()> {
    let Some(mode) = mode else {
        return Ok(());
    };
    let kind = mode & 0o170000;
    let allowed = kind == 0 || kind == 0o100000 || (is_dir && kind == 0o040000);
    if !allowed {
        bail!(
            "archive rejects symlink or special file: {}",
            path.display()
        );
    }
    Ok(())
}

/// Hash an already canonical, path-sorted metadata list. This does not validate
/// paths or establish that file bytes exist; callers own that evidence.
pub fn digest_entries(entries: &[TreeEntry]) -> TreeDigest {
    let mut hasher = Sha256::new();
    hasher.update(b"polar-package-tree/v1\0");
    for entry in entries {
        hash_field(&mut hasher, entry.path.as_bytes());
        hasher.update(entry.size.to_be_bytes());
        hash_field(&mut hasher, entry.digest.as_str().as_bytes());
    }
    TreeDigest(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Hash an already canonical, path-sorted metadata list, using the same identity
/// as directory validation. Metadata alone is not proof of payload integrity.
pub fn digest_rootfs_entries(entries: &[RootfsEntry]) -> TreeDigest {
    let mut hasher = Sha256::new();
    hasher.update(b"polar-rootfs-tree/v1\0");
    for entry in entries {
        hash_field(&mut hasher, entry.path.as_bytes());
        match &entry.kind {
            RootfsEntryKind::Directory { mode } => {
                hasher.update(b"directory\0");
                hasher.update(mode.to_be_bytes());
            }
            RootfsEntryKind::File { mode, size, digest } => {
                hasher.update(b"file\0");
                hasher.update(mode.to_be_bytes());
                hasher.update(size.to_be_bytes());
                hash_field(&mut hasher, digest.as_str().as_bytes());
            }
            RootfsEntryKind::Symlink { target } => {
                hasher.update(b"symlink\0");
                hash_field(&mut hasher, target.as_bytes());
            }
        }
    }
    TreeDigest(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn digest_key(digest: &TreeDigest) -> &str {
    digest
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or(digest.as_str())
}

fn publish_hardlink_tree(tree: &ValidatedTree, target: &Path) -> Result<ValidatedTree> {
    if target.exists() {
        let existing = validate_directory(target)?;
        if existing.digest != tree.digest {
            bail!("existing tree digest does not match requested tree");
        }
        return Ok(existing);
    }
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("tree target has no parent: {}", target.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create tree target parent {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".polar-hardlink-tree-")
        .tempdir_in(parent)
        .context("create hardlink tree staging directory")?;
    let staging = temporary.path().join("tree");
    std::fs::create_dir(&staging)?;
    for entry in &tree.entries {
        let source = tree.root.join(&entry.path);
        let destination = staging.join(&entry.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        polar_cas_local::link::link_verified_file(&source, &destination).with_context(|| {
            format!(
                "hardlink package file {} -> {} (source and target must share a filesystem)",
                source.display(),
                destination.display()
            )
        })?;
    }
    let staged = validate_directory(&staging)?;
    if staged.digest != tree.digest {
        bail!("hardlink staging tree digest changed");
    }
    std::fs::rename(&staging, target)
        .with_context(|| format!("atomically publish hardlink tree to {}", target.display()))?;
    Ok(ValidatedTree {
        root: target.to_path_buf(),
        ..staged
    })
}


