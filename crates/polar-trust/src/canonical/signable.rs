use polar_identity::{BindingPublicKey, NodeId, NodeName, UnixTimestamp};
use polar_transport::AccessPlaneEndpointHints;

fn hash_node_name(name: &NodeName) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(name.as_str().as_bytes());
    digest.into()
}

fn hash_str(value: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(value.as_bytes());
    digest.into()
}

pub(crate) fn trust_request_signable(parts: TrustRequestSignableParts<'_>) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut out = Vec::with_capacity(360);
    out.extend_from_slice(parts.sender_node_id.as_bytes());
    out.extend_from_slice(&hash_node_name(parts.sender_node_name));
    out.extend_from_slice(parts.sender_binding_pubkey.as_bytes());
    out.extend_from_slice(parts.receiver_node_id.as_bytes());
    out.extend_from_slice(&hash_str(parts.sender_plane));
    out.extend_from_slice(&hash_str(parts.receiver_plane));
    out.extend_from_slice(&parts.timestamp.as_secs().to_be_bytes());
    if let Some(peer_ca_der) = parts.peer_ca_der {
        let digest = Sha256::digest(peer_ca_der);
        out.extend_from_slice(&digest);
    } else {
        out.extend_from_slice(&[0u8; 32]);
    }
    if let Some(peer_session_pubkey) = parts.peer_session_pubkey {
        out.extend_from_slice(peer_session_pubkey);
    } else {
        out.extend_from_slice(&[0u8; 32]);
    }
    out.extend_from_slice(&hash_plane_endpoints(parts.plane_endpoints));
    out.extend_from_slice(&Sha256::digest(parts.request_id.as_bytes()));
    if let Some(admission_token) = parts.admission_token {
        out.extend_from_slice(&Sha256::digest(admission_token.as_bytes()));
    } else {
        out.extend_from_slice(&[0u8; 32]);
    }
    out
}

pub(crate) struct TrustRequestSignableParts<'a> {
    pub request_id: &'a str,
    pub sender_node_id: &'a NodeId,
    pub sender_node_name: &'a NodeName,
    pub sender_binding_pubkey: &'a BindingPublicKey,
    pub receiver_node_id: &'a NodeId,
    pub sender_plane: &'a str,
    pub receiver_plane: &'a str,
    pub timestamp: UnixTimestamp,
    pub peer_ca_der: Option<&'a [u8]>,
    pub peer_session_pubkey: Option<&'a [u8; 32]>,
    pub plane_endpoints: &'a AccessPlaneEndpointHints,
    pub admission_token: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn trust_decision_signable_with_plane_endpoints(
    from_node_id: &NodeId,
    to_node_id: &NodeId,
    sender_node_name: &NodeName,
    sender_binding_pubkey: &BindingPublicKey,
    sender_plane: &str,
    receiver_plane: &str,
    kind: crate::decision::TrustDecisionKind,
    timestamp: UnixTimestamp,
    peer_ca_der: Option<&[u8]>,
    peer_session_pubkey: Option<&[u8; 32]>,
    plane_endpoints: &AccessPlaneEndpointHints,
) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut out = Vec::with_capacity(265);
    out.extend_from_slice(from_node_id.as_bytes());
    out.extend_from_slice(to_node_id.as_bytes());
    out.extend_from_slice(&hash_node_name(sender_node_name));
    out.extend_from_slice(sender_binding_pubkey.as_bytes());
    out.extend_from_slice(&hash_str(sender_plane));
    out.extend_from_slice(&hash_str(receiver_plane));
    out.push(match kind {
        crate::decision::TrustDecisionKind::Approve => 1,
        crate::decision::TrustDecisionKind::Reject => 2,
        crate::decision::TrustDecisionKind::Revoke => 3,
        crate::decision::TrustDecisionKind::Restore => 4,
    });
    out.extend_from_slice(&timestamp.as_secs().to_be_bytes());
    if let Some(peer_ca_der) = peer_ca_der {
        let digest = Sha256::digest(peer_ca_der);
        out.extend_from_slice(&digest);
    } else {
        out.extend_from_slice(&[0u8; 32]);
    }
    if let Some(peer_session_pubkey) = peer_session_pubkey {
        out.extend_from_slice(peer_session_pubkey);
    } else {
        out.extend_from_slice(&[0u8; 32]);
    }
    out.extend_from_slice(&hash_plane_endpoints(plane_endpoints));
    out
}

fn hash_plane_endpoints(plane_endpoints: &AccessPlaneEndpointHints) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut canonical = String::new();
    for (plane_id, endpoint_sets) in plane_endpoints {
        canonical.push_str(plane_id);
        canonical.push('\n');
        let mut sorted_sets = endpoint_sets.clone();
        for endpoint_set in &mut sorted_sets {
            endpoint_set.endpoints.sort();
            endpoint_set.endpoints.dedup();
        }
        sorted_sets.sort_by(|left, right| {
            left.backend
                .cmp(&right.backend)
                .then(left.protocol.cmp(&right.protocol))
                .then(left.auth.cmp(&right.auth))
                .then(left.endpoints.cmp(&right.endpoints))
        });
        for endpoint_set in sorted_sets {
            canonical.push_str(&endpoint_set.backend);
            canonical.push('\n');
            canonical.push_str(&endpoint_set.protocol);
            canonical.push('\n');
            canonical.push_str(&endpoint_set.auth);
            canonical.push('\n');
            for endpoint in endpoint_set.endpoints {
                canonical.push_str(&endpoint);
                canonical.push('\n');
            }
            canonical.push('\n');
        }
        canonical.push('\n');
    }
    Sha256::digest(canonical.as_bytes()).into()
}
