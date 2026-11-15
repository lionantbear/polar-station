use polar_identity::{NodeId, SignatureBytes, SyncSigner, UnixTimestamp};

use crate::canonical::signable::{
    capability_distribution_signable, CapabilityDistributionSignable,
};
use crate::errors::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCapabilityDistribution {
    pub sender_node_id: NodeId,
    pub target_node_id: NodeId,
    pub capability_name: String,
    pub version: Option<String>,
    pub timestamp: UnixTimestamp,
    pub force_install: bool,
    pub force_auth_overwrite: bool,
    pub auth_sha256: String,
    pub sealed_auth_ephemeral_pubkey_hex: String,
    pub sealed_auth_nonce_hex: String,
    pub sealed_auth_ciphertext_hex: String,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCapabilityDistribution {
    pub sender_node_id: NodeId,
    pub target_node_id: NodeId,
    pub capability_name: String,
    pub version: Option<String>,
    pub timestamp: UnixTimestamp,
    pub force_install: bool,
    pub force_auth_overwrite: bool,
    pub auth_sha256: String,
    pub sealed_auth_ephemeral_pubkey_hex: String,
    pub sealed_auth_nonce_hex: String,
    pub sealed_auth_ciphertext_hex: String,
}

pub struct SealedCapabilityAuth {
    pub ephemeral_pubkey_hex: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

pub struct CapabilityDistributionSigningInput {
    pub sender_node_id: NodeId,
    pub target_node_id: NodeId,
    pub capability_name: String,
    pub version: Option<String>,
    pub timestamp: UnixTimestamp,
    pub force_install: bool,
    pub force_auth_overwrite: bool,
    pub auth_sha256: String,
    pub sealed_auth: SealedCapabilityAuth,
}

impl SignedCapabilityDistribution {
    pub fn sign(
        input: CapabilityDistributionSigningInput,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != input.sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let capability_name = validate_capability_name(input.capability_name)?;
        let version = validate_version(input.version)?;
        let auth_sha256 = validate_sha256_hex(&input.auth_sha256)?;
        validate_hex_len(&input.sealed_auth.ephemeral_pubkey_hex, 32)?;
        validate_hex_len(&input.sealed_auth.nonce_hex, 12)?;
        validate_nonempty_hex(&input.sealed_auth.ciphertext_hex)?;
        let signable = capability_distribution_signable(CapabilityDistributionSignable {
            sender_node_id: &input.sender_node_id,
            target_node_id: &input.target_node_id,
            capability_name: &capability_name,
            version: version.as_deref(),
            timestamp: input.timestamp,
            force_install: input.force_install,
            force_auth_overwrite: input.force_auth_overwrite,
            auth_sha256_hex: &auth_sha256,
        });
        Ok(Self {
            sender_node_id: input.sender_node_id,
            target_node_id: input.target_node_id,
            capability_name,
            version,
            timestamp: input.timestamp,
            force_install: input.force_install,
            force_auth_overwrite: input.force_auth_overwrite,
            auth_sha256,
            sealed_auth_ephemeral_pubkey_hex: input.sealed_auth.ephemeral_pubkey_hex,
            sealed_auth_nonce_hex: input.sealed_auth.nonce_hex,
            sealed_auth_ciphertext_hex: input.sealed_auth.ciphertext_hex,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        input: CapabilityDistributionSigningInput,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != input.sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let capability_name = validate_capability_name(input.capability_name)?;
        let version = validate_version(input.version)?;
        let auth_sha256 = validate_sha256_hex(&input.auth_sha256)?;
        validate_hex_len(&input.sealed_auth.ephemeral_pubkey_hex, 32)?;
        validate_hex_len(&input.sealed_auth.nonce_hex, 12)?;
        validate_nonempty_hex(&input.sealed_auth.ciphertext_hex)?;
        let signable = capability_distribution_signable(CapabilityDistributionSignable {
            sender_node_id: &input.sender_node_id,
            target_node_id: &input.target_node_id,
            capability_name: &capability_name,
            version: version.as_deref(),
            timestamp: input.timestamp,
            force_install: input.force_install,
            force_auth_overwrite: input.force_auth_overwrite,
            auth_sha256_hex: &auth_sha256,
        });
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::CapabilityDistributionV1,
            signable,
        )?;
        let sig = request
            .sign_with(
                input.sender_node_id,
                signer,
                polar_identity::SignOptions::default(),
            )
            .await?;
        Ok(Self {
            sender_node_id: input.sender_node_id,
            target_node_id: input.target_node_id,
            capability_name,
            version,
            timestamp: input.timestamp,
            force_install: input.force_install,
            force_auth_overwrite: input.force_auth_overwrite,
            auth_sha256,
            sealed_auth_ephemeral_pubkey_hex: input.sealed_auth.ephemeral_pubkey_hex,
            sealed_auth_nonce_hex: input.sealed_auth.nonce_hex,
            sealed_auth_ciphertext_hex: input.sealed_auth.ciphertext_hex,
            sig,
        })
    }
}

pub fn verify_signed_capability_distribution(
    distribution: &SignedCapabilityDistribution,
) -> Result<VerifiedCapabilityDistribution, ProtocolError> {
    let capability_name = validate_capability_name(distribution.capability_name.clone())?;
    let version = validate_version(distribution.version.clone())?;
    let auth_sha256 = validate_sha256_hex(&distribution.auth_sha256)?;
    validate_hex_len(&distribution.sealed_auth_ephemeral_pubkey_hex, 32)?;
    validate_hex_len(&distribution.sealed_auth_nonce_hex, 12)?;
    validate_nonempty_hex(&distribution.sealed_auth_ciphertext_hex)?;
    let signable = capability_distribution_signable(CapabilityDistributionSignable {
        sender_node_id: &distribution.sender_node_id,
        target_node_id: &distribution.target_node_id,
        capability_name: &capability_name,
        version: version.as_deref(),
        timestamp: distribution.timestamp,
        force_install: distribution.force_install,
        force_auth_overwrite: distribution.force_auth_overwrite,
        auth_sha256_hex: &auth_sha256,
    });
    super::canonical::signable::verify_signature(
        &distribution.sender_node_id,
        &signable,
        &distribution.sig,
    )?;
    Ok(VerifiedCapabilityDistribution {
        sender_node_id: distribution.sender_node_id,
        target_node_id: distribution.target_node_id,
        capability_name,
        version,
        timestamp: distribution.timestamp,
        force_install: distribution.force_install,
        force_auth_overwrite: distribution.force_auth_overwrite,
        auth_sha256,
        sealed_auth_ephemeral_pubkey_hex: distribution.sealed_auth_ephemeral_pubkey_hex.clone(),
        sealed_auth_nonce_hex: distribution.sealed_auth_nonce_hex.clone(),
        sealed_auth_ciphertext_hex: distribution.sealed_auth_ciphertext_hex.clone(),
    })
}

fn validate_capability_name(name: String) -> Result<String, ProtocolError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 64 {
        return Err(ProtocolError::InvalidCapability);
    }
    Ok(trimmed.to_string())
}

fn validate_version(version: Option<String>) -> Result<Option<String>, ProtocolError> {
    version
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 64 {
                Err(ProtocolError::InvalidCapability)
            } else {
                Ok(trimmed.to_string())
            }
        })
        .transpose()
}

fn validate_sha256_hex(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    let bytes = hex::decode(trimmed).map_err(|_| ProtocolError::InvalidSecret)?;
    if bytes.len() != 32 {
        return Err(ProtocolError::InvalidSecret);
    }
    Ok(trimmed.to_lowercase())
}

fn validate_hex_len(value: &str, expected_len: usize) -> Result<(), ProtocolError> {
    let bytes = hex::decode(value.trim()).map_err(|_| ProtocolError::InvalidSecret)?;
    if bytes.len() != expected_len {
        return Err(ProtocolError::InvalidSecret);
    }
    Ok(())
}

fn validate_nonempty_hex(value: &str) -> Result<(), ProtocolError> {
    let bytes = hex::decode(value.trim()).map_err(|_| ProtocolError::InvalidSecret)?;
    if bytes.is_empty() {
        return Err(ProtocolError::InvalidSecret);
    }
    Ok(())
}


