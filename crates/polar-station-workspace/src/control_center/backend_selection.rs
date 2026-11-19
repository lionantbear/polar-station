//! Shared copy and enablement for the process-wide local backend page.
use polar_station_protocol::{
    StationBackendAdmissionV1, StationBackendCompositionV1, StationBackendDescriptorV1,
};
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
pub struct Input {
    #[serde(default)]
    pub references: Vec<Reference>,
    pub current: String,
    pub target: String,
    pub preview: Option<StationBackendDescriptorV1>,
    #[serde(default)]
    pub workspace_count: usize,
    #[serde(default)]
    pub pane_count: usize,
    #[serde(default)]
    pub busy: bool,
    pub message: Option<String>,
}
#[derive(Deserialize)]
pub struct Reference {
    pub source: polar_station_protocol::HostRefV1,
    pub title: String,
}
#[derive(Serialize)]
pub struct Row {
    label: &'static str,
    value: String,
}
#[derive(Serialize)]
pub struct View {
    title: &'static str,
    description: &'static str,
    pub can_switch: bool,
    inspect_label: &'static str,
    switch_label: &'static str,
    endpoint_label: &'static str,
    fields: Vec<Row>,
    impacts: Vec<Row>,
    message: String,
}
pub fn project(input: Input) -> View {
    let mut fields = vec![
        row("Current backend", &input.current),
        row("Target socket", &input.target),
    ];
    if let Some(p) = &input.preview {
        for reference in &input.references {
            if !p.source_associations.iter().any(|a| a.source == reference.source) {
                fields.push(row("Missing Host association", &reference.title));
            }
        }
        fields.extend([
            row("Host", &format!("{} · {}", p.host.host_name, p.host_id)),
            row(
                "Mode",
                match p.composition {
                    StationBackendCompositionV1::Compact => "Compact",
                    StationBackendCompositionV1::Full => "Full Polar",
                },
            ),
            row("Execution user", &format!("Local UID {}", p.execution_uid)),
            row(
                "Desktop admission",
                match p.admission {
                    StationBackendAdmissionV1::Available => "Available",
                    StationBackendAdmissionV1::CurrentClient => "This client",
                    StationBackendAdmissionV1::Occupied => "Another client is connected",
                },
            ),
        ]);
    }
    View {title:"Station Backend",description:"Choose one local backend for every Workspace in this client.",
        can_switch:!input.busy && input.current!=input.target && input.preview.as_ref().is_some_and(|p|p.admission!=StationBackendAdmissionV1::Occupied),
        inspect_label:"Inspect",switch_label:"Switch backend",endpoint_label:"Local socket",fields,
        impacts:vec![row("Workspaces and layout",&format!("{} Workspaces · {} Panes retained on this client",input.workspace_count,input.pane_count)),
            row("Drafts","Keep original targets; input is never replayed"),row("Appearance and input preferences","Keep this client's preferences"),
            row("Execution Hosts and Session access","Use target configuration. Original source references are retained; missing associations need Host configuration"),
            row("Monitoring, resources and voice","Use target configuration; service configuration differences are not yet verified"),
            row("History, Columbus and credentials","Stay with their original backend; not copied"),
            row("Running Sessions and files","Stay in their execution environment; not moved or stopped")],
        message:input.message.unwrap_or_default()}
}
fn row(label: &'static str, value: &str) -> Row {
    Row {
        label,
        value: value.into(),
    }
}
