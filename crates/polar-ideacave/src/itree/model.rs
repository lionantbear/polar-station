use crate::{
    invalid,
    reference::{valid_key, valid_type},
    Error, Reference, Result,
};
use polar_cas::ContentRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_nodes: usize,
    pub max_relations: usize,
    pub max_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 8 << 20,
            max_nodes: 4096,
            max_relations: 65536,
            max_depth: 64,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub relation: String,
    pub target: Reference,
    pub properties: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::reference::optional_uuid"
    )]
    uuid: Option<Uuid>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::reference::present_optional"
    )]
    type_schema: Option<ContentRef>,
    properties: BTreeMap<String, Value>,
    relations: BTreeMap<String, Relation>,
}
impl Node {
    pub fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }
    pub fn type_id(&self) -> &str {
        &self.kind
    }
    pub fn properties(&self) -> &BTreeMap<String, Value> {
        &self.properties
    }
    pub fn relations(&self) -> &BTreeMap<String, Relation> {
        &self.relations
    }
    pub fn type_schema(&self) -> Option<&ContentRef> {
        self.type_schema.as_ref()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTree {
    pub schema: String,
    pub root: String,
    pub nodes: BTreeMap<String, Node>,
}
/// Structurally checked description; does not prove CAS admission or existence.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct IdeaTree {
    pub(super) raw: RawTree,
}
impl IdeaTree {
    pub fn root(&self) -> &str {
        &self.raw.root
    }
    pub fn node(&self, key: &str) -> Option<&Node> {
        self.raw.nodes.get(key)
    }
    pub fn nodes(&self) -> &BTreeMap<String, Node> {
        &self.raw.nodes
    }
    pub fn root_node(&self) -> &Node {
        &self.raw.nodes[&self.raw.root]
    }
}
pub(crate) fn check_values(value: &Value, depth: usize, maximum: usize) -> Result<()> {
    if depth > maximum {
        return Err(Error::Limit("property depth"));
    }
    match value {
        Value::Number(n) if !n.is_i64() && !n.is_u64() => {
            Err(invalid("properties", "non-integer number"))
        }
        Value::Array(a) => {
            for v in a {
                check_values(v, depth + 1, maximum)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            for v in o.values() {
                check_values(v, depth + 1, maximum)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
impl RawTree {
    pub(super) fn validate(self, limits: &Limits) -> Result<IdeaTree> {
        if self.schema != "polar.ideacave.itree.v1" {
            return Err(invalid("schema", "unsupported schema"));
        }
        if self.nodes.is_empty() || self.nodes.len() > limits.max_nodes {
            return Err(Error::Limit("nodes"));
        }
        if !valid_key(&self.root) {
            return Err(invalid("root", "invalid key"));
        }
        let root = self
            .nodes
            .get(&self.root)
            .ok_or_else(|| invalid("root", "missing root"))?;
        if root.uuid.is_none() {
            return Err(invalid("root", "root UUID required"));
        }
        let mut edges = 0usize;
        for (key, node) in &self.nodes {
            if !valid_key(key) || !valid_type(&node.kind) {
                return Err(invalid(key, "invalid key/type"));
            }
            for value in node.properties.values() {
                check_values(value, 0, limits.max_depth)?;
            }
            edges = edges
                .checked_add(node.relations.len())
                .ok_or(Error::Limit("relations"))?;
            if edges > limits.max_relations {
                return Err(Error::Limit("relations"));
            }
            for (slot, relation) in &node.relations {
                if !valid_key(slot) || !valid_type(&relation.relation) {
                    return Err(invalid(key, "invalid relation"));
                }
                for v in relation.properties.values() {
                    check_values(v, 0, limits.max_depth)?;
                }
                match &relation.target {
                    Reference::Local { node }
                        if !valid_key(node) || !self.nodes.contains_key(node) =>
                    {
                        return Err(invalid(key, "missing local target"));
                    }
                    Reference::Tree { node, .. } if !valid_key(node) => {
                        return Err(invalid(key, "invalid external node"))
                    }
                    Reference::Content {
                        media_type: Some(s),
                        ..
                    } if s.is_empty() || s.len() > 160 => {
                        return Err(invalid(key, "invalid media type"))
                    }
                    _ => {}
                }
            }
        }
        // Only composition edges must be acyclic. Behavior/observation links
        // may legitimately refer back. Iterative DFS avoids recursion overflow.
        let mut colors = BTreeMap::<&str, u8>::new();
        let mut heights = BTreeMap::<&str, usize>::new();
        for key in self.nodes.keys() {
            let mut stack = vec![(key.as_str(), false, 0usize)];
            while let Some((key, exit, depth)) = stack.pop() {
                if exit {
                    let mut height = 0usize;
                    for relation in self.nodes[key].relations.values() {
                        if matches!(
                            relation.relation.as_str(),
                            "polar.contains.v1" | "polar.fs.entry.v1"
                        ) {
                            if let Reference::Local { node } = &relation.target {
                                height = height.max(
                                    heights
                                        .get(node.as_str())
                                        .copied()
                                        .unwrap_or(0)
                                        .checked_add(1)
                                        .ok_or(Error::Limit("composition depth"))?,
                                );
                            }
                        }
                    }
                    if height > limits.max_depth {
                        return Err(Error::Limit("composition depth"));
                    }
                    heights.insert(key, height);
                    colors.insert(key, 2);
                    continue;
                }
                match colors.get(key).copied().unwrap_or(0) {
                    2 => continue,
                    1 => return Err(invalid(key, "composition cycle")),
                    _ => {}
                }
                if depth > limits.max_depth {
                    return Err(Error::Limit("composition depth"));
                }
                colors.insert(key, 1);
                stack.push((key, true, depth));
                for relation in self.nodes[key].relations.values().rev() {
                    if matches!(
                        relation.relation.as_str(),
                        "polar.contains.v1" | "polar.fs.entry.v1"
                    ) {
                        if let Reference::Local { node } = &relation.target {
                            stack.push((node.as_str(), false, depth + 1));
                        }
                    }
                }
            }
        }
        Ok(IdeaTree { raw: self })
    }
}
