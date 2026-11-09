//! Pure lowering of native filesystem definitions, not a project executor.
//!
//! Definitions and entry names come from the I-tree itself. A caller supplies
//! already prepared metadata views; this module never opens a host path, reads
//! a pack, queries a latest UUID, or certifies admission. The execution boundary
//! MUST require admitted immutable sources before consuming the returned tree.
use std::collections::BTreeMap;

use polar_cas::{
    manifest::bundle::BundleLimits,
    projection::{ProjectionEntry, ProjectionNode, ProjectionTree},
    ContentRef, DigestAlgorithm,
};
use serde_json::Value;

use crate::{invalid, itree::Node, Error, IdeaTree, Reference, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryView {
    CasDirectory,
    ErofsDirectory,
}

/// Resolves only explicit fixed references to previously prepared metadata.
/// Providers must apply selectors relative to the selected content, not to the
/// host filesystem. The result is rooted at "" and includes all parent entries.
/// This trait carries neither credentials nor an admission assertion.
pub trait FilesystemViews {
    fn directory(
        &self,
        source: &Reference,
        view: DirectoryView,
        selector: &str,
    ) -> Result<Vec<ProjectionEntry>>;

    fn file(&self, source: &Reference) -> Result<ContentRef>;
}

/// Lowers one native pack/copy/hardlink root, or a directory root, to the
/// existing CAS directory format. This is metadata planning only. Registered
/// runtime executors are still required to implement the respective methods.
pub fn lower_filesystem(
    idea: &IdeaTree,
    views: &impl FilesystemViews,
    algorithm: DigestAlgorithm,
    limits: BundleLimits,
) -> Result<ProjectionTree> {
    validate_native_root(idea)?;
    lower_filesystem_entries(idea, views, algorithm, limits, idea.root_node().type_id() == "polar.fs.directory.v1")
}

/// Registered executors own root semantics. This helper still validates every
/// filesystem entry, source interpretation, path and budget. It performs no IO
/// and cannot grant admission or authorize execution of an unknown definition.
pub fn lower_registered_filesystem(
    idea: &IdeaTree, views: &impl FilesystemViews, algorithm: DigestAlgorithm, limits: BundleLimits,
) -> Result<ProjectionTree> {
    lower_filesystem_entries(idea, views, algorithm, limits, false)
}

/// The four built-in root contracts, reused by the built-in executor objects.
pub fn validate_native_root(idea: &IdeaTree) -> Result<()> {
    let root = idea.root_node();
    match root.type_id() {
        "polar.project.erofs-pack.v1" => {
            check_properties(
                root,
                &["representation", "compression", "metadata_profile"],
                idea.root(),
            )?;
            require_string(root, "representation", &["standalone"], idea.root())?;
            require_string(root, "compression", &["none", "lz4"], idea.root())?;
            require_string(
                root,
                "metadata_profile",
                &["readonly-portable-v1"],
                idea.root(),
            )?;
        }
        "polar.project.tar-pack.v1" => {
            check_properties(root, &["metadata_profile", "archive_profile"], idea.root())?;
            require_string(root, "archive_profile", &["tar_v1"], idea.root())?;
            require_string(
                root,
                "metadata_profile",
                &["readonly-portable-v1"],
                idea.root(),
            )?;
        }
        "polar.project.directory-copy.v1" | "polar.project.directory-hardlink.v1" => {
            check_properties(root, &["metadata_profile", "owner"], idea.root())?;
            if let Some(owner) = root.properties().get("owner") {
                let _: polar_cas::projection::FileOwner = serde_json::from_value(owner.clone())
                    .map_err(|_| invalid(idea.root(), "invalid owner"))?;
            }
            require_string(
                root,
                "metadata_profile",
                &["readonly-portable-v1"],
                idea.root(),
            )?;
        }
        "polar.fs.directory.v1" => {}
        _ => return Err(Error::MissingHandler(idea.root().into())),
    }

    Ok(())
}

fn lower_filesystem_entries(
    idea: &IdeaTree, views: &impl FilesystemViews, algorithm: DigestAlgorithm, limits: BundleLimits,
    directory_root: bool,
) -> Result<ProjectionTree> {
    let mut out = BTreeMap::new();
    insert(&mut out, "".into(), ProjectionNode::Directory, limits)?;
    let mut budget = WalkBudget {
        visits: 0,
        max_visits: limits.tree.max_entries,
    };
    if directory_root {
        expand_directory(
            idea,
            idea.root(),
            "",
            views,
            &mut out,
            &mut budget,
            0,
            limits,
        )?;
    } else {
        expand_children(
            idea,
            idea.root(),
            "",
            views,
            &mut out,
            &mut budget,
            0,
            limits,
            true,
        )?;
    }
    Ok(ProjectionTree::from_entries(
        out.into_iter()
            .map(|(path, node)| ProjectionEntry { path, node }),
        algorithm,
        limits,
    )?)
}

struct WalkBudget {
    visits: usize,
    max_visits: usize,
}
impl WalkBudget {
    fn visit(&mut self, depth: usize, limits: BundleLimits) -> Result<()> {
        if depth > limits.tree.max_depth || depth > 128 {
            return Err(Error::Limit("filesystem occurrence depth"));
        }
        self.visits = self
            .visits
            .checked_add(1)
            .ok_or(Error::Limit("filesystem occurrences"))?;
        if self.visits > self.max_visits {
            return Err(Error::Limit("filesystem occurrences"));
        }
        Ok(())
    }
}

fn insert(
    entries: &mut BTreeMap<String, ProjectionNode>,
    path: String,
    node: ProjectionNode,
    limits: BundleLimits,
) -> Result<()> {
    if entries.len() >= limits.tree.max_entries {
        return Err(Error::Limit("filesystem entries"));
    }
    if entries.contains_key(&path) {
        return Err(invalid(path, "duplicate output path"));
    }
    entries.insert(path, node);
    Ok(())
}

fn child_path(parent: &str, name: &str) -> Result<String> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err(invalid(parent, "invalid filesystem entry name"));
    }
    Ok(if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    })
}

fn check_properties(node: &Node, allowed: &[&str], key: &str) -> Result<()> {
    if node
        .properties()
        .keys()
        .any(|p| !allowed.contains(&p.as_str()))
    {
        return Err(invalid(key, "unsupported native property"));
    }
    Ok(())
}

fn require_string(node: &Node, name: &str, values: &[&str], key: &str) -> Result<()> {
    match node.properties().get(name).and_then(Value::as_str) {
        Some(value) if values.contains(&value) => Ok(()),
        _ => Err(invalid(key, "missing or unsupported native property value")),
    }
}

fn fixed_source<'a>(node: &'a Node, key: &str) -> Result<Option<&'a Reference>> {
    let mut found = None;
    for relation in node.relations().values() {
        if relation.relation != "polar.fs.source.v1" {
            continue;
        }
        if found.is_some() || !relation.properties.is_empty() {
            return Err(invalid(
                key,
                "ambiguous source or unsupported source options",
            ));
        }
        match &relation.target {
            Reference::Content { .. } | Reference::Tree { .. } => found = Some(&relation.target),
            Reference::Object { .. } => return Err(Error::Unresolved(key.into())),
            Reference::Local { .. } => {
                return Err(Error::MissingHandler(format!("{key}:source-definition")));
            }
        }
    }
    Ok(found)
}

#[allow(clippy::too_many_arguments)]
fn expand_children(
    idea: &IdeaTree,
    key: &str,
    parent: &str,
    views: &impl FilesystemViews,
    out: &mut BTreeMap<String, ProjectionNode>,
    budget: &mut WalkBudget,
    depth: usize,
    limits: BundleLimits,
    project_root: bool,
) -> Result<()> {
    let node = idea.node(key).ok_or_else(|| invalid(key, "missing node"))?;
    for (slot, relation) in node.relations() {
        match relation.relation.as_str() {
            "polar.fs.source.v1" if !project_root => continue,
            // Placement is part of the same I-tree but not a filesystem entry.
            // A runtime executor MUST separately enforce these required targets.
            "polar.storage.required-at.v1" | "polar.storage.build-at.v1" if project_root => {
                continue
            }
            "polar.fs.entry.v1" => {}
            _ => return Err(Error::MissingHandler(format!("{key}/{slot}:relation"))),
        }
        if relation.properties.len() != 1 {
            return Err(invalid(
                format!("{key}/{slot}"),
                "filesystem entry requires only name",
            ));
        }
        let name = relation
            .properties
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("{key}/{slot}"), "filesystem name required"))?;
        let path = child_path(parent, name)?;
        let child = match &relation.target {
            Reference::Local { node } => node,
            Reference::Object { .. } => return Err(Error::Unresolved(format!("{key}/{slot}"))),
            _ => return Err(Error::MissingHandler(format!("{key}/{slot}:external-node"))),
        };
        expand_node(idea, child, &path, views, out, budget, depth + 1, limits)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn expand_node(
    idea: &IdeaTree,
    key: &str,
    path: &str,
    views: &impl FilesystemViews,
    out: &mut BTreeMap<String, ProjectionNode>,
    budget: &mut WalkBudget,
    depth: usize,
    limits: BundleLimits,
) -> Result<()> {
    budget.visit(depth, limits)?;
    let node = idea.node(key).ok_or_else(|| invalid(key, "missing node"))?;
    match node.type_id() {
        "polar.fs.directory.v1" => {
            insert(out, path.into(), ProjectionNode::Directory, limits)?;
            expand_directory(idea, key, path, views, out, budget, depth, limits)
        }
        "polar.fs.file.v1" => {
            check_properties(node, &["executable", "source_view"], key)?;
            require_string(node, "source_view", &["bytes"], key)?;
            if node
                .relations()
                .values()
                .any(|r| r.relation != "polar.fs.source.v1")
            {
                return Err(Error::MissingHandler(format!("{key}:file-relation")));
            }
            let executable = node
                .properties()
                .get("executable")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid(key, "executable boolean required"))?;
            let source =
                fixed_source(node, key)?.ok_or_else(|| invalid(key, "file source required"))?;
            let content = views.file(source)?;
            if let Reference::Content {
                content: expected, ..
            } = source
            {
                if expected != &content {
                    return Err(Error::IdentityMismatch);
                }
            }
            insert(
                out,
                path.into(),
                ProjectionNode::File {
                    content,
                    executable,
                },
                limits,
            )
        }
        "polar.fs.symlink.v1" => {
            check_properties(node, &["target"], key)?;
            if !node.relations().is_empty() {
                return Err(invalid(key, "symlink relations unsupported"));
            }
            let target = node
                .properties()
                .get("target")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(key, "symlink target required"))?;
            insert(
                out,
                path.into(),
                ProjectionNode::Symlink {
                    target: target.into(),
                },
                limits,
            )
        }
        _ => Err(Error::MissingHandler(key.into())),
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_directory(
    idea: &IdeaTree,
    key: &str,
    path: &str,
    views: &impl FilesystemViews,
    out: &mut BTreeMap<String, ProjectionNode>,
    budget: &mut WalkBudget,
    depth: usize,
    limits: BundleLimits,
) -> Result<()> {
    let node = idea.node(key).ok_or_else(|| invalid(key, "missing node"))?;
    check_properties(node, &["source_view", "selector"], key)?;
    if let Some(source) = fixed_source(node, key)? {
        let view = match node.properties().get("source_view").and_then(Value::as_str) {
            Some("cas-directory") => DirectoryView::CasDirectory,
            Some("erofs-directory") => DirectoryView::ErofsDirectory,
            _ => return Err(invalid(key, "directory source_view required")),
        };
        let selector = node
            .properties()
            .get("selector")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(key, "directory selector required"))?;
        if !selector.is_empty()
            && selector
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | "..") || p.contains(['\\', '\0']))
        {
            return Err(invalid(key, "invalid directory selector"));
        }
        let entries = views.directory(source, view, selector)?;
        // Existing CAS validation checks full source parents and link rules
        // before prefixing. No name/metadata is inferred from a pack digest.
        let selected = ProjectionTree::from_entries(entries, DigestAlgorithm::Sha256, limits)?;
        for entry in selected.entries() {
            if entry.path.is_empty() {
                if !matches!(entry.node, ProjectionNode::Directory) {
                    return Err(invalid(key, "directory view must have a directory root"));
                }
                continue;
            }
            let projected = if path.is_empty() {
                entry.path.clone()
            } else {
                format!("{path}/{}", entry.path)
            };
            insert(out, projected, entry.node.clone(), limits)?;
        }
    } else if !node.properties().is_empty() {
        return Err(invalid(key, "directory source options require a source"));
    }
    expand_children(idea, key, path, views, out, budget, depth, limits, false)
}
