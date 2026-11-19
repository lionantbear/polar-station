//! Portable Host references and import review contracts. No credentials or live handles.
use crate::{HostRefV1, MachineConnectionV1, SshRouteProfileV1};
use serde::{Deserialize, Serialize};
mod record;
pub use record::PortableHostV1;

pub const HOST_EXPORT_SCHEMA_V1: &str = "polar-hosts/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSystemIdentityV1 {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionUserV1 {
    pub name: String,
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServerKeyV1 {
    pub algorithm: String,
    pub fingerprint: String,
}

/// Exported observations are claims; deserialization never grants local verification.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostIdentityObservationV1 {
    pub system: Option<HostSystemIdentityV1>,
    pub user: Option<HostExecutionUserV1>,
    pub server_keys: Vec<HostServerKeyV1>,
    pub hostname: Option<String>,
    pub platform: Option<String>,
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableHostConnectionV1 {
    /// Origin-local never means the importing machine's local account.
    Local,
    Ssh {
        destination: String,
        route_profile: Option<SshRouteProfileV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExportBundleV1 {
    pub schema: String,
    pub hosts: Vec<PortableHostV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostFieldChangeV1 {
    Equal,
    Added,
    Missing,
    Changed,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFieldComparisonV1 {
    pub field: String,
    pub label: String,
    pub local: Option<String>,
    pub imported: Option<String>,
    pub change: HostFieldChangeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportCandidateV1 {
    pub target_id: String,
    pub display_name: String,
    pub identity_verified: bool,
    pub different_user: bool,
    pub fields: Vec<HostFieldComparisonV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostImportDispositionV1 {
    Identical,
    Review,
    New,
    DuplicateAddress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportRowV1 {
    pub fields: Vec<HostFieldComparisonV1>,
    pub host: PortableHostV1,
    pub disposition: HostImportDispositionV1,
    pub candidates: Vec<HostImportCandidateV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportTargetV1 {
    pub target_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportPlanV1 {
    pub targets: Vec<HostImportTargetV1>,
    pub revision: String,
    pub rows: Vec<HostImportRowV1>,
}

/// Explicit choices, replayed against a freshly computed plan, never a client-supplied plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostImportDecisionV1 {
    Skip {
        source: HostRefV1,
    },
    Automatic {
        source: HostRefV1,
    },
    /// Source-local can only be associated with an explicitly selected configured Host.
    Merge {
        source: HostRefV1,
        target_id: String,
        /// Only selected configuration values are applied; omitted fields remain local.
        fields: Vec<HostImportConfigFieldV1>,
        /// Replacing a route profile can remove addresses, so needs an explicit choice.
        allow_address_removal: bool,
    },
    Create {
        source: HostRefV1,
    },
}
impl HostImportDecisionV1 {
    pub fn source(&self) -> &HostRefV1 {
        match self {
            Self::Skip { source }
            | Self::Automatic { source }
            | Self::Merge { source, .. }
            | Self::Create { source } => source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostImportConfigFieldV1 {
    DisplayName,
    Destination,
    LanAddress,
    LanPort,
    WanHost,
    WanPort,
    DohProviders,
    RouteProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportAssociationV1 {
    pub source: HostRefV1,
    pub target_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostImportResultV1 {
    pub revision: String,
    pub associations: Vec<HostImportAssociationV1>,
    pub created: Vec<String>,
    pub changed: Vec<String>,
    pub skipped: Vec<HostRefV1>,
}

impl From<MachineConnectionV1> for PortableHostConnectionV1 {
    fn from(machine: MachineConnectionV1) -> Self {
        Self::Ssh {
            destination: machine.destination,
            route_profile: machine.route_profile,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostTransferRequestV1 {
    ExportContent { bindings: Vec<crate::PaneContentBindingV1>, #[serde(default)] target_ids: Vec<String> },
    Resolve { sources: Vec<HostRefV1> },
    Export {
        target_ids: Vec<String>,
    },
    Plan {
        bundle: HostExportBundleV1,
    },
    Compare {
        expected_revision: String,
        host: Box<PortableHostV1>,
        target_id: String,
    },
    Apply {
        expected_revision: String,
        bundle: HostExportBundleV1,
        decisions: Vec<HostImportDecisionV1>,
    },
}
impl HostTransferRequestV1 {
    pub(crate) fn validate(&self) -> Result<(), crate::ProtocolValidationError> {
        if serde_json::to_vec(self).map_or(true, |bytes| {
            bytes.len() > crate::MAX_APPLICATION_PAYLOAD_BYTES / 2
        }) {
            return Err(crate::ProtocolValidationError::InvalidControlCenterCommand);
        }
        if let Self::Export { target_ids } = self {
            if target_ids.is_empty() || target_ids.len() > 256 {
                return Err(crate::ProtocolValidationError::InvalidControlCenterCommand);
            }
            for id in target_ids {
                crate::validate_stable_identifier(id, "Host export target")?;
            }
        }
        match self {
            Self::ExportContent { bindings, target_ids } => {
                if bindings.len() > 1152 { return Err(crate::ProtocolValidationError::InvalidControlCenterCommand); }
                for binding in bindings { binding.validate()?; }
                if target_ids.len()>256 { return Err(crate::ProtocolValidationError::InvalidControlCenterCommand); }
                for target in target_ids { crate::validate_stable_identifier(target,"content source Host")?; }
            }
            Self::Resolve { sources } => {
                if sources.len() > 256 { return Err(crate::ProtocolValidationError::InvalidControlCenterCommand); }
                for source in sources { source.validate()?; }
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostTransferResponseV1 {
    ContentExported { bundle: HostExportBundleV1, sources: Vec<crate::TerminalSourceV1> },
    Resolved { revision: String, associations: Vec<HostImportAssociationV1> },
    Exported {
        bundle: HostExportBundleV1,
    },
    Planned {
        plan: HostImportPlanV1,
    },
    Applied {
        result: HostImportResultV1,
    },
    Compared {
        source: HostRefV1,
        revision: String,
        candidate: HostImportCandidateV1,
    },
}
