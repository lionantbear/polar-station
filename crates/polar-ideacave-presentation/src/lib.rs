//! Shared Idea Tree read models and navigation. Hosts supply domain-verified
//! graphs; this crate performs no I/O, CAS admission, rendering or version selection.
pub mod controls;
pub mod browsing_configuration;
pub mod perspectives;
use polar_ideacave::{
    composition::{IdeaGraph, OccurrencePath},
    NodeRef, Reference, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct DisplayNode {
    pub identity: NodeRef,
    pub title: String,
    pub properties: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DisplayRelation {
    pub owner: NodeRef,
    pub slot: String,
    pub kind: String,
    pub target: Reference,
    pub properties: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize)]
pub struct IdeaTreeReadModel {
    pub nodes: Vec<DisplayNode>,
    pub relations: Vec<DisplayRelation>,
}
impl IdeaTreeReadModel {
    /// Preserve every definition and original relation, including unresolved targets.
    pub fn from_graph(graph: &IdeaGraph) -> Result<Self> {
        let mut nodes = Vec::new();
        let mut relations = Vec::new();
        for (document, tree) in graph.documents() {
            for (key, node) in tree.nodes() {
                let identity = NodeRef {
                    document: document.clone(),
                    node: key.clone(),
                    expected_uuid: node.uuid(),
                };
                let title = node
                    .properties()
                    .get("title")
                    .or_else(|| node.properties().get("text"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(key)
                    .to_owned();
                nodes.push(DisplayNode {
                    identity: identity.clone(),
                    title,
                    properties: node.properties().clone(),
                });
                for (slot, relation) in node.relations() {
                    relations.push(DisplayRelation {
                        owner: identity.clone(),
                        slot: slot.clone(),
                        kind: relation.relation.clone(),
                        target: relation.target.clone(),
                        properties: relation.properties.clone(),
                    });
                }
            }
        }
        Ok(Self { nodes, relations })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub root_index: usize,
    pub path: OccurrencePath,
    pub node: NodeRef,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdeaTreeSavedView {
    pub roots: Vec<NodeRef>,
    pub selection: Option<Selection>,
}
#[derive(Clone, Debug, Serialize)]
pub struct IdeaTreeViewUpdate {
    pub selection: Selection,
    pub locate: bool,
}
pub struct IdeaTreeView {
    state: IdeaTreeSavedView,
    past: Vec<IdeaTreeSavedView>,
    future: Vec<IdeaTreeSavedView>,
}
impl IdeaTreeView {
    pub fn new(graph: &IdeaGraph, roots: Vec<NodeRef>) -> Result<Self> {
        let state = IdeaTreeSavedView {
            roots,
            selection: None,
        };
        validate(graph, &state)?;
        Ok(Self {
            state,
            past: Vec::new(),
            future: Vec::new(),
        })
    }
    pub fn select(
        &mut self,
        graph: &IdeaGraph,
        root_index: usize,
        path: OccurrencePath,
        locate: bool,
    ) -> Result<IdeaTreeViewUpdate> {
        let root = self
            .state
            .roots
            .get(root_index)
            .ok_or(polar_ideacave::Error::IdentityMismatch)?;
        let selection = Selection {
            root_index,
            node: graph.select(root, &path)?,
            path,
        };
        if self.state.selection.as_ref() != Some(&selection) {
            push_bounded(&mut self.past, self.state.clone());
            self.future.clear();
            self.state.selection = Some(selection.clone());
        }
        Ok(IdeaTreeViewUpdate { selection, locate })
    }
    pub fn save(&self) -> IdeaTreeSavedView {
        self.state.clone()
    }
    /// Exact roots must match; the host must explicitly open a different revision.
    pub fn restore(&mut self, graph: &IdeaGraph, state: IdeaTreeSavedView) -> Result<()> {
        if state.roots != self.state.roots {
            return Err(polar_ideacave::Error::IdentityMismatch);
        }
        validate(graph, &state)?;
        self.state = state;
        self.past.clear();
        self.future.clear();
        Ok(())
    }
    pub fn travel(
        &mut self,
        graph: &IdeaGraph,
        forward: bool,
    ) -> Result<Option<IdeaTreeViewUpdate>> {
        let (from, to) = if forward {
            (&mut self.future, &mut self.past)
        } else {
            (&mut self.past, &mut self.future)
        };
        let Some(candidate) = from.last() else {
            return Ok(None);
        };
        validate(graph, candidate)?;
        let candidate = from.pop().expect("candidate checked");
        push_bounded(to, std::mem::replace(&mut self.state, candidate));
        Ok(self
            .state
            .selection
            .clone()
            .map(|selection| IdeaTreeViewUpdate {
                selection,
                locate: true,
            }))
    }
}
fn push_bounded(history: &mut Vec<IdeaTreeSavedView>, state: IdeaTreeSavedView) {
    if history.len() == 100 {
        history.remove(0);
    }
    history.push(state);
}
fn validate(graph: &IdeaGraph, state: &IdeaTreeSavedView) -> Result<()> {
    if state.roots.is_empty() || state.roots.len() > graph.limits.max_documents {
        return Err(polar_ideacave::Error::Limit("view roots"));
    }
    for root in &state.roots {
        graph.exact(root)?;
    }
    if let Some(selection) = &state.selection {
        let root = state
            .roots
            .get(selection.root_index)
            .ok_or(polar_ideacave::Error::IdentityMismatch)?;
        if graph.select(root, &selection.path)? != selection.node {
            return Err(polar_ideacave::Error::IdentityMismatch);
        }
    }
    Ok(())
}
