use polar_identity::{NodeIdentity, SignatureBytes, SyncSigner, UnixTimestamp};

use crate::canonical::signable::signed_discovery_signable;
use crate::errors::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDiscovery {
    pub identity: NodeIdentity,
    pub installation_generation: [u8; 16],
    pub timestamp: UnixTimestamp,
    pub intent: String,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDiscovery {
    pub identity: NodeIdentity,
    pub installation_generation: [u8; 16],
    pub timestamp: UnixTimestamp,
    pub intent: String,
}

impl SignedDiscovery {
    pub fn sign(
        identity: NodeIdentity,
        installation_generation: [u8; 16],
        timestamp: UnixTimestamp,
        intent: impl Into<String>,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != identity.node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        validate_generation(installation_generation)?;
        let intent = validate_intent(intent.into())?;
        let signable =
            signed_discovery_signable(&identity, installation_generation, timestamp, &intent);
        Ok(Self {
            identity,
            installation_generation,
            timestamp,
            intent,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        identity: NodeIdentity,
        installation_generation: [u8; 16],
        timestamp: UnixTimestamp,
        intent: impl Into<String>,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != identity.node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        validate_generation(installation_generation)?;
        let intent = validate_intent(intent.into())?;
        let signable =
            signed_discovery_signable(&identity, installation_generation, timestamp, &intent);
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::DiscoveryV1,
            signable,
        )?;
        let sig = request
            .sign_with(
                identity.node_id,
                signer,
                polar_identity::SignOptions::default(),
            )
            .await?;
        Ok(Self {
            identity,
            installation_generation,
            timestamp,
            intent,
            sig,
        })
    }
}

pub fn verify_signed_discovery(
    discovery: &SignedDiscovery,
) -> Result<VerifiedDiscovery, ProtocolError> {
    validate_generation(discovery.installation_generation)?;
    let intent = validate_intent(discovery.intent.clone())?;
    let signable = signed_discovery_signable(
        &discovery.identity,
        discovery.installation_generation,
        discovery.timestamp,
        &intent,
    );
    super::canonical::signable::verify_signature(
        &discovery.identity.node_id,
        &signable,
        &discovery.sig,
    )?;
    Ok(VerifiedDiscovery {
        identity: discovery.identity.clone(),
        installation_generation: discovery.installation_generation,
        timestamp: discovery.timestamp,
        intent,
    })
}

fn validate_generation(generation: [u8; 16]) -> Result<(), ProtocolError> {
    if generation == [0; 16] {
        return Err(ProtocolError::InvalidGeneration);
    }
    Ok(())
}

fn validate_intent(intent: String) -> Result<String, ProtocolError> {
    let trimmed = intent.trim();
    if trimmed == "announce" {
        Ok(trimmed.to_string())
    } else {
        Err(ProtocolError::InvalidIntent)
    }
}




