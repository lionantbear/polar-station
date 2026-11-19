//! Whole-Workspace interchange, used locally by both native clients.
//! This file never executes connections or owns a runtime/native catalog.
use polar_station_protocol::{
    HostExportBundleV1, HostImportAssociationV1, HostRefV1, StationPaneContentV1, TerminalSourceV1,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "polar-workspace/v1";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub mod native;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub schema: String,
    pub source_workspace_id: String,
    pub workspace: Workspace,
    pub hosts: HostExportBundleV1,
}

/// Portable saved structure, not an instruction to a physical renderer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub selected_tab_id: String,
    pub managed_terminal_font_size_points: u8,
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub display_mode: Mode,
    pub main_pane_id: Option<String>,
    pub canonical_pane_ids: Vec<String>,
    pub visual_pane_ids: Vec<String>,
    #[serde(default)]
    pub split_ratios: Vec<f64>,
    pub panes: Vec<Pane>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Monitor,
    Focus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pane {
    pub id: String,
    pub content: Content,
}

pub use polar_station_protocol::PortableContentV1 as Content;

#[derive(Clone, Debug, Serialize)]
pub struct ImportPreview {
    pub name: String,
    pub tab_count: usize,
    pub pane_count: usize,
    pub missing_hosts: Vec<HostRefV1>,
    pub recognized_hosts: Vec<HostImportAssociationV1>,
    pub unavailable_contents: usize,
    pub warning: Option<String>,
    pub summary: String,
}

fn text(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(format!("Invalid {label}"));
    }
    Ok(())
}
fn id(value: &str) -> Result<(), String> {
    text(value, "identity")?;
    if value.len() > 256 {
        return Err("Identity exceeds 256 bytes".into());
    }
    Ok(())
}

impl Package {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Workspace file exceeds 8 MiB".into());
        }
        let package: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid Workspace file: {e}"))?;
        package.validate()?;
        Ok(package)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA
            || self.hosts.schema != polar_station_protocol::HOST_EXPORT_SCHEMA_V1
        {
            return Err("Unsupported Workspace or Host file version".into());
        }
        id(&self.source_workspace_id)?;
        id(&self.workspace.id)?;
        text(&self.workspace.name, "Workspace name")?;
        if self.workspace.tabs.is_empty()
            || self.workspace.tabs.len() > 128
            || !(10..=32).contains(&self.workspace.managed_terminal_font_size_points)
        {
            return Err("Invalid Workspace structure".into());
        }
        let mut all_ids = BTreeSet::from([self.workspace.id.as_str()]);
        let mut tab_ids = BTreeSet::new();
        let mut required_hosts = BTreeSet::new();
        for tab in &self.workspace.tabs {
            id(&tab.id)?;
            text(&tab.title, "Tab title")?;
            if !all_ids.insert(&tab.id) || !tab_ids.insert(&tab.id) || tab.panes.len() > 9 {
                return Err("Duplicate identity or too many Panes".into());
            }
            let mut panes = BTreeSet::new();
            for pane in &tab.panes {
                id(&pane.id)?;
                if !all_ids.insert(&pane.id) || !panes.insert(&pane.id) {
                    return Err("Duplicate Pane identity".into());
                }
                pane.content.validate().map_err(|e| e.to_string())?;
                if let Some(host) = pane.content.host() {
                    required_hosts.insert(host);
                }
            }
            for order in [&tab.canonical_pane_ids, &tab.visual_pane_ids] {
                if order.len() != panes.len() || order.iter().collect::<BTreeSet<_>>() != panes {
                    return Err("Pane order must include every Pane exactly once".into());
                }
            }
            if tab
                .main_pane_id
                .as_ref()
                .is_some_and(|id| !panes.contains(id))
                || (tab.display_mode == Mode::Focus
                    && !panes.is_empty()
                    && tab.main_pane_id.is_none())
                || tab.split_ratios.len() > tab.panes.len().saturating_sub(1)
                || tab
                    .split_ratios
                    .iter()
                    .any(|v| !v.is_finite() || *v <= 0.0 || *v >= 1.0)
            {
                return Err("Invalid saved layout".into());
            }
        }
        if !tab_ids.contains(&self.workspace.selected_tab_id) {
            return Err("Selected Tab is missing".into());
        }
        let mut hosts = BTreeSet::new();
        for host in &self.hosts.hosts {
            host.source.validate().map_err(|e| e.to_string())?;
            if !hosts.insert(&host.source) {
                return Err("Duplicate source Host".into());
            }
        }
        if !required_hosts.is_subset(&hosts) {
            return Err("Workspace package omits a required source Host".into());
        }
        Ok(())
    }
    pub fn preview(
        &self,
        associations: &[HostImportAssociationV1],
    ) -> Result<ImportPreview, String> {
        self.validate()?;
        let required: BTreeSet<_> = self.workspace.tabs.iter().flat_map(|tab| &tab.panes)
            .filter_map(|pane| pane.content.host()).collect();
        let mut mapped = BTreeMap::new();
        for association in associations.iter().filter(|a| required.contains(&a.source)) {
            if association.target_id.is_empty() { return Err("Empty target Host association".into()); }
            if let Some(previous) = mapped.insert(&association.source, &association.target_id) {
                if previous != &association.target_id { return Err("Conflicting Host associations".into()); }
            }
        }
        let missing_hosts: Vec<_> = required.iter().filter(|source| !mapped.contains_key(*source))
            .map(|source| (*source).clone()).collect();
        let recognized_hosts: Vec<_> = mapped.into_iter().map(|(source,target_id)| HostImportAssociationV1 {
            source: source.clone(), target_id: target_id.clone(),
        }).collect();
        let unavailable_contents = self
            .workspace
            .tabs
            .iter()
            .flat_map(|t| &t.panes)
            .filter(|p| match &p.content {
                Content::Terminal { source } => source.session.is_none(),
                Content::IdeaTree { .. } | Content::ToolSetup { .. } => true,
                _ => false,
            })
            .count();
        let mut summary=format!("{} · {} Tabs · {} Panes\n\nA separate Workspace will be added. Existing Workspaces and running Sessions stay intact.", self.workspace.name,self.workspace.tabs.len(),self.workspace.tabs.iter().map(|t|t.panes.len()).sum::<usize>());
        if missing_hosts.is_empty() {
            summary.push_str(&format!("\n\nAll {} referenced Hosts recognized.", required.len()));
        } else {
            summary.push_str(&format!("\n\n{} of {} referenced Hosts recognized.", recognized_hosts.len(), required.len()));
        }
        for association in &recognized_hosts {
            let title = self.hosts.hosts.iter().find(|h| h.source == association.source)
                .map_or(association.source.target_id.as_str(), |h| h.display_name.as_str());
            summary.push_str(&format!("\n• {} → {}", title, association.target_id));
        }
        if !missing_hosts.is_empty() {
            summary.push_str("\n\nHosts not recognized (association needed):");
            for host in self
                .hosts
                .hosts
                .iter()
                .filter(|h| missing_hosts.contains(&h.source))
            {
                summary.push_str(&format!(
                    "\n• {} ({})",
                    host.display_name, host.source.target_id
                ));
            }
        }
        if unavailable_contents > 0 {
            summary.push_str(&format!(
                "\n\n{unavailable_contents} Panes retain unavailable content references."
            ));
        }
        Ok(ImportPreview { summary, name:self.workspace.name.clone(),tab_count:self.workspace.tabs.len(),
            pane_count:self.workspace.tabs.iter().map(|t|t.panes.len()).sum(),
            warning: (!missing_hosts.is_empty() || unavailable_contents > 0).then(||
                "Some Panes may be unavailable. Their sources and layout will be retained; configure the missing Hosts or sources to use them.".into()),
            missing_hosts, recognized_hosts, unavailable_contents })
    }
    /// Always a new complete definition; never merges into an existing one.
    pub fn new_copy(&self, name: &str) -> Result<Self, String> {
        self.validate()?;
        text(name, "Workspace name")?;
        let mut copy = self.clone();
        let mut ids = BTreeMap::new();
        ids.insert(
            copy.workspace.id.clone(),
            polar_identity::generation::new_uuid_v4().to_string(),
        );
        for tab in &copy.workspace.tabs {
            ids.insert(
                tab.id.clone(),
                polar_identity::generation::new_uuid_v4().to_string(),
            );
            for pane in &tab.panes {
                ids.insert(
                    pane.id.clone(),
                    polar_identity::generation::new_uuid_v4().to_string(),
                );
            }
        }
        copy.workspace.id = ids[&copy.workspace.id].clone();
        copy.workspace.name = name.into();
        copy.workspace.selected_tab_id = ids[&copy.workspace.selected_tab_id].clone();
        for tab in &mut copy.workspace.tabs {
            tab.id = ids[&tab.id].clone();
            tab.main_pane_id = tab.main_pane_id.as_ref().map(|id| ids[id].clone());
            for id in tab
                .canonical_pane_ids
                .iter_mut()
                .chain(&mut tab.visual_pane_ids)
            {
                *id = ids[id].clone();
            }
            for pane in &mut tab.panes {
                pane.id = ids[&pane.id].clone();
            }
        }
        copy.validate()?;
        Ok(copy)
    }
}


