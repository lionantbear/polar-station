use super::{layout::FlatNode, ProjectionNode};
use crate::{manifest::bundle::BundleLimits, Error, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Resolved {
    path: Vec<String>,
    follows: usize,
}

pub(super) fn validate(nodes: &BTreeMap<String, FlatNode>, limits: BundleLimits) -> Result<()> {
    let mut resolver = Resolver {
        nodes,
        memo: BTreeMap::new(),
        active: BTreeSet::new(),
        memo_bytes: 0,
        max_bytes: limits.max_bundle_bytes,
    };
    for (path, node) in nodes {
        if matches!(node.node, ProjectionNode::Symlink { .. }) {
            resolver.resolve(path)?;
        }
    }
    Ok(())
}
struct Resolver<'a> {
    nodes: &'a BTreeMap<String, FlatNode>,
    memo: BTreeMap<String, Resolved>,
    active: BTreeSet<String>,
    memo_bytes: usize,
    max_bytes: usize,
}
fn invalid() -> Error {
    Error::Invalid("symlink must resolve inside the projected tree".into())
}
impl Resolver<'_> {
    fn resolve(&mut self, path: &str) -> Result<Resolved> {
        if let Some(resolved) = self.memo.get(path) {
            return Ok(resolved.clone());
        }
        if self.active.len() >= 40 || !self.active.insert(path.into()) {
            return Err(invalid());
        }
        let ProjectionNode::Symlink { target } =
            &self.nodes.get(path).ok_or(Error::Integrity)?.node
        else {
            return Err(Error::Integrity);
        };
        let target = target.clone();
        let mut resolved: Vec<String> = path
            .rsplit_once('/')
            .map_or(vec![], |(p, _)| p.split('/').map(String::from).collect());
        let mut follows = 1usize;
        for component in target.split('/') {
            if !matches!(
                self.nodes.get(&resolved.join("/")).map(|n| &n.node),
                Some(ProjectionNode::Directory)
            ) {
                return Err(invalid());
            }
            match component {
                "." => (),
                ".." => {
                    if resolved.pop().is_none() {
                        return Err(invalid());
                    }
                }
                name => {
                    resolved.push(name.into());
                    let next = resolved.join("/");
                    match &self.nodes.get(&next).ok_or_else(invalid)?.node {
                        ProjectionNode::Symlink { .. } => {
                            let expanded = self.resolve(&next)?;
                            follows = follows
                                .checked_add(expanded.follows)
                                .ok_or(Error::TooLarge)?;
                            if follows > 40 {
                                return Err(invalid());
                            }
                            resolved = expanded.path;
                        }
                        ProjectionNode::Directory | ProjectionNode::File { .. } => (),
                    }
                }
            }
        }
        self.active.remove(path);
        self.memo_bytes = self
            .memo_bytes
            .checked_add(path.len())
            .and_then(|n| n.checked_add(resolved.iter().map(|s| s.len() + 1).sum::<usize>()))
            .ok_or(Error::TooLarge)?;
        if self.memo_bytes > self.max_bytes {
            return Err(Error::TooLarge);
        }
        let result = Resolved {
            path: resolved,
            follows,
        };
        self.memo.insert(path.into(), result.clone());
        Ok(result)
    }
}
