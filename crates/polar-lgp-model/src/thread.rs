use polar_identity::{NodeId, UnixTimestamp};

use crate::TransportPlane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRecord {
    pub owner_id: NodeId,
    pub thread_id: String,
    pub channel_owner_id: NodeId,
    pub channel_id: String,
    pub partner_id: Option<NodeId>,
    /// Default in-house Polar partner for this LGP thread.
    ///
    /// In `system.navigation.v1`, this partner may coordinate the message after-flow. It is not a
    /// fixed single executor: the partner may choose noop, bridge delivery, command/runtime action,
    /// Gear execution, another in-house Polar, or self execution through a NavigationPlan.
    pub partner_inhouse_polar_id: Option<String>,
    pub access_plane: TransportPlane,
    pub tag: Option<String>,
    /// Compatibility storage flag for config-rule navigation.
    ///
    /// New API surfaces expose this through ThreadPartnerMode / navigation policy. This field remains
    /// internal while older SQLite rows are migrated.
    pub partner_navigation_enabled: bool,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}
