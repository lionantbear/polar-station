//! Per-secret permissions. Storage labels never grant permission.
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationRule {
    Allow,
    HostConfirmation,
    DeviceConfirmation,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretOperation {
    Metadata,
    Use,
    CiphertextExport,
    PlaintextExport,
    Rotate,
    Revoke,
    Delete,
    PolicyChange,
}

/// All fields are explicit for new configuration; omitted fields are errors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretPolicy {
    pub metadata: OperationRule,
    #[serde(rename = "use")]
    pub use_secret: OperationRule,
    pub ciphertext_export: OperationRule,
    pub plaintext_export: OperationRule,
    pub rotate: OperationRule,
    pub revoke: OperationRule,
    pub delete: OperationRule,
    pub policy_change: OperationRule,
}
impl SecretPolicy {
    /// Migration of the existing SSH directory only. Not a general creation
    /// default, and does not claim that the unused export APIs exist.
    pub fn legacy_ssh() -> Self {
        use OperationRule::*;
        Self {
            metadata: Allow,
            use_secret: Allow,
            ciphertext_export: Deny,
            plaintext_export: Deny,
            rotate: Allow,
            revoke: Allow,
            delete: Deny,
            policy_change: Allow,
        }
    }
    pub fn rule(&self, operation: SecretOperation) -> OperationRule {
        match operation {
            SecretOperation::Metadata => self.metadata,
            SecretOperation::Use => self.use_secret,
            SecretOperation::CiphertextExport => self.ciphertext_export,
            SecretOperation::PlaintextExport => self.plaintext_export,
            SecretOperation::Rotate => self.rotate,
            SecretOperation::Revoke => self.revoke,
            SecretOperation::Delete => self.delete,
            SecretOperation::PolicyChange => self.policy_change,
        }
    }
    /// Caller already has owner/purpose authorization. Confirmation does not
    /// become Allow just because a transport has not implemented its UI yet.
    pub fn require(&self, operation: SecretOperation) -> Result<()> {
        match self.rule(operation) {
            OperationRule::Allow => Ok(()),
            OperationRule::Deny => bail!("secret operation denied"),
            OperationRule::HostConfirmation => bail!("secret operation requires host confirmation"),
            OperationRule::DeviceConfirmation => {
                bail!("secret operation requires device confirmation")
            }
        }
    }
    pub fn validate_connected_ssh(&self) -> Result<()> {
        let mut supported = Self::legacy_ssh();
        supported.use_secret = self.use_secret;
        supported.rotate = self.rotate;
        supported.revoke = self.revoke;
        supported.policy_change = self.policy_change;
        ensure!(
            *self == supported
                && [
                    self.use_secret,
                    self.rotate,
                    self.revoke,
                    self.policy_change
                ]
                .iter()
                .all(|r| matches!(r, OperationRule::Allow | OperationRule::Deny)),
            "SSH policy operation/confirmation is not connected"
        );
        Ok(())
    }
}


