//! Shared ancestry/search projection. Native callers supply flat reference ranks.
use polar_tmux_protocol::{AgentSessionCatalogRowV1, SessionTargetGroupV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Deserialize)]
pub struct AgentCatalogInput {
    pub agents: Vec<AgentSessionCatalogRowV1>,
    pub groups: Vec<SessionTargetGroupV1>,
    pub ranks: Vec<Option<usize>>,
    pub query: String,
    pub filter: String,
}
#[derive(Debug, Serialize)]
pub struct AgentCatalogRow {
    pub root: usize,
    pub parent: Option<usize>,
    pub key: String,
    pub group: String,
    pub matches: bool,
    pub rank: Option<usize>,
}
pub fn project_agent_catalog(input: AgentCatalogInput) -> Vec<AgentCatalogRow> {
    let agents = &input.agents;
    let index: BTreeMap<_, _> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            (
                (
                    a.owner_host_id.as_str(),
                    a.endpoint_id.as_str(),
                    a.thread_id.as_str(),
                ),
                i,
            )
        })
        .collect();
    let machines: BTreeMap<_, _> = input
        .groups
        .iter()
        .map(|g| (g.id.as_str(), g.title.as_str()))
        .collect();
    let query = input.query.trim().to_lowercase();
    let mut rows: Vec<_> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let parent = a
                .metadata
                .parent_thread_id
                .as_deref()
                .and_then(|p| {
                    index
                        .get(&(a.owner_host_id.as_str(), a.endpoint_id.as_str(), p))
                        .copied()
                })
                .filter(|&p| p != i && agents[p].execution_target_id == a.execution_target_id);
            let text = format!(
                "{} {} {} {}",
                a.display_name,
                a.metadata.preview.as_deref().unwrap_or(""),
                a.metadata.cwd.as_deref().unwrap_or(""),
                machines
                    .get(a.execution_target_id.as_str())
                    .copied()
                    .unwrap_or(&a.execution_target_id)
            );
            AgentCatalogRow {
                root: i,
                parent,
                key: format!("{}/{}/{}", a.owner_host_id, a.endpoint_id, a.thread_id),
                group: format!(
                    "{} / {}",
                    a.execution_target_id,
                    a.metadata.cwd.as_deref().unwrap_or("Unknown project")
                ),
                matches: matches!(input.filter.as_str(), "all" | "codex")
                    && text.to_lowercase().contains(&query),
                rank: input.ranks.get(i).copied().flatten(),
            }
        })
        .collect();
    // Visit each vertex once; break cycles at the smallest full identity.
    let mut done = vec![false; rows.len()];
    let mut positions: Vec<Option<usize>> = vec![None; rows.len()];
    for i in 0..rows.len() {
        if done[i] {
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut cursor = i;
        let root = loop {
            if done[cursor] {
                break rows[cursor].root;
            }
            if let Some(start) = positions[cursor] {
                let root = *path[start..]
                    .iter()
                    .min_by_key(|&&j| {
                        (
                            &agents[j].owner_host_id,
                            &agents[j].endpoint_id,
                            &agents[j].thread_id,
                        )
                    })
                    .expect("nonempty cycle");
                rows[root].parent = None;
                break root;
            }
            positions[cursor] = Some(path.len());
            path.push(cursor);
            match rows[cursor].parent {
                Some(parent) => cursor = parent,
                None => break cursor,
            }
        };
        for j in path {
            rows[j].root = root;
            done[j] = true;
            positions[j] = None;
        }
    }
    // Bottom-up propagation is linear even for a long single-child chain.
    let mut children = vec![0usize; rows.len()];
    for row in &rows {
        if let Some(p) = row.parent {
            children[p] += 1;
        }
    }
    let mut leaves: Vec<_> = children
        .iter()
        .enumerate()
        .filter_map(|(i, &n)| (n == 0).then_some(i))
        .collect();
    while let Some(i) = leaves.pop() {
        if let Some(p) = rows[i].parent {
            rows[p].matches |= rows[i].matches;
            rows[p].rank = rows[p].rank.into_iter().chain(rows[i].rank).min();
            children[p] -= 1;
            if children[p] == 0 {
                leaves.push(p);
            }
        }
    }
    for i in 0..rows.len() {
        rows[i].group = rows[rows[i].root].group.clone();
    }
    rows
}

