//! Per-secret storage and execution contract. These labels describe data flow,
//! never an authorization grant or a claim that hardware was attested.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStorage {
    Vault,
    HostMemory,
    DeviceMemory,
    SecureElement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretCompute {
    Host,
    Device,
    SecureElement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultLocation {
    Host,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretRetention {
    Operation,
    Session { ttl_ms: u32 },
    HardwareObject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretProtection {
    pub storage: SecretStorage,
    pub compute: SecretCompute,
    pub retention: SecretRetention,
    pub vault: Option<VaultLocation>,
}

/// Configured plaintext location, not a runtime measurement or attestation.
/// Ciphertext location is still carried separately by SecretProtection::vault.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaintextLocation {
    HostRam,
    DeviceRam,
    SecureElement,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecretCopyView {
    pub policy: SecretProtection,
    pub label: String,
    pub plaintext: PlaintextLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectionError {
    UnsupportedCombination,
    InvalidRetention,
    MissingVault,
    UnexpectedVault,
    CapabilityUnavailable,
}

/// Capabilities come from the assembled executor, never from an untrusted
/// request. Object/algorithm/owner admission remains the executor's job.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtectionCapabilities {
    pub host: bool,
    pub device: bool,
    pub secure_element: bool,
    pub device_vault: bool,
}

impl SecretProtection {
    pub fn label(&self) -> &'static str {
        match (self.storage, self.compute) {
            (SecretStorage::Vault, SecretCompute::Host) => "V/C1",
            (SecretStorage::Vault, SecretCompute::Device) => "V/C2",
            (SecretStorage::HostMemory, SecretCompute::Host) => "M1/C1",
            (SecretStorage::DeviceMemory, SecretCompute::Device) => "M2/C2",
            (SecretStorage::SecureElement, SecretCompute::SecureElement) => "H/C3",
            _ => "unsupported",
        }
    }

    /// Only the five admissible topology contracts are accepted. In
    /// particular, validation never silently exports an H key or downgrades it.
    pub fn validate(&self, caps: ProtectionCapabilities) -> Result<(), ProtectionError> {
        use ProtectionError as E;
        use SecretRetention as R;
        use SecretStorage as S;
        if self.label() == "unsupported" {
            return Err(E::UnsupportedCombination);
        }
        match (self.storage, self.retention) {
            (S::Vault, R::Operation) | (S::SecureElement, R::HardwareObject) => {}
            (S::HostMemory | S::DeviceMemory, R::Operation) => {}
            (S::HostMemory | S::DeviceMemory, R::Session { ttl_ms }) if ttl_ms > 0 => {}
            _ => return Err(E::InvalidRetention),
        }
        if self.storage == S::Vault && self.vault.is_none() {
            return Err(E::MissingVault);
        }
        if self.storage == S::SecureElement && self.vault.is_some() {
            // A recoverable software copy is a separate record; do not hide it
            // in an H object and imply that every copy is non-exportable.
            return Err(E::UnexpectedVault);
        }
        let available = match self.compute {
            SecretCompute::Host => caps.host,
            SecretCompute::Device => caps.device,
            SecretCompute::SecureElement => caps.secure_element,
        };
        if !available || (self.vault == Some(VaultLocation::Device) && !caps.device_vault) {
            return Err(E::CapabilityUnavailable);
        }
        Ok(())
    }

    /// Call only for a policy already admitted by the assembled directory.
    /// Every backup gets its own view; H/C3 never hides a V/C1 copy.
    pub fn configured_copy(&self) -> SecretCopyView {
        SecretCopyView {
            policy: *self,
            label: self.label().into(),
            plaintext: match self.compute {
                SecretCompute::Host => PlaintextLocation::HostRam,
                SecretCompute::Device => PlaintextLocation::DeviceRam,
                SecretCompute::SecureElement => PlaintextLocation::SecureElement,
            },
        }
    }
}

/// Reports every copy rather than computing a misleading scalar score.
pub fn validate_secret_copies(
    copies: &[SecretProtection],
    capabilities: ProtectionCapabilities,
) -> Result<(), ProtectionError> {
    if copies.is_empty() || copies.len() > 8 {
        return Err(ProtectionError::UnsupportedCombination);
    }
    for copy in copies {
        copy.validate(capabilities)?;
    }
    Ok(())
}


