//! Public, non-secret installation/association identifiers. These identify a
//! ledger incarnation; possession never grants Access Plane authorization.
use crate::{sha256_bytes, WireError, WireResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "[u8; 16]", into = "[u8; 16]")]
pub struct InstallationGeneration([u8; 16]);

impl InstallationGeneration {
    pub fn from_bytes(bytes: [u8; 16]) -> WireResult<Self> {
        if bytes == [0; 16] {
            return Err(WireError::Codec("zero installation generation".into()));
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl TryFrom<[u8; 16]> for InstallationGeneration {
    type Error = WireError;
    fn try_from(value: [u8; 16]) -> WireResult<Self> {
        Self::from_bytes(value)
    }
}

impl From<InstallationGeneration> for [u8; 16] {
    fn from(value: InstallationGeneration) -> Self {
        value.0
    }
}

pub type AssociationId = [u8; 32];

pub fn association_id(
    local_node: &[u8; 32],
    local_generation: InstallationGeneration,
    peer_node: &[u8; 32],
    peer_generation: InstallationGeneration,
) -> WireResult<AssociationId> {
    if local_node == peer_node {
        return Err(WireError::Codec(
            "association requires distinct identities".into(),
        ));
    }
    let (a, ag, b, bg) = if local_node < peer_node {
        (local_node, local_generation, peer_node, peer_generation)
    } else {
        (peer_node, peer_generation, local_node, local_generation)
    };
    let mut input = [0u8; 104];
    input[..8].copy_from_slice(b"PGENPAIR");
    input[8..40].copy_from_slice(a);
    input[40..56].copy_from_slice(ag.as_bytes());
    input[56..88].copy_from_slice(b);
    input[88..104].copy_from_slice(bg.as_bytes());
    Ok(sha256_bytes(&input))
}


