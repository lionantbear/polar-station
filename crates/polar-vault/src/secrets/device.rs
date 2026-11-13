//! Private use operations. The device returns a result, never decrypted key bytes.
use super::*;
use polar_identity::{signed_identity::NodeIdSignatureVerifier, SignatureBytes, SignatureVerifier};

#[async_trait]
pub trait Ed25519SecretExecutor: Send + Sync {
    fn compute_location(&self) -> crate::protection::SecretCompute;
    /// Low-level adapter called only after domain authorization. The device must
    /// independently authenticate the record AAD and check the derived public key.
    async fn sign_record(
        &self,
        record: &StoredSecret,
        public: NodeId,
        message: &[u8],
    ) -> Result<SignatureBytes>;
}

impl PolarVault {
    /// Authority was fixed by the authenticated owner runtime. Never accepts an
    /// owner, arbitrary chip object, or private-key bytes from the signing caller.
    pub async fn sign_ed25519_secret(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        public: NodeId,
        message: &[u8],
        executor: &dyn Ed25519SecretExecutor,
    ) -> Result<SignatureBytes> {
        ensure!(
            message.len() <= 32_768,
            "secret signing message exceeds capacity"
        );
        self.authorize_secret_operation(authority, id, SecretOperation::Use)
            .await?;
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !record.metadata.revoked && record.metadata.kind == SecretKind::GitSshPrivateKey,
            "credential unavailable"
        );
        ensure!(
            executor.compute_location()
                == if record.metadata.device_only {
                    crate::protection::SecretCompute::Device
                } else {
                    crate::protection::SecretCompute::Host
                },
            "secret execution policy mismatch"
        );
        ensure!(
            record.nonce.len() == 12 && record.ciphertext.len() == 48,
            "invalid Ed25519 seed envelope"
        );
        let signature = executor.sign_record(&record, public, message).await?;
        NodeIdSignatureVerifier.verify_signature(&public, message, &signature)?;
        self.authorize_secret_operation(authority, id, SecretOperation::Use)
            .await?;
        let current = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !current.metadata.revoked && current.metadata.version == record.metadata.version,
            "credential changed during use"
        );
        Ok(signature)
    }
}

/// Public device directory view; no private material or ciphertext.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSecretInfo {
    pub id: SecretId,
    pub vault_id: uuid::Uuid,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub owner: NodeId,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub public_key: NodeId,
    pub version: u64,
    pub ttl_ms: u32,
    pub revoked: bool,
    pub envelope_digest: [u8; 32],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceSecretMutation {
    Pending { request_token: String },
    Complete { info: DeviceSecretInfo },
}
impl PolarVault {
    /// Trusted management assembly only; deliberately not a serializable SDK
    /// record or a plaintext resolver. Reuses the same UUID authorization.
    pub async fn device_secret_record(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
    ) -> Result<StoredSecret> {
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            record.metadata.device_only && record.metadata.kind == SecretKind::GitSshPrivateKey,
            "device SSH record required"
        );
        ensure!(
            record.nonce.len() == 12 && record.ciphertext.len() == 48,
            "invalid device envelope"
        );
        Ok(record)
    }
}
