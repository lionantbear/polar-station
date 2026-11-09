use polar_identity::{NodeId, UnixTimestamp};

/// Scope of management authority granted to a peer node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ControlScope {
    /// Peer may read node state and metrics but cannot mutate configuration.
    ReadOnly,
    /// Peer may issue any management command, including config mutations.
    Full,
}

/// Records that a peer node has been granted management authority over this
/// node.
///
/// This is the third independent admission layer:
/// - TLS layer: `PeerCaCert` in trusted peer CA set
/// - Application trust layer: `TrustRecord` with `InboundTrustState::Approved`
/// - Management authority layer: `ControlAuthorityRecord` with `ControlScope`
///
/// Presence of a `TrustRecord` does NOT imply `ControlAuthorityRecord` and
/// vice versa.  A peer may be trusted for LGP message exchange without having
/// management authority, and management authority is always explicitly granted.
///
/// In the current control-bus rollout, this managed-side record is one half of
/// the local "control-bus-ready peer pair" state. Transport admission currently
/// requires this record together with `ManagedNodeRecord`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ControlAuthorityRecord {
    /// The peer node that has been granted authority.
    #[serde(with = "crate::serde_value::node_id")]
    pub peer_node_id: NodeId,
    /// When authority was granted.
    #[serde(with = "crate::serde_value::timestamp")]
    pub granted_at: UnixTimestamp,
    /// The node that granted the authority.  Currently always the local node
    /// (self-grant only; delegation is reserved for a future phase).
    #[serde(with = "crate::serde_value::node_id")]
    pub granted_by: NodeId,
    /// The scope of the granted authority.
    pub scope: ControlScope,
}

/// Records that a peer node is explicitly managed by this node.
///
/// This is the manager-side complement to `ControlAuthorityRecord`.
/// If `polar A` manages `polar B`, then `polar A` stores a
/// `ManagedNodeRecord` for `polar B`, while `polar B` stores a
/// `ControlAuthorityRecord` for `polar A`.
///
/// The current transport admission path requires this record together with
/// `ControlAuthorityRecord` for the same peer, so the two records collectively
/// represent the local control-bus-ready state for that peer pair.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagedNodeRecord {
    /// The managed peer node.
    #[serde(with = "crate::serde_value::node_id")]
    pub peer_node_id: NodeId,
    /// When this node recorded the peer as managed.
    #[serde(with = "crate::serde_value::timestamp")]
    pub recorded_at: UnixTimestamp,
    /// The node that recorded the relationship. Currently always the local
    /// node; remote delegation is reserved for a future phase.
    #[serde(with = "crate::serde_value::node_id")]
    pub recorded_by: NodeId,
}

impl ControlAuthorityRecord {
    /// Returns `true` if this record authorises full management commands.
    pub fn allows_full_control(&self) -> bool {
        matches!(self.scope, ControlScope::Full)
    }

    /// Returns `true` if this record authorises at least read-only access.
    pub fn allows_read(&self) -> bool {
        true // both scopes allow reading
    }
}


