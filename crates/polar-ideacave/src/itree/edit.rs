use crate::{encode_idea, invalid, EncodedIdea, Error, IdeaTree, Limits, Reference, Result};
/// Replace only one local relation after compare-with-expected. Does not imply
/// admission of replacement; callers must obtain a checked runtime binding.
pub fn replace_reference(
    tree: &IdeaTree,
    node: &str,
    slot: &str,
    expected: &Reference,
    replacement: Reference,
    limits: &Limits,
) -> Result<EncodedIdea> {
    let current = tree
        .node(node)
        .and_then(|n| n.relations().get(slot))
        .ok_or_else(|| invalid(node, "missing relation"))?;
    if &current.target != expected {
        return Err(Error::IdentityMismatch);
    }
    let mut value = serde_json::to_value(tree)?;
    value["nodes"][node]["relations"][slot]["target"] = serde_json::to_value(replacement)?;
    let updated = super::parse_idea(&serde_json::to_vec(&value)?, limits)?;
    encode_idea(&updated, limits)
}

/// Change exactly one LOCAL occurrence path. Path copying retains aliases of
/// shared nodes and does not invent new business UUIDs. The returned document
/// keeps its original root; `start` may select any node in it.
pub fn replace_local_occurrence(
    tree: &IdeaTree,
    start: &str,
    slots: &[String],
    expected: &Reference,
    replacement: Reference,
    limits: &Limits,
) -> Result<EncodedIdea> {
    if slots.is_empty() || slots.len() > limits.max_depth {
        return Err(Error::Limit("binding occurrence depth"));
    }
    let mut nodes = Vec::new();
    let mut current = start.to_owned();
    for (i, slot) in slots.iter().enumerate() {
        let relation = tree.node(&current).and_then(|n| n.relations().get(slot))
            .ok_or_else(|| invalid(slot, "missing binding occurrence"))?;
        nodes.push(current.clone());
        if i + 1 < slots.len() {
            current = match &relation.target {
                Reference::Local { node } => node.clone(),
                _ => return Err(invalid(slot, "binding segment must be local")),
            };
        } else if &relation.target != expected {
            return Err(Error::IdentityMismatch);
        }
    }
    let mut value = serde_json::to_value(tree)?;
    let mut target = replacement;
    for i in (0..slots.len()).rev() {
        let original = &nodes[i];
        let mut node = value["nodes"][original].clone();
        node["relations"][&slots[i]]["target"] = serde_json::to_value(&target)?;
        if i == 0 {
            value["nodes"][original] = node;
        } else {
            // Content-derived local alias, not a new logical-object identity.
            let bytes = polar_cas::canonical::canonical_json_bytes(&node)?;
            let hash = polar_cas::ObjectId::for_bytes(&bytes).hex();
            let base = format!("copy-{}", &hash[..48]);
            let mut key = base.clone();
            let mut suffix = 0usize;
            while value["nodes"].get(&key).is_some_and(|existing| existing != &node) {
                suffix += 1;
                if suffix > limits.max_nodes { return Err(Error::Limit("binding aliases")); }
                key = format!("{base}-{suffix}");
            }
            value["nodes"][&key] = node;
            target = Reference::Local { node: key };
        }
    }
    let updated = super::parse_idea(&serde_json::to_vec(&value)?, limits)?;
    encode_idea(&updated, limits)
}
