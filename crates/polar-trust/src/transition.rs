use polar_identity::{NodeId, UnixTimestamp};

use crate::decision::TrustDecisionKind;
use crate::errors::TrustError;
use crate::peer_state::PeerState;
use crate::record::TrustRecord;
use crate::states::{InboundTrustState, OutboundTrustState};
use crate::verified::{VerifiedTrustDecision, VerifiedTrustRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTransition {
    ObserveCandidate,
    ReceiveInboundRequest,
    SendOutboundRequest,
    ObserveIdentityDrift,
    ApproveAccess,
    RejectAccess,
    RevokeAccess,
    RestoreAccess,
    RateLimitPeer,
    QuarantinePeer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionResult {
    pub new_record: TrustRecord,
    pub new_peer_state: PeerState,
}

pub fn apply_transition(
    current: Option<&TrustRecord>,
    transition: TrustTransition,
    now: UnixTimestamp,
    verified_request: Option<&VerifiedTrustRequest>,
    verified_decision: Option<&VerifiedTrustDecision>,
) -> Result<TransitionResult, TrustError> {
    let mut record = match current {
        Some(record) => record.clone(),
        None => {
            build_record_from_request(verified_request.ok_or(TrustError::InvalidStateTransition)?)?
        }
    };

    match transition {
        TrustTransition::ObserveCandidate => {}
        TrustTransition::ReceiveInboundRequest => {
            let req = verified_request.ok_or(TrustError::InvalidStateTransition)?;
            ensure_same_peer(record.peer_node_id, req.sender_node_id)?;
            ensure_same_binding(record.peer_binding_pubkey, req.sender_binding_pubkey)?;
            record.inbound = InboundTrustState::Pending;
        }
        TrustTransition::SendOutboundRequest => {
            record.outbound = OutboundTrustState::Pending;
        }
        TrustTransition::ObserveIdentityDrift => {
            record.identity_change_pending = true;
        }
        TrustTransition::ApproveAccess => {
            record.outbound = OutboundTrustState::Trusted;
            record.inbound = InboundTrustState::Trusted;
            record.identity_change_pending = false;
            record.approved_at = Some(now);
        }
        TrustTransition::RejectAccess => {
            record.inbound = InboundTrustState::None;
        }
        TrustTransition::RevokeAccess => {
            record.outbound = OutboundTrustState::Revoked;
            record.identity_change_pending = false;
            record.revoked_at = Some(now);
        }
        TrustTransition::RestoreAccess => {
            record.outbound = OutboundTrustState::Trusted;
            record.identity_change_pending = false;
            record.revoked_at = None;
        }
        TrustTransition::RateLimitPeer => {}
        TrustTransition::QuarantinePeer => {}
    }

    if let Some(decision) = verified_decision {
        ensure_same_peer(record.peer_node_id, decision.from_node_id)?;
        match decision.kind {
            TrustDecisionKind::Approve | TrustDecisionKind::Restore => {
                record.inbound = InboundTrustState::Trusted;
                if record.outbound == OutboundTrustState::Pending {
                    record.outbound = OutboundTrustState::Trusted;
                }
            }
            TrustDecisionKind::Reject => {
                record.inbound = InboundTrustState::None;
                if record.outbound == OutboundTrustState::Pending {
                    record.outbound = OutboundTrustState::None;
                }
            }
            TrustDecisionKind::Revoke => {
                record.inbound = InboundTrustState::Revoked;
            }
        }
    }

    let new_peer_state = match transition {
        TrustTransition::ObserveIdentityDrift => PeerState::IdentityChangePending,
        TrustTransition::RateLimitPeer => PeerState::RateLimited,
        TrustTransition::QuarantinePeer => PeerState::Quarantined,
        _ => record.peer_state(),
    };

    Ok(TransitionResult {
        new_record: record,
        new_peer_state,
    })
}

fn build_record_from_request(req: &VerifiedTrustRequest) -> Result<TrustRecord, TrustError> {
    Ok(TrustRecord {
        peer_node_id: req.sender_node_id,
        peer_node_name: req.sender_node_name.clone(),
        peer_binding_pubkey: req.sender_binding_pubkey,
        outbound: OutboundTrustState::None,
        inbound: InboundTrustState::Pending,
        identity_change_pending: false,
        approved_at: None,
        revoked_at: None,
    })
}

fn ensure_same_peer(expected: NodeId, got: NodeId) -> Result<(), TrustError> {
    if expected == got {
        Ok(())
    } else {
        Err(TrustError::IdentityMismatch)
    }
}

fn ensure_same_binding(
    expected: polar_identity::BindingPublicKey,
    got: polar_identity::BindingPublicKey,
) -> Result<(), TrustError> {
    if expected == got {
        Ok(())
    } else {
        Err(TrustError::BindingMismatch)
    }
}


