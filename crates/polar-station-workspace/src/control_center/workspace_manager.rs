//! Local Settings projection over borrowed native catalog metadata.
//! No layout, persistence, execution or independent catalog lives here.
use super::{ControlActionProjection, ControlActionRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub archived: bool,
    pub open: bool,
    pub tab_count: usize,
    pub pane_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct Input {
    pub workspaces: Vec<Item>,
    pub current: Option<String>,
    pub selected: Option<String>,
    #[serde(default)]
    pub archived_expanded: bool,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub transfer_available: bool,
    #[serde(default)]
    pub required_hosts: Vec<String>,
    pub machines: Option<Vec<polar_station_protocol::MachineConnectionV1>>,
}

#[derive(Debug, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub item: Item,
    pub summary: String,
    pub status: &'static str,
    pub selected: bool,
}

#[derive(Debug, Serialize)]
pub struct HostReference {
    pub id: String,
    pub name: String,
    pub access: String,
    pub pane_count: usize,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct View {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub archived_label: String,
    pub rows: Vec<Row>,
    pub selected_id: Option<String>,
    pub actions: Vec<ControlActionProjection>,
    pub page_actions: Vec<ControlActionProjection>,
    pub hosts: Vec<HostReference>,
    pub details_title: &'static str,
    pub preview_caption: &'static str,
    pub hosts_title: &'static str,
    pub empty_message: &'static str,
    pub error: Option<&'static str>,
}

pub fn project(input: Input) -> View {
    let invalid = input.workspaces.iter().any(|w| w.id.is_empty())
        || input
            .workspaces
            .iter()
            .map(|w| &w.id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != input.workspaces.len();
    let selected = input
        .selected
        .as_ref()
        .and_then(|id| input.workspaces.iter().find(|w| &w.id == id))
        .or_else(|| {
            input
                .current
                .as_ref()
                .and_then(|id| input.workspaces.iter().find(|w| &w.id == id && !w.archived))
        })
        .or_else(|| input.workspaces.iter().find(|w| !w.archived));
    let active_count = input.workspaces.iter().filter(|w| !w.archived).count();
    let mut actions = Vec::new();
    if let Some(item) = selected {
        if item.archived {
            actions.push(action("restore", "Restore"));
        } else {
            actions.push(action(
                "open",
                if item.open {
                    "Focus existing window"
                } else {
                    "Open"
                },
            ));
        }
        actions.push(action("rename", "Rename"));
        let mut export = action("export", "Export");
        if !input.transfer_available {
            disable(&mut export, "Workspace export is unavailable.");
        }
        actions.push(export);
        if !item.archived {
            let mut archive = action("archive", "Archive").with_confirmation(
                item.id.clone(), format!("Archive {}?", item.name),
                "Keep this entire Workspace, including all Tabs, Panes and saved layout. Running Sessions stay running.", "Archive");
            if active_count <= 1 {
                disable(&mut archive, "Keep at least one unarchived Workspace.");
            }
            actions.push(archive);
        }
    }
    let mut import = action("import", "Import…");
    if !input.transfer_available {
        disable(&mut import, "Workspace import is unavailable.");
    }
    let mut page_actions = vec![import, action("new", "New Workspace")];
    if input.pending || invalid {
        for a in actions.iter_mut().chain(page_actions.iter_mut()) {
            disable(
                a,
                if invalid {
                    "Cannot read the Workspace catalog."
                } else {
                    "Saving Workspace…"
                },
            );
        }
    }
    let selected_id = selected.map(|w| w.id.clone());
    let rows = input
        .workspaces
        .iter()
        .filter(|w| !w.archived)
        .chain(
            input
                .workspaces
                .iter()
                .filter(|w| w.archived && input.archived_expanded),
        )
        .map(|w| Row {
            item: w.clone(),
            summary: format!(
                "{} · {}",
                count_label(w.tab_count, "Tab"),
                count_label(w.pane_count, "Pane")
            ),
            status: if w.archived {
                "Archived"
            } else if w.open {
                "Open"
            } else {
                "Saved"
            },
            selected: selected_id.as_ref() == Some(&w.id),
        })
        .collect();
    View {
        title: "Workspaces",
        subtitle: "Browse saved Tabs and Panes without opening them.",
        archived_label: format!("Archived ({})", input.workspaces.len() - active_count),
        rows,
        selected_id,
        actions,
        page_actions,
        details_title: "Tabs & saved layout",
        preview_caption: "Preview · no live output",
        hosts_title: "Host references",
        hosts: host_references(&input.required_hosts, input.machines.as_deref()),
        empty_message: "No saved Workspaces.",
        error: invalid.then_some("Cannot read the Workspace catalog."),
    }
}

fn count_label(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

fn host_references(
    required: &[String],
    machines: Option<&[polar_station_protocol::MachineConnectionV1]>,
) -> Vec<HostReference> {
    let mut rows: Vec<HostReference> = vec![];
    for id in required.iter().filter(|id| !id.is_empty()) {
        if let Some(row) = rows.iter_mut().find(|row| &row.id == id) {
            row.pane_count += 1;
            continue;
        }
        let machine = machines.and_then(|items| items.iter().find(|item| &item.id == id));
        rows.push(HostReference {
            id: id.clone(),
            name: machine.map(|m| m.display_name.clone()).unwrap_or_else(|| {
                if id == "local" {
                    "Local".into()
                } else {
                    id.clone()
                }
            }),
            access: machine.map(|m| m.destination.clone()).unwrap_or_default(),
            pane_count: 1,
            status: if id == "local" {
                "Local to current backend"
            } else if machine.is_some() {
                "Configured · not checked live"
            } else if machines.is_some() {
                "Needs Host configuration"
            } else {
                "Host catalog unavailable"
            },
        });
    }
    rows
}

fn action(id: &str, label: &str) -> ControlActionProjection {
    ControlActionProjection::enabled(
        format!("workspace.manage.{id}"),
        label,
        ControlActionRole::Secondary,
    )
}
fn disable(action: &mut ControlActionProjection, reason: &str) {
    action.enabled = false;
    action.disabled_reason = Some(reason.into());
}


