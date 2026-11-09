//! Typed asynchronous private-key operations. A handle names one identity for
//! its entire lifetime. Purposes are admission profiles, never key selectors.
use std::{fmt, sync::Arc, time::Instant};

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

use crate::signed_identity::NodeIdSignatureVerifier;
use crate::{BindingPublicKey, NodeId, SignatureBytes, SignatureVerifier};



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum SigningPurpose {
    LgpAuthorV5 = 0x0001,
    WirePacketV2 = 0x0002,
    LgpAuthorDigestV6 = 0x0003,
    WirePacketDigestV3 = 0x0004,
    NodeIdentityV1 = 0x0010,
    NodeRenameV1 = 0x0011,
    BindingRebindV1 = 0x0012,
    DiscoveryV1 = 0x0020,
    HeartbeatV1 = 0x0021,
    SessionKeyUpdateV1 = 0x0022,
    DmReplayV1 = 0x0023,
    CapabilityDistributionV1 = 0x0024,
    LegacyLgpRouteV1 = 0x0025,
    TrustRequestV1 = 0x0030,
    TrustDecisionV1 = 0x0031,
    RealmEnrollmentV1 = 0x0040,
    RealmMemberTrustV1 = 0x0041,
    AccessReceiptV1 = 0x0042,
    RealmEnrollmentTokenV1 = 0x0043,
    RealmEnrollmentDecisionV1 = 0x0044,
    RealmMemberTrustDecisionV1 = 0x0045,
    DeliveryPublicationV1 = 0x0050,
    ProducerProvenanceV1 = 0x0051,
    BootstrapInvitationV1 = 0x0060,
    CompactBootstrapInvitationV1 = 0x0061,
    CompactRealmTrustInvitationV1 = 0x0062,
}

impl SigningPurpose {
    pub const ALL: &'static [Self] = &[
        Self::LgpAuthorV5,
        Self::WirePacketV2,
        Self::LgpAuthorDigestV6,
        Self::WirePacketDigestV3,
        Self::NodeIdentityV1,
        Self::NodeRenameV1,
        Self::BindingRebindV1,
        Self::DiscoveryV1,
        Self::HeartbeatV1,
        Self::SessionKeyUpdateV1,
        Self::DmReplayV1,
        Self::CapabilityDistributionV1,
        Self::LegacyLgpRouteV1,
        Self::TrustRequestV1,
        Self::TrustDecisionV1,
        Self::RealmEnrollmentV1,
        Self::RealmMemberTrustV1,
        Self::AccessReceiptV1,
        Self::RealmEnrollmentTokenV1,
        Self::RealmEnrollmentDecisionV1,
        Self::RealmMemberTrustDecisionV1,
        Self::DeliveryPublicationV1,
        Self::ProducerProvenanceV1,
        Self::BootstrapInvitationV1,
        Self::CompactBootstrapInvitationV1,
        Self::CompactRealmTrustInvitationV1,
    ];

    /// Application-level domain separation; these remain ordinary Ed25519
    /// signatures, not Ed25519ph. Legacy purposes retain their original bytes.
    pub fn digest_domain(self) -> Option<&'static [u8; 8]> {
        match self {
            Self::LgpAuthorDigestV6 => Some(b"PLGA0001"),
            Self::WirePacketDigestV3 => Some(b"PWPK0001"),
            _ => None,
        }
    }

    pub fn digest_preimage(self, canonical: &[u8]) -> Result<[u8; 40], SignerError> {
        use sha2::{Digest, Sha256};
        let domain = self.digest_domain().ok_or(SignerError::InvalidRequest)?;
        let mut result = [0; 40];
        result[..8].copy_from_slice(domain);
        result[8..].copy_from_slice(&Sha256::digest(canonical));
        Ok(result)
    }

    fn validate_preimage(self, canonical: &[u8]) -> Result<(), SignerError> {
        if canonical.is_empty()
            || self
                .digest_domain()
                .is_some_and(|domain| canonical.len() != 40 || !canonical.starts_with(domain))
        {
            return Err(SignerError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerBackendKind {
    Software,
    PolarHear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerDescriptor {
    pub node_id: NodeId,
    pub binding_public_key: BindingPublicKey,
    pub backend: SignerBackendKind,
    pub max_preimage_bytes: usize,
    pub max_batch: usize,
    pub supported_purposes: &'static [SigningPurpose],
    /// False for software checks. A hardware adapter must report actual device
    /// enforcement, not merely the fact that its USB endpoint was reachable.
    pub device_enforced_purpose: bool,
}

impl SignerDescriptor {
    pub fn software(node_id: NodeId, binding_public_key: BindingPublicKey) -> Self {
        Self {
            node_id,
            binding_public_key,
            backend: SignerBackendKind::Software,
            max_preimage_bytes: usize::MAX,
            max_batch: 32,
            supported_purposes: SigningPurpose::ALL,
            device_enforced_purpose: false,
        }
    }

    pub fn admit(&self, request: &SignRequest) -> Result<(), SignerError> {
        if !self.supported_purposes.contains(&request.purpose) {
            return Err(SignerError::UnsupportedPurpose(request.purpose));
        }
        request.purpose.validate_preimage(&request.canonical)?;
        if request.canonical.len() > self.max_preimage_bytes {
            return Err(SignerError::PayloadTooLarge);
        }
        Ok(())
    }
}

/// Canonical bytes are frozen before crossing an async or device boundary.
/// No key ID, backend ID, seed, or host-approved boolean is accepted here.
#[derive(Clone)]
pub struct SignRequest {
    id: Uuid,
    purpose: SigningPurpose,
    canonical: Arc<[u8]>,
    evidence: Arc<[u8]>,
}

impl fmt::Debug for SignRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignRequest")
            .field("id", &self.id)
            .field("purpose", &self.purpose)
            .field("canonical_bytes", &self.canonical.len())
            .field("evidence_bytes", &self.evidence.len())
            .finish_non_exhaustive()
    }
}

impl SignRequest {
    pub fn new(
        purpose: SigningPurpose,
        canonical: impl Into<Arc<[u8]>>,
    ) -> Result<Self, SignerError> {
        let canonical = canonical.into();
        purpose.validate_preimage(&canonical)?;
        Ok(Self {
            id: crate::generation::new_uuid_v7(),
            purpose,
            canonical,
            evidence: Arc::from([]),
        })
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn purpose(&self) -> SigningPurpose {
        self.purpose
    }
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }

    /// Public purpose proof, independently checked by the device. It never
    /// changes the canonical bytes or authorizes a different identity.
    pub fn with_evidence(mut self, evidence: impl Into<Arc<[u8]>>) -> Self {
        self.evidence = evidence.into();
        self
    }

    pub fn evidence(&self) -> &[u8] {
        &self.evidence
    }

    /// Sign these exact frozen bytes with the already-bound identity and verify
    /// the backend response before a domain object can publish it. This method
    /// does not select a key or retry a device operation.
    pub async fn sign_with(
        &self,
        expected_author: NodeId,
        signer: &dyn AsyncSigner,
        options: SignOptions,
    ) -> Result<SignatureBytes, SignerError> {
        if signer.descriptor().node_id != expected_author {
            return Err(SignerError::IdentityMismatch);
        }
        signer.descriptor().admit(self)?;
        options.check_deadline()?;
        let response = signer.sign(self.clone(), options).await?;
        self.verify_response(expected_author, &response)?;
        Ok(response.signature)
    }

    pub fn verify_response(
        &self,
        node: NodeId,
        response: &SignResponse,
    ) -> Result<(), SignerError> {
        if self.id != response.request_id {
            return Err(SignerError::RequestMismatch);
        }
        NodeIdSignatureVerifier
            .verify_signature(&node, &self.canonical, &response.signature)
            .map_err(|_| SignerError::InvalidBackendSignature)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SignOptions {
    pub deadline: Option<Instant>,
    pub allow_local_confirmation: bool,
}
impl SignOptions {
    pub fn check_deadline(&self) -> Result<(), SignerError> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(SignerError::DeadlineBeforeStart)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignResponse {
    pub request_id: Uuid,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignItemOutcome {
    Signed(SignResponse),
    Rejected {
        request_id: Uuid,
        error: SignerError,
    },
    NotStarted {
        request_id: Uuid,
    },
    CompletionUnknown {
        request_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignBatchOutcome {
    pub items: Vec<SignItemOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SignerError {
    #[error("signer closed; restart the Polar instance to select a backend")]
    Closed,
    #[error("signer identity differs from the configured identity")]
    IdentityMismatch,
    #[error("unsupported signing purpose: {0:?}")]
    UnsupportedPurpose(SigningPurpose),
    #[error("unsupported signer or transport capability")]
    Unsupported,
    #[error("invalid signing request")]
    InvalidRequest,
    #[error("signing request exceeds the admitted size")]
    PayloadTooLarge,
    #[error("invalid signing batch size")]
    InvalidBatch,
    #[error("device rejected the request before signing")]
    PolicyDenied,
    #[error("device queue is full")]
    QueueFull,
    #[error("deadline elapsed before execution")]
    DeadlineBeforeStart,
    #[error("device operation may have completed; no automatic retry is permitted")]
    CompletionUnknown,
    #[error("device unavailable; no fallback signer is configured")]
    Disconnected,
    #[error("device response does not match the request")]
    RequestMismatch,
    #[error("device returned an invalid signature")]
    InvalidBackendSignature,
    #[error("device protocol rejected or malformed")]
    Protocol,
    #[error("device requires local confirmation")]
    InteractionRequired,
}

#[async_trait]
pub trait AsyncSigner: Send + Sync {
    fn descriptor(&self) -> &SignerDescriptor;

    async fn sign(
        &self,
        request: SignRequest,
        options: SignOptions,
    ) -> Result<SignResponse, SignerError>;

    /// Fallback batching is sequential signing, not aggregate signatures or an
    /// atomic transaction. Implementations may override with a native batch.
    async fn sign_batch(
        &self,
        requests: Vec<SignRequest>,
        options: SignOptions,
    ) -> Result<SignBatchOutcome, SignerError> {
        if requests.is_empty() || requests.len() > self.descriptor().max_batch {
            return Err(SignerError::InvalidBatch);
        }
        // Reject structurally inadmissible batches before the first operation.
        for request in &requests {
            self.descriptor().admit(request)?;
        }
        options.check_deadline()?;
        let mut items = Vec::with_capacity(requests.len());
        let mut stopped = false;
        for request in requests {
            let request_id = request.id();
            if stopped {
                items.push(SignItemOutcome::NotStarted { request_id });
                continue;
            }
            let outcome = match self.sign(request, options).await {
                Ok(response) => SignItemOutcome::Signed(response),
                Err(SignerError::CompletionUnknown) => {
                    stopped = true;
                    SignItemOutcome::CompletionUnknown { request_id }
                }
                Err(error) => {
                    stopped = true;
                    SignItemOutcome::Rejected { request_id, error }
                }
            };
            items.push(outcome);
        }
        Ok(SignBatchOutcome { items })
    }
}
