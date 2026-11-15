//! Bounded exact-definition graph and occurrence-preserving native lowering.
//!
//! This is an in-memory view of admitted documents supplied by the runtime,
//! not a new wire format or a second user-authored projection specification.
use crate::{invalid, encode_idea, IdeaTree, Limits, NodeRef, Reference, Result, TreeFormat};
use polar_cas::{ContentRef, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub struct GraphLimits {
    pub max_documents: usize,
    pub max_bytes: u64,
    pub max_occurrences: usize,
    pub max_depth: usize,
}
impl Default for GraphLimits {
    fn default() -> Self {
        Self { max_documents: 256, max_bytes: 64 << 20, max_occurrences: 4096, max_depth: 64 }
    }
}

/// Slots are composition occurrences, never filesystem paths or UUID keys.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OccurrencePath(pub Vec<String>);
impl OccurrencePath {
    pub fn child(&self, slot: &str) -> Result<Self> {
        if !crate::reference::valid_key(slot) { return Err(invalid(slot, "invalid occurrence slot")); }
        let mut out = self.0.clone();
        out.push(slot.to_owned());
        Ok(Self(out))
    }
}

#[derive(Clone, Debug)]
pub struct DefinitionOrigin {
    pub definition: NodeRef,
    pub occurrence: OccurrencePath,
    pub filesystem_path: String,
}

/// Internal lowered view. Its nodes are copied per occurrence, but origins
/// always refer to the actual CAS document, not to this temporary structure.
#[derive(Clone, Debug)]
pub struct ExpandedIdea {
    pub idea: IdeaTree,
    pub origins: BTreeMap<String, DefinitionOrigin>,
}

#[derive(Clone, Debug)]
pub struct IdeaGraph {
    documents: BTreeMap<String, (ContentRef, IdeaTree)>,
    bytes: u64,
    pub limits: GraphLimits,
}
impl IdeaGraph {
    pub fn new(limits: GraphLimits) -> Self {
        Self { documents: BTreeMap::new(), bytes: 0, limits }
    }
    pub fn contains(&self, reference: &ContentRef) -> Result<bool> {
        match self.documents.get(&reference.id.to_string()) {
            Some((actual, _)) if actual != reference => Err(crate::Error::IdentityMismatch),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }
    /// Byte identity is verified here; this pure API does not mint admission.
    pub fn insert(&mut self, content: ContentRef, idea: IdeaTree, limits: &Limits) -> Result<()> {
        let encoded = encode_idea(&idea, limits)?;
        if content.size_bytes != encoded.bytes().len() as u64
            || ObjectId::for_bytes_with(content.id.algorithm(), encoded.bytes()) != content.id {
            return Err(crate::Error::IdentityMismatch);
        }
        if self.contains(&content)? { return Ok(()); }
        if self.documents.len() >= self.limits.max_documents { return Err(crate::Error::Limit("definition documents")); }
        let bytes = self.bytes.checked_add(content.size_bytes).ok_or(crate::Error::Limit("definition bytes"))?;
        if bytes > self.limits.max_bytes { return Err(crate::Error::Limit("definition bytes")); }
        self.documents.insert(content.id.to_string(), (content, idea));
        self.bytes = bytes;
        Ok(())
    }
    pub fn document(&self, reference: &ContentRef) -> Result<&IdeaTree> {
        let (actual, tree) = self.documents.get(&reference.id.to_string())
            .ok_or_else(|| invalid(reference.id.to_string(), "definition not loaded"))?;
        if actual != reference { return Err(crate::Error::IdentityMismatch); }
        Ok(tree)
    }
    pub fn node(&self, reference: &NodeRef) -> Result<&crate::itree::Node> {
        let tree = self.document(&reference.document)?;
        reference.check_target(&reference.document, tree)?;
        tree.node(&reference.node).ok_or_else(|| invalid(&reference.node, "missing node"))
    }
    pub fn exact(&self, reference: &NodeRef) -> Result<NodeRef> {
        let node = self.node(reference)?;
        Ok(NodeRef { expected_uuid: node.uuid(), ..reference.clone() })
    }
    pub fn resolve(&self, owner: &NodeRef, target: &Reference) -> Result<NodeRef> {
        let r = match target {
            Reference::Local { node } => NodeRef { document: owner.document.clone(), node: node.clone(), expected_uuid: None },
            Reference::Tree { format: TreeFormat::Itree, content, node, uuid } => NodeRef {
                document: content.clone(), node: node.clone(), expected_uuid: *uuid,
            },
            Reference::Object { uuid } => return Err(crate::Error::Unresolved(uuid.to_string())),
            _ => return Err(invalid(&owner.node, "reference is not an I-tree definition")),
        };
        // Assertions apply to every edge, even for already loaded documents.
        self.exact(&r)
    }
    pub fn select(&self, root: &NodeRef, path: &OccurrencePath) -> Result<NodeRef> {
        if path.0.len() > self.limits.max_depth { return Err(crate::Error::Limit("occurrence depth")); }
        let mut current = self.exact(root)?;
        for slot in &path.0 {
            let relation = self.node(&current)?.relations().get(slot)
                .ok_or_else(|| invalid(slot, "missing occurrence relation"))?;
            current = self.resolve(&current, &relation.target)?;
        }
        Ok(current)
    }
    pub fn documents(&self) -> impl Iterator<Item = (&ContentRef, &IdeaTree)> {
        self.documents.values().map(|(r, t)| (r, t))
    }
    /// Expand only filesystem entry edges. Semantic/back links are not recursively
    /// executed. Sources keep an exact reference so the runtime can type-check
    /// and resolve their admitted bytes separately.
    pub fn expand_filesystem(&self, root: &NodeRef, limits: &Limits) -> Result<ExpandedIdea> {
        let root = self.exact(root)?;
        if self.node(&root)?.uuid().is_none() { return Err(invalid(&root.node, "native projection UUID required")); }
        let mut expander = Expander { graph: self, nodes: BTreeMap::new(), origins: BTreeMap::new(),
            active: BTreeSet::new(), counter: 0, bytes: 0, max_bytes: limits.max_bytes };
        let key = expander.expand(&root, OccurrencePath::default(), String::new(), 0)?;
        let value = serde_json::json!({"schema":"polar.ideacave.itree.v1", "root":key, "nodes":expander.nodes});
        let idea = crate::parse_idea(&serde_json::to_vec(&value)?, limits)?;
        Ok(ExpandedIdea { idea, origins: expander.origins })
    }
}

struct Expander<'a> {
    graph: &'a IdeaGraph,
    nodes: BTreeMap<String, serde_json::Value>,
    origins: BTreeMap<String, DefinitionOrigin>,
    active: BTreeSet<(String, String)>,
    counter: usize,
    bytes: usize,
    max_bytes: usize,
}
impl Expander<'_> {
    fn expand(&mut self, reference: &NodeRef, occurrence: OccurrencePath, path: String, depth: usize) -> Result<String> {
        if depth > self.graph.limits.max_depth || depth > 128 { return Err(crate::Error::Limit("occurrence depth")); }
        if self.counter >= self.graph.limits.max_occurrences { return Err(crate::Error::Limit("filesystem occurrences")); }
        let identity = (reference.document.id.to_string(), reference.node.clone());
        if !self.active.insert(identity.clone()) { return Err(invalid(&reference.node, "definition composition cycle")); }
        let key = if self.counter == 0 { "output".to_owned() } else { format!("n{}", self.counter) };
        self.counter += 1;
        let source = self.graph.node(reference)?;
        let bytes = polar_cas::canonical::canonical_json_bytes(source)?.len();
        self.bytes = self.bytes.checked_add(bytes).ok_or(crate::Error::Limit("expanded description bytes"))?;
        if self.bytes > self.max_bytes { return Err(crate::Error::Limit("expanded description bytes")); }
        let node = source.clone();
        let mut value = serde_json::to_value(&node)?;
        for (slot, relation) in node.relations() {
            if relation.relation == "polar.fs.entry.v1" {
                let name = relation.properties.get("name").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid(slot, "filesystem name required"))?;
                if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
                    return Err(invalid(slot, "invalid filesystem name"));
                }
                let target = self.graph.resolve(reference, &relation.target)?;
                let child_path = if path.is_empty() { name.to_owned() } else { format!("{path}/{name}") };
                let child = self.expand(&target, occurrence.child(slot)?, child_path, depth + 1)?;
                value["relations"][slot]["target"] = serde_json::to_value(Reference::Local { node: child })?;
            } else if relation.relation == "polar.fs.source.v1" {
                // A Local source is relative to its ORIGINAL document, not the
                // generated occurrence keys. Make that ownership explicit.
                if let Reference::Local { .. } = &relation.target {
                    let target = self.graph.resolve(reference, &relation.target)?;
                    value["relations"][slot]["target"] = serde_json::to_value(Reference::Tree {
                        format: TreeFormat::Itree, content: target.document, node: target.node, uuid: target.expected_uuid,
                    })?;
                }
            }
        }
        self.active.remove(&identity);
        self.origins.insert(key.clone(), DefinitionOrigin { definition: reference.clone(), occurrence, filesystem_path: path });
        self.nodes.insert(key.clone(), value);
        Ok(key)
    }
}
