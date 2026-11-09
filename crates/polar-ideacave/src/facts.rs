//! Pure facts extraction. A future persisted logical index stores these facts;
//! it must not become the physical Catalog or invent a latest UUID definition.
use crate::{encode_idea, Error, IdeaTree, Limits, NodeRef, Reference, Result};
use polar_cas::{ContentRef, ObjectId};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NodeFact {
    pub node: NodeRef,
    pub uuid: Option<Uuid>,
    pub type_id: String,
}
#[derive(Clone, Debug)]
pub struct EdgeFact {
    pub from: NodeRef,
    pub slot: String,
    pub relation: String,
    pub target: Reference,
}
#[derive(Clone, Debug)]
pub struct DocumentFacts {
    pub document: ContentRef,
    pub nodes: Vec<NodeFact>,
    pub edges: Vec<EdgeFact>,
}
pub fn extract_facts(
    tree: &IdeaTree,
    document: &ContentRef,
    limits: &Limits,
) -> Result<DocumentFacts> {
    let encoded = encode_idea(tree, limits)?;
    if encoded.bytes().len() as u64 != document.size_bytes
        || ObjectId::for_bytes_with(document.id.algorithm(), encoded.bytes()) != document.id
    {
        return Err(Error::IdentityMismatch);
    }
    let mut facts = DocumentFacts {
        document: document.clone(),
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for (key, node) in tree.nodes() {
        let from = NodeRef {
            document: document.clone(),
            node: key.clone(),
            expected_uuid: node.uuid(),
        };
        facts.nodes.push(NodeFact {
            node: from.clone(),
            uuid: node.uuid(),
            type_id: node.type_id().into(),
        });
        for (slot, r) in node.relations() {
            facts.edges.push(EdgeFact {
                from: from.clone(),
                slot: slot.clone(),
                relation: r.relation.clone(),
                target: r.target.clone(),
            });
        }
        if let Some(schema) = node.type_schema() {
            facts.edges.push(EdgeFact {
                from,
                slot: "type_schema".into(),
                relation: "polar.type-schema.v1".into(),
                target: Reference::Content {
                    content: schema.clone(),
                    media_type: Some("application/schema+json".into()),
                },
            });
        }
    }
    Ok(facts)
}
