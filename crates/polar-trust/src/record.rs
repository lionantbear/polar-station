use polar_identity::{BindingPublicKey, NodeId, NodeName, UnixTimestamp};

use crate::eligibility::MemberEligibility;
use crate::peer_state::PeerState;
use crate::states::{InboundTrustState, OutboundTrustState};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrustRecord {
    #[serde(with = "crate::serde_value::node_id")]
    pub peer_node_id: NodeId,
    #[serde(with = "crate::serde_value::node_name")]
    pub peer_node_name: NodeName,
    #[serde(with = "crate::serde_value::binding_key")]
    pub peer_binding_pubkey: BindingPublicKey,
    pub outbound: OutboundTrustState,
    pub inbound: InboundTrustState,
    pub identity_change_pending: bool,
    #[serde(with = "crate::serde_value::optional_timestamp")]
    pub approved_at: Option<UnixTimestamp>,
    #[serde(with = "crate::serde_value::optional_timestamp")]
    pub revoked_at: Option<UnixTimestamp>,
}

impl TrustRecord {
    pub fn peer_state(&self) -> PeerState {
        if self.identity_change_pending {
            return PeerState::IdentityChangePending;
        }
        if self.outbound == OutboundTrustState::Revoked
            || self.inbound == InboundTrustState::Revoked
        {
            return PeerState::Revoked;
        }
        match (self.outbound, self.inbound) {
            (OutboundTrustState::Trusted, InboundTrustState::Trusted) => PeerState::MutualTrusted,
            (OutboundTrustState::Trusted, _) | (_, InboundTrustState::Trusted) => {
                PeerState::TrustedOneWay
            }
            (OutboundTrustState::Pending, _) => PeerState::PendingOutbound,
            (_, InboundTrustState::Pending) => PeerState::PendingInbound,
            _ => PeerState::Discovered,
        }
    }

    pub fn member_eligibility(&self) -> MemberEligibility {
        let state = self.peer_state();
        MemberEligibility {
            allows_member_plane: matches!(
                state,
                PeerState::TrustedOneWay | PeerState::MutualTrusted
            ),
            allows_inbound_member_plane: !self.identity_change_pending
                && matches!(self.outbound, OutboundTrustState::Trusted),
            is_mutual: matches!(state, PeerState::MutualTrusted),
        }
    }
}
