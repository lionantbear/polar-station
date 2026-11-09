use super::*;
use crate::{
    manifest::{
        bundle::{AccessMode, BundleDraft, Completeness},
        validate_name, TreeEntry, TreeManifest,
    },
    Error,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(super) struct FlatNode {
    pub node: ProjectionNode,
    original: Option<ContentRef>,
}
type FlatTree = BTreeMap<String, FlatNode>;

fn metadata_bundle(
    root: ContentRef,
    directories: Vec<DirectoryRecord>,
    limits: BundleLimits,
) -> Result<ManifestBundle> {
    ManifestBundle::new(
        BundleDraft {
            root,
            directories,
            storages: vec![],
            sources: vec![],
            access: AccessMode::Durable,
            completeness: Completeness::ReferencesOnly,
        },
        limits,
    )
}
pub(super) fn plan(
    root: &ContentRef,
    directories: &[DirectoryRecord],
    layout: &ProjectionLayout,
    algorithm: DigestAlgorithm,
    limits: BundleLimits,
) -> Result<ProjectionTree> {
    let input = metadata_bundle(root.clone(), directories.to_vec(), limits)?;
    let original = flatten(&input, limits)?;
    let layout = normalize(layout, limits)?;
    let selected = match &layout {
        ProjectionLayout::Preserve => original.clone(),
        ProjectionLayout::Select(mappings) => select(&original, mappings, limits)?,
    };
    super::links::validate(&selected, limits)?;
    let unchanged = selected.len() == original.len()
        && selected
            .iter()
            .all(|(path, node)| original.get(path).is_some_and(|old| old.node == node.node));
    let output = if unchanged {
        input
    } else {
        derive(&selected, &original, Some(&input), algorithm, limits)?
    };
    Ok(ProjectionTree {
        input: root.clone(),
        layout,
        output,
        limits,
        algorithm,
        entries: selected
            .into_iter()
            .map(|(path, node)| ProjectionEntry {
                path,
                node: node.node,
            })
            .collect(),
    })
}
pub(super) fn normalize(
    layout: &ProjectionLayout,
    limits: BundleLimits,
) -> Result<ProjectionLayout> {
    match layout {
        ProjectionLayout::Preserve => Ok(ProjectionLayout::Preserve),
        ProjectionLayout::Select(entries) => {
            if entries.len() > limits.tree.max_entries.saturating_add(1) {
                return Err(Error::TooLarge);
            }
            let mut entries = entries.clone();
            for entry in &entries {
                validate_path(&entry.source, limits)?;
                validate_path(&entry.destination, limits)?;
            }
            entries.sort_by(|a, b| (&a.destination, &a.source).cmp(&(&b.destination, &b.source)));
            Ok(ProjectionLayout::Select(entries))
        }
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
    for part in path.split('/') {
        validate_name(part)?;
    }
    Ok(())
}
fn joined(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else if name.is_empty() {
        parent.into()
    } else {
        format!("{parent}/{name}")
    }
}
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

struct Budget {
    limits: BundleLimits,
    entries: usize,
    paths: usize,
    bytes: u64,
}
impl Budget {
    fn new(limits: BundleLimits) -> Self {
        Self {
            limits,
            entries: 0,
            paths: 0,
            bytes: 0,
        }
    }
    fn add(&mut self, path: &str, node: &ProjectionNode) -> Result<()> {
        if !path.is_empty() {
            self.entries = self.entries.checked_add(1).ok_or(Error::TooLarge)?;
            let depth = if matches!(node, ProjectionNode::Directory) {
                path.split('/').count()
            } else {
                path.split('/').count() - 1
            };
            if depth > self.limits.tree.max_depth {
                return Err(Error::TooLarge);
            }
        }
        self.paths = self.paths.checked_add(path.len()).ok_or(Error::TooLarge)?;
        match node {
            ProjectionNode::File { content, .. } => {
                self.bytes = self
                    .bytes
                    .checked_add(content.size_bytes)
                    .ok_or(Error::TooLarge)?
            }
            ProjectionNode::Symlink { target } => {
                self.paths = self
                    .paths
                    .checked_add(target.len())
                    .ok_or(Error::TooLarge)?
            }
            ProjectionNode::Directory => (),
        }
        if self.entries > self.limits.tree.max_entries
            || self.paths > self.limits.max_bundle_bytes
            || self.bytes > self.limits.tree.max_total_bytes
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}
fn flatten(input: &ManifestBundle, limits: BundleLimits) -> Result<FlatTree> {
    let nodes: BTreeMap<_, _> = input
        .directories()
        .iter()
        .map(|d| {
            Ok((
                d.content.id.to_string(),
                TreeManifest::from_bytes(d.canonical_json.as_bytes(), limits.tree)?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut result = FlatTree::new();
    let mut pending = vec![(String::new(), input.root().clone())];
    let mut budget = Budget::new(limits);
    while let Some((path, reference)) = pending.pop() {
        let tree = nodes
            .get(&reference.id.to_string())
            .ok_or(Error::Integrity)?;
        budget.add(&path, &ProjectionNode::Directory)?;
        if result
            .insert(
                path.clone(),
                FlatNode {
                    node: ProjectionNode::Directory,
                    original: Some(reference),
                },
            )
            .is_some()
        {
            return Err(Error::Integrity);
        }
        for entry in tree.entries() {
            let path = joined(&path, entry.name());
            let node = match entry {
                TreeEntry::Directory { manifest, .. } => {
                    pending.push((path, manifest.clone()));
                    continue;
                }
                TreeEntry::File {
                    content,
                    executable,
                    ..
                } => ProjectionNode::File {
                    content: content.clone(),
                    executable: *executable,
                },
                TreeEntry::Symlink { target, .. } => ProjectionNode::Symlink {
                    target: target.clone(),
                },
            };
            budget.add(&path, &node)?;
            if result
                .insert(
                    path,
                    FlatNode {
                        node,
                        original: None,
                    },
                )
                .is_some()
            {
                return Err(Error::Integrity);
            }
        }
    }
    Ok(result)
}
fn select(
    original: &FlatTree,
    mappings: &[ProjectionMapping],
    limits: BundleLimits,
) -> Result<FlatTree> {
    let mut result = FlatTree::new();
    result.insert(
        String::new(),
        FlatNode {
            node: ProjectionNode::Directory,
            original: None,
        },
    );
    let mut explicit = BTreeSet::new();
    let mut budget = Budget::new(limits);
    for mapping in mappings {
        let first = original.get(&mapping.source).ok_or(Error::NotFound)?;
        if mapping.destination.is_empty() && !matches!(first.node, ProjectionNode::Directory) {
            return Err(Error::Conflict);
        }
        let prefix = if mapping.source.is_empty() {
            String::new()
        } else {
            format!("{}/", mapping.source)
        };
        let children = original
            .range(prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(&prefix))
            .filter(|(path, _)| !path.is_empty());
        for (source, node) in std::iter::once((&mapping.source, first)).chain(children) {
            let suffix = if *source == mapping.source {
                ""
            } else {
                source.strip_prefix(&prefix).ok_or(Error::Integrity)?
            };
            let destination = joined(&mapping.destination, suffix);
            let mut ancestor = String::new();
            let parts: Vec<_> = destination.split('/').collect();
            for part in parts.iter().take(parts.len().saturating_sub(1)) {
                ancestor = joined(&ancestor, part);
                match result.get(&ancestor) {
                    Some(FlatNode {
                        node: ProjectionNode::Directory,
                        ..
                    }) => (),
                    Some(_) => return Err(Error::Conflict),
                    None => {
                        budget.add(&ancestor, &ProjectionNode::Directory)?;
                        result.insert(
                            ancestor.clone(),
                            FlatNode {
                                node: ProjectionNode::Directory,
                                original: None,
                            },
                        );
                    }
                }
            }
            if let Some(old) = result.get(&destination) {
                if explicit.contains(&destination)
                    || !matches!(
                        (&old.node, &node.node),
                        (ProjectionNode::Directory, ProjectionNode::Directory)
                    )
                {
                    return Err(Error::Conflict);
                }
            } else {
                budget.add(&destination, &node.node)?;
            }
            explicit.insert(destination.clone());
            result.insert(destination, node.clone());
        }
    }
    Ok(result)
}
fn derive(
    selected: &FlatTree,
    original: &FlatTree,
    input: Option<&ManifestBundle>,
    algorithm: DigestAlgorithm,
    limits: BundleLimits,
) -> Result<ManifestBundle> {
    let records: BTreeMap<_, _> = input
        .into_iter()
        .flat_map(|bundle| bundle.directories())
        .map(|r| (r.content.id.to_string(), r))
        .collect();
    let mut children = BTreeMap::<String, Vec<&String>>::new();
    for path in selected.keys().filter(|path| !path.is_empty()) {
        children.entry(parent(path).into()).or_default().push(path);
    }
    let mut built = BTreeMap::<String, DirectoryRecord>::new();
    for (path, node) in selected
        .iter()
        .rev()
        .filter(|(_, n)| matches!(n.node, ProjectionNode::Directory))
    {
        let mut entries = Vec::new();
        for child in children.get(path).into_iter().flatten() {
            let name = child.rsplit('/').next().ok_or(Error::Integrity)?.to_owned();
            entries.push(match &selected.get(*child).ok_or(Error::Integrity)?.node {
                ProjectionNode::Directory => TreeEntry::Directory {
                    name,
                    manifest: built.get(*child).ok_or(Error::Integrity)?.content.clone(),
                },
                ProjectionNode::File {
                    content,
                    executable,
                } => TreeEntry::File {
                    name,
                    content: content.clone(),
                    executable: *executable,
                },
                ProjectionNode::Symlink { target } => TreeEntry::Symlink {
                    name,
                    target: target.clone(),
                },
            });
        }
        let tree = TreeManifest::new(entries, limits.tree)?;
        let bytes = tree.canonical_bytes()?;
        let old = node
            .original
            .as_ref()
            .or_else(|| original.get(path).and_then(|n| n.original.as_ref()));
        let preserved = old
            .and_then(|r| records.get(&r.id.to_string()))
            .filter(|r| r.canonical_json.as_bytes() == bytes);
        let record = match preserved {
            Some(record) => (*record).clone(),
            None => DirectoryRecord::from_tree(&tree, algorithm)?,
        };
        built.insert(path.clone(), record);
    }
    let root = built.get("").ok_or(Error::Integrity)?.content.clone();
    let unique: BTreeMap<_, _> = built
        .into_values()
        .map(|record| (record.content.id.to_string(), record))
        .collect();
    metadata_bundle(root, unique.into_values().collect(), limits)
}

pub(super) fn from_entries(
    entries: impl IntoIterator<Item = ProjectionEntry>,
    algorithm: DigestAlgorithm,
    limits: BundleLimits,
) -> Result<ProjectionTree> {
    let mut selected = FlatTree::new();
    let mut budget = Budget::new(limits);
    for entry in entries {
        validate_path(&entry.path, limits)?;
        budget.add(&entry.path, &entry.node)?;
        if selected
            .insert(
                entry.path,
                FlatNode {
                    node: entry.node,
                    original: None,
                },
            )
            .is_some()
        {
            return Err(Error::Conflict);
        }
    }
    if !matches!(
        selected.get("").map(|node| &node.node),
        Some(ProjectionNode::Directory)
    ) {
        return Err(Error::Invalid(
            "tree root must be an explicit directory".into(),
        ));
    }
    for (path, node) in &selected {
        if !path.is_empty()
            && !matches!(
                selected.get(parent(path)).map(|node| &node.node),
                Some(ProjectionNode::Directory)
            )
        {
            return Err(Error::Invalid(
                "tree entry requires a directory parent".into(),
            ));
        }
        if let ProjectionNode::Symlink { target } = &node.node {
            crate::manifest::validate_symlink(target)?;
        }
    }
    super::links::validate(&selected, limits)?;
    let output = derive(&selected, &FlatTree::new(), None, algorithm, limits)?;
    Ok(ProjectionTree {
        input: output.root().clone(),
        layout: ProjectionLayout::Preserve,
        output,
        limits,
        algorithm,
        entries: selected
            .into_iter()
            .map(|(path, node)| ProjectionEntry {
                path,
                node: node.node,
            })
            .collect(),
    })
}
