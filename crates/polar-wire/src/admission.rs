//! Fixed, bounded admission envelope. This authenticates generation facts;
//! it does not grant access and must not enter the business sequence journal.
use crate::{InstallationGeneration, PolarWireHeader, WireError, WireResult};
use serde::{Deserialize, Serialize};

pub const MAX_ADMISSION_PACKET_BYTES: usize = 32_768;
pub const ADMISSION_HEADER_BYTES: usize = 122;
pub const MAX_ADMISSION_BODY_BYTES: usize = MAX_ADMISSION_PACKET_BYTES - 240;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionBinding {
    pub sender_generation: InstallationGeneration,
    pub target_generation: InstallationGeneration,
    pub admission_id: [u8; 16],
}

pub fn is_admission_body_kind(kind: u16) -> bool {
    matches!(
        kind,
        crate::BODY_KIND_ACCESS_WORKFLOW_REQUEST
            | crate::BODY_KIND_ACCESS_WORKFLOW_DECISION
            | crate::BODY_KIND_ACCESS_WORKFLOW_RECEIPT
            | crate::BODY_KIND_GENERATION_CHANGE_REQUEST
            | crate::BODY_KIND_GENERATION_CHANGE_DECISION
            | crate::BODY_KIND_REAUTHORIZATION_REQUEST
            | crate::BODY_KIND_REAUTHORIZATION_DECISION
            | crate::BODY_KIND_TRANSPORT_QUERY
            | crate::BODY_KIND_TRANSPORT_SNAPSHOT
    )
}

impl PolarWireHeader {
    /// Explicitly construct a generation-bound admission header. No peer
    /// trust can be inferred from a successful construction or valid signature.
    pub fn admission(
        sender: [u8; 32],
        target: [u8; 32],
        binding: AdmissionBinding,
        body_kind_code: u16,
        created_at_secs: u64,
    ) -> WireResult<Self> {
        if sender == target
            || binding.admission_id == [0; 16]
            || !is_admission_body_kind(body_kind_code)
        {
            return Err(WireError::Codec("invalid admission header".into()));
        }
        let association = crate::association_id(
            &sender,
            binding.sender_generation,
            &target,
            binding.target_generation,
        )?;
        let mut header = Self::new_for_access_plane(
            "admission",
            0,
            0,
            hex::encode(sender),
            Some(hex::encode(target)),
            "bootstrap",
            body_kind_code,
            crate::PROTECTION_CLEAR_SIGNED,
            created_at_secs,
        )
        .with_association(association);
        header.version = crate::WIRE_PACKET_DIGEST_VERSION;
        header.admission = Some(binding);
        Ok(header)
    }
}


