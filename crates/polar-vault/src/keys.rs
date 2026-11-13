//! Shared key management metadata. No private bytes in recovery directories.
use crate::{protection::*, SecretId};
use anyhow::{ensure, Result};
use polar_identity::NodeId;
use serde::{Deserialize, Serialize};
pub mod derivation;
pub mod directory;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAlgorithm {
    Ed25519,
    P256,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPurpose {
    PolarRoot,
    Ssh,
    WebAuthn,
    VaultWorkingKey,
    MessageWindow,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyManagement {
    VaultStored,
    PathDerived,
    ChipGenerated,
    ExternalImported,
    DerivedImported,
    VaultBackupChip,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeyRecovery {
    RootWords,
    RootWordsAndPath { path: derivation::DerivationSpec },
    VaultBackup { id: SecretId },
    ExternalBackup,
    Unrecoverable,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyRecord {
    pub id: SecretId,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub owner: NodeId,
    pub purpose: KeyPurpose,
    pub algorithm: KeyAlgorithm,
    pub management: KeyManagement,
    pub protection: SecretProtection,
    #[serde(default = "crate::policy::SecretPolicy::legacy_ssh")]
    pub policy: crate::policy::SecretPolicy,
    pub other_copies: Vec<SecretProtection>,
    pub recovery: KeyRecovery,
    pub public_key: Vec<u8>,
    /// Authenticated scope, e.g. RP ID or an SSH account label. Never a secret.
    pub scope: String,
    /// Object reference is metadata, not permission to use arbitrary chip IDs.
    pub hardware_object: Option<u32>,
}
impl KeyRecord {
    /// A consumer may reuse its executor only while these authenticated fields
    /// match. Operation permissions are checked separately on every use.
    pub fn same_ssh_configuration(&self, other: &Self) -> bool {
        self.id == other.id
            && self.owner == other.owner
            && self.purpose == other.purpose
            && self.algorithm == other.algorithm
            && self.management == other.management
            && self.protection == other.protection
            && self.other_copies == other.other_copies
            && self.recovery == other.recovery
            && self.public_key == other.public_key
            && self.scope == other.scope
            && self.hardware_object == other.hardware_object
    }
    pub fn validate(&self, owner: NodeId, caps: ProtectionCapabilities) -> Result<()> {
        ensure!(
            self.owner == owner && self.scope.len() <= 253 && !self.scope.contains('\0'),
            "invalid key owner or scope"
        );
        ensure!(
            match self.algorithm {
                KeyAlgorithm::Ed25519 => self.public_key.len() == 32,
                KeyAlgorithm::P256 =>
                    self.public_key.len() == 33 && matches!(self.public_key[0], 2 | 3),
            },
            "invalid public key encoding"
        );
        self.protection
            .validate(caps)
            .map_err(|_| anyhow::anyhow!("unsupported key protection"))?;
        ensure!(self.other_copies.len() <= 7, "too many key copies");
        for p in &self.other_copies {
            p.validate(caps)
                .map_err(|_| anyhow::anyhow!("unsupported backup protection"))?;
        }
        let hardware = self.protection.storage == SecretStorage::SecureElement;
        ensure!(
            hardware == self.hardware_object.is_some_and(|id| id != 0),
            "key object/protection mismatch"
        );
        match self.management {
            KeyManagement::VaultStored => ensure!(
                matches!(
                    self.protection.storage,
                    SecretStorage::Vault | SecretStorage::HostMemory | SecretStorage::DeviceMemory
                ) && self.protection.vault.is_some(),
                "Vault key requires an encrypted source and explicit residency"
            ),
            KeyManagement::PathDerived => {
                ensure!(
                    matches!(
                        self.protection.storage,
                        SecretStorage::HostMemory | SecretStorage::DeviceMemory
                    ),
                    "derived key must have explicit memory policy"
                );
                ensure!(
                    matches!(self.recovery, KeyRecovery::RootWordsAndPath { .. }),
                    "derived key requires recovery path"
                );
            }
            KeyManagement::ChipGenerated => {
                ensure!(hardware, "chip key requires H/C3");
                ensure!(
                    self.recovery == KeyRecovery::Unrecoverable,
                    "chip-random key has no seed recovery"
                );
            }
            KeyManagement::ExternalImported => {
                ensure!(hardware, "imported chip key requires H/C3");
                ensure!(
                    matches!(
                        self.recovery,
                        KeyRecovery::ExternalBackup | KeyRecovery::Unrecoverable
                    ) || (self.purpose == KeyPurpose::PolarRoot
                        && self.recovery == KeyRecovery::RootWords),
                    "external import requires honest backup status"
                );
            }
            KeyManagement::DerivedImported => {
                ensure!(
                    hardware && matches!(self.recovery, KeyRecovery::RootWordsAndPath { .. }),
                    "derived import requires H/C3 and path recovery"
                );
            }
            KeyManagement::VaultBackupChip => {
                ensure!(
                    hardware
                        && matches!(self.recovery, KeyRecovery::VaultBackup { .. })
                        && self
                            .other_copies
                            .iter()
                            .any(|p| p.storage == SecretStorage::Vault),
                    "chip plus Vault backup must disclose its software copy"
                );
            }
        }
        if let KeyRecovery::RootWordsAndPath { path } = &self.recovery {
            ensure!(
                path.algorithm == self.algorithm && path.purpose == self.purpose,
                "recovery path domain mismatch"
            );
            path.validate()?;
        }
        if self.purpose == KeyPurpose::PolarRoot {
            ensure!(
                self.algorithm == KeyAlgorithm::Ed25519
                    && self.public_key.as_slice() == owner.as_bytes()
                    && self.recovery == KeyRecovery::RootWords,
                "Polar root must preserve NodeId and mandatory recovery words"
            );
            ensure!(
                !matches!(
                    self.management,
                    KeyManagement::ChipGenerated | KeyManagement::PathDerived
                ),
                "Polar root cannot be a new random or derived identity"
            );
        }
        Ok(())
    }
}
