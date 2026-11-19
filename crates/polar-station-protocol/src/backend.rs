//! Read-only local Station backend facts, obtained without a desktop lease.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationBackendCompositionV1 {
    Compact,
    Full,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationBackendAdmissionV1 {
    Available,
    CurrentClient,
    Occupied,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationBackendDescriptorV1 {
    pub backend_selection_version: u16,
    pub host_id: String,
    pub desktop_version: u16,
    pub composition: StationBackendCompositionV1,
    pub execution_uid: u32,
    pub admission: StationBackendAdmissionV1,
    pub host: crate::HostDescriptor,
    #[serde(default)]
    pub source_associations: Vec<crate::HostImportAssociationV1>,
}
