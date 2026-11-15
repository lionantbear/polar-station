//! The visible Idea draft is the only instruction source. Native editors own
//! carets/IME/undo, and edit this existing per-view draft through typed inputs.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdeaNodeBlock {
    pub node_uuid: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IdeaSelectedNode {
    pub node_uuid: String,
    pub title: String,
}
impl IdeaSelectedNode {
    pub(super) fn from_reference(reference: &polar_ideacave_presentation::controls::NodeReference) -> Option<Self> {
        let node_uuid = uuid::Uuid::parse_str(&reference.node_uuid).ok()?.hyphenated().to_string();
        Some(Self { node_uuid, title: reference.title.clone() })
    }
}

pub(super) fn validate_document(text: &str, blocks: &[IdeaNodeBlock]) -> Result<(), String> {
    if blocks.len() > 64 { return Err("At most 64 node instructions per draft".into()); }
    let mut identities = BTreeSet::new();
    let mut bytes = text.len();
    let mut has_section = !text.is_empty();
    for block in blocks {
        let id = uuid::Uuid::parse_str(&block.node_uuid).map_err(|_| "Invalid node UUID")?;
        if block.node_uuid != id.hyphenated().to_string() || !identities.insert(id) {
            return Err("Node blocks require unique canonical UUIDs".into());
        }
        if block.title.len() > 4096 { return Err("Node title exceeds 4 KiB".into()); }
        bytes = bytes.saturating_add(block.title.len()).saturating_add(block.node_uuid.len()).saturating_add(block.text.len()).saturating_add(if has_section { 9 } else { 7 });
        has_section = true;
    }
    if bytes > 32 * 1024 { return Err("Idea draft exceeds 32 KiB".into()); }
    Ok(())
}

impl Draft {
    pub(super) fn idea_has_content(&self) -> bool {
        !self.text.trim().is_empty() || self.node_blocks.iter().any(|b| !b.text.trim().is_empty())
    }
    pub(super) fn idea_document_text(&self) -> String {
        let mut sections = Vec::with_capacity(self.node_blocks.len() + 1);
        if !self.text.is_empty() { sections.push(self.text.clone()); }
        for block in &self.node_blocks {
            sections.push(format!("{}（{}）\n{}", block.title, block.node_uuid, block.text));
        }
        sections.join("\n\n")
    }
}

impl Engine {
    pub(super) fn idea_node_instruction(&mut self, window: &str, view_id: Option<&str>, node_uuid: String, title: String, text: String, remove: bool) -> Result<(), String> {
        let key = self.idea_key(window)?;
        if let (Some(expected), DraftContext::Idea(actual)) = (view_id, &key.1) {
            if expected != actual { return Err("Idea editor belongs to a different Pane".into()); }
        }
        let node_uuid = uuid::Uuid::parse_str(&node_uuid).map_err(|_| "Invalid node UUID")?.hyphenated().to_string();
        let draft = self.drafts.entry(key).or_default();
        let mut blocks = draft.node_blocks.clone();
        let found = blocks.iter().position(|b| b.node_uuid == node_uuid);
        if remove {
            if let Some(index) = found { blocks.remove(index); }
        } else if let Some(index) = found {
            // Identity and captured title remain stable while the source changes.
            blocks[index].text = text;
        } else if !text.is_empty() {
            blocks.push(IdeaNodeBlock { node_uuid, title, text });
        }
        validate_document(&draft.text, &blocks)?;
        if blocks != draft.node_blocks {
            let revision = draft.revision.checked_add(1).ok_or("draft version exhausted")?;
            draft.node_blocks = blocks;
            draft.revision = revision;
        }
        Ok(())
    }
}
