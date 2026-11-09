//! Fixed native result records. These are descriptions, NOT admission witnesses.
//! Runtime storage must follow successful observation/admission. Native results
//! preserve v1 native roots and v2 composed results. Active instances are not
//! serialized as frozen filesystem content by either format.
use crate::{
    invalid, itree::strict_value, reference::valid_key, Error, Limits, NodeRef, Reference, Result,
    TreeFormat,
};
use polar_cas::{canonical::canonical_json_bytes, ContentRef, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeEntity {
    /// Structured observations owned by the named, versioned observer contract.
    Observation {
        observer: String,
        result: serde_json::Value,
    },
    /// A described whole whose determined members are recorded as relations.
    Composition,
    /// Stable pointer, NOT the current merged filesystem state. v3 only.
    ActiveDirectory {
        binding: crate::workspace::ActiveBinding,
    },
    Symlink {
        target: String,
    },
    File {
        content: ContentRef,
    },
    Directory {
        tree: ContentRef,
    },
    ErofsPack {
        image: ContentRef,
        tree: ContentRef,
    },
    TarPack {
        archive: ContentRef,
        tree: ContentRef,
    },
}
impl NativeEntity {
    pub fn contents(&self) -> Vec<&ContentRef> {
        match self {
            Self::Composition | Self::Symlink { .. } | Self::Observation { .. } => Vec::new(),
            Self::ActiveDirectory { binding } => vec![
                &binding.source.document,
                &binding.lower_tree,
                &binding.lower_image,
            ],
            Self::File { content } => vec![content],
            Self::Directory { tree } => vec![tree],
            Self::ErofsPack { image, tree } => vec![image, tree],
            Self::TarPack { archive, tree } => vec![archive, tree],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityNode {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::reference::optional_uuid"
    )]
    uuid: Option<Uuid>,
    definition: NodeRef,
    value: NativeEntity,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    relations: BTreeMap<String, crate::itree::Relation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::reference::present_optional"
    )]
    occurrence: Option<crate::composition::OccurrencePath>,
}
impl EntityNode {
    /// Pure structure; the runtime must verify admission before storing it.
    pub fn new(
        definition: NodeRef,
        value: NativeEntity,
        relations: BTreeMap<String, crate::itree::Relation>,
        occurrence: crate::composition::OccurrencePath,
    ) -> Self {
        Self {
            uuid: definition.expected_uuid,
            definition,
            value,
            relations,
            occurrence: Some(occurrence),
        }
    }
    pub fn relations(&self) -> &BTreeMap<String, crate::itree::Relation> {
        &self.relations
    }
    pub fn occurrence(&self) -> Option<&crate::composition::OccurrencePath> {
        self.occurrence.as_ref()
    }

    pub fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }
    pub fn definition(&self) -> &NodeRef {
        &self.definition
    }
    pub fn value(&self) -> &NativeEntity {
        &self.value
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntity {
    schema: String,
    root: String,
    nodes: BTreeMap<String, EntityNode>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct EntityTree {
    raw: RawEntity,
}
impl EntityTree {
    /// Construct a record of a native result. No CAS/physical assertion is
    /// bestowed by this pure function; the runtime supplies actual receipts.
    pub fn native_result(
        definition: NodeRef,
        uuid: Uuid,
        value: NativeEntity,
        limits: &Limits,
    ) -> Result<Self> {
        let raw = RawEntity {
            schema: "polar.ideacave.etree.v1".into(),
            root: "output".into(),
            nodes: [(
                "output".into(),
                EntityNode {
                    uuid: Some(uuid),
                    definition,
                    value,
                    relations: BTreeMap::new(),
                    occurrence: None,
                },
            )]
            .into(),
        };
        Self::validate(raw, limits)
    }
    pub fn composed(
        root: String,
        nodes: BTreeMap<String, EntityNode>,
        limits: &Limits,
    ) -> Result<Self> {
        let schema = if nodes
            .values()
            .any(|n| matches!(n.value, NativeEntity::Observation { .. }))
        {
            "polar.ideacave.etree.v4"
        } else if nodes
            .values()
            .any(|n| matches!(n.value, NativeEntity::ActiveDirectory { .. }))
        {
            "polar.ideacave.etree.v3"
        } else {
            "polar.ideacave.etree.v2"
        };
        Self::validate(
            RawEntity {
                schema: schema.into(),
                root,
                nodes,
            },
            limits,
        )
    }
    /// Records an observed mounted active instance. Caller supplies actual
    /// persisted backing and must not use this constructor to bypass admission.
    pub fn active_result(
        definition: NodeRef,
        binding: crate::workspace::ActiveBinding,
        limits: &Limits,
    ) -> Result<Self> {
        binding.validate()?;
        if definition.expected_uuid != Some(binding.instance) {
            return Err(Error::IdentityMismatch);
        }
        let node = EntityNode::new(
            definition,
            NativeEntity::ActiveDirectory { binding },
            BTreeMap::new(),
            crate::composition::OccurrencePath(Vec::new()),
        );
        Self::composed("output".into(), [("output".into(), node)].into(), limits)
    }
    pub fn schema(&self) -> &str {
        &self.raw.schema
    }
    fn validate(raw: RawEntity, limits: &Limits) -> Result<Self> {
        let v4 = raw.schema == "polar.ideacave.etree.v4";
        let v3 = raw.schema == "polar.ideacave.etree.v3" || v4;
        let v2 = raw.schema == "polar.ideacave.etree.v2" || v3;
        if (raw.schema != "polar.ideacave.etree.v1" && !v2)
            || !valid_key(&raw.root)
            || !raw.nodes.contains_key(&raw.root)
        {
            return Err(invalid("entity", "invalid root/schema"));
        }
        if raw.nodes.is_empty() || raw.nodes.len() > limits.max_nodes {
            return Err(Error::Limit("entity nodes"));
        }
        if raw.nodes[&raw.root].uuid.is_none() {
            return Err(invalid("entity", "root UUID required"));
        }
        let mut relation_count = 0usize;
        for (key, node) in &raw.nodes {
            if let NativeEntity::Observation { observer, result } = &node.value {
                if !v4 || !crate::reference::valid_type(observer) || !result.is_object() {
                    return Err(invalid(
                        key,
                        "observation requires v4, observer contract and object result",
                    ));
                }
                crate::itree::check_values(result, 0, limits.max_depth)?;
            }
            if !valid_key(key)
                || !valid_key(&node.definition.node)
                || node.definition.expected_uuid != node.uuid
            {
                return Err(Error::IdentityMismatch);
            }
            if !v2
                && (node.uuid.is_none()
                    || !node.relations.is_empty()
                    || node.occurrence.is_some()
                    || matches!(
                        node.value,
                        NativeEntity::Composition | NativeEntity::Symlink { .. }
                    ))
            {
                return Err(invalid(key, "v2 entity fields in v1"));
            }
            if v2 && node.occurrence.is_none() {
                return Err(invalid(key, "v2 occurrence required"));
            }
            if let NativeEntity::ActiveDirectory { binding } = &node.value {
                if !v3 || node.uuid != Some(binding.instance) {
                    return Err(invalid(
                        key,
                        "active entity requires v3 and matching identity",
                    ));
                }
                binding.validate()?;
            }
            if let NativeEntity::Symlink { target } = &node.value {
                polar_cas::manifest::validate_symlink(target)?;
            }
            if let Some(path) = &node.occurrence {
                if path.0.len() > limits.max_depth || path.0.iter().any(|s| !valid_key(s)) {
                    return Err(Error::Limit("entity occurrence"));
                }
            }
            if matches!(node.value, NativeEntity::Composition) && node.relations.is_empty() {
                return Err(invalid(key, "composition has no determined members"));
            }
            relation_count = relation_count
                .checked_add(node.relations.len())
                .ok_or(Error::Limit("entity relations"))?;
            if relation_count > limits.max_relations {
                return Err(Error::Limit("entity relations"));
            }
            for (slot, relation) in &node.relations {
                if !valid_key(slot) || !crate::reference::valid_type(&relation.relation) {
                    return Err(invalid(key, "invalid entity relation"));
                }
                for v in relation.properties.values() {
                    crate::itree::check_values(v, 0, limits.max_depth)?;
                }
                match &relation.target {
                    Reference::Local { node: target } if raw.nodes.contains_key(target) => {}
                    Reference::Content { .. } => {}
                    Reference::Tree {
                        format: TreeFormat::Etree,
                        node: target,
                        ..
                    } if valid_key(target) => {}
                    _ => return Err(invalid(key, "entity relation is not determined")),
                }
            }
            for content in std::iter::once(&node.definition.document).chain(node.value.contents()) {
                if content.size_bytes > i64::MAX as u64 {
                    return Err(Error::Limit("entity content size"));
                }
            }
            if let NativeEntity::TarPack { archive, .. } = &node.value {
                archive.id.require_sha256()?;
                if archive.size_bytes < 1024 || archive.size_bytes % 512 != 0 {
                    return Err(invalid(key, "tar image alignment"));
                }
            }
            if let NativeEntity::ErofsPack { image, .. } = &node.value {
                image.id.require_sha256()?;
                if image.size_bytes < 4096 || image.size_bytes % 4096 != 0 {
                    return Err(invalid(key, "EROFS image alignment"));
                }
            }
        }
        // Only containment/entry edges are an acyclic composition. Source
        // and evidence relations may refer back without inventing history edges.
        let mut colors = BTreeMap::new();
        let mut heights = BTreeMap::<&str, usize>::new();
        for key in raw.nodes.keys() {
            let mut stack = vec![(key.as_str(), false, 0usize)];
            while let Some((key, exit, depth)) = stack.pop() {
                if exit {
                    let mut height = 0usize;
                    for relation in raw.nodes[key].relations.values() {
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
                                        .ok_or(Error::Limit("entity composition depth"))?,
                                );
                            }
                        }
                    }
                    if height > limits.max_depth {
                        return Err(Error::Limit("entity composition depth"));
                    }
                    heights.insert(key, height);
                    colors.insert(key, 2u8);
                    continue;
                }
                match colors.get(key).copied().unwrap_or(0) {
                    2 => continue,
                    1 => return Err(invalid(key, "entity composition cycle")),
                    _ => {}
                }
                if depth > limits.max_depth {
                    return Err(Error::Limit("entity composition depth"));
                }
                colors.insert(key, 1);
                stack.push((key, true, depth));
                for relation in raw.nodes[key].relations.values().rev() {
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
        Ok(Self { raw })
    }
    pub fn root(&self) -> &str {
        &self.raw.root
    }
    pub fn node(&self, key: &str) -> Option<&EntityNode> {
        self.raw.nodes.get(key)
    }
    pub fn nodes(&self) -> &BTreeMap<String, EntityNode> {
        &self.raw.nodes
    }
    pub fn canonical_bytes(&self, limits: &Limits) -> Result<Vec<u8>> {
        Self::validate(self.raw.clone(), limits)?;
        let bytes = canonical_json_bytes(self)?;
        if bytes.len() > limits.max_bytes {
            return Err(Error::Limit("entity bytes"));
        }
        Ok(bytes)
    }
    pub fn decode_cas(bytes: &[u8], expected: &ContentRef, limits: &Limits) -> Result<Self> {
        if bytes.len() as u64 != expected.size_bytes
            || ObjectId::for_bytes_with(expected.id.algorithm(), bytes) != expected.id
        {
            return Err(Error::IdentityMismatch);
        }
        let raw = serde_json::from_value(strict_value(bytes, limits)?)?;
        let tree = Self::validate(raw, limits)?;
        if tree.canonical_bytes(limits)? != bytes {
            return Err(Error::NonCanonical);
        }
        Ok(tree)
    }
}
