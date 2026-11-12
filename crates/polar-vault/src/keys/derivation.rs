//! Standard SLIP-0010, using the pinned dependency. Root NodeId is NOT derived.
use super::{KeyAlgorithm, KeyPurpose};
use anyhow::{ensure, Result};
use bip0032::{
    curve::{
        ed25519::{Ed25519Curve, Ed25519DalekBackend},
        nist256p1::{Nist256p1Curve, P256Backend},
    },
    slip10::{Slip10HardenedOnlyDerivation, Slip10MasterKey, Slip10NonHardenedDerivation},
    DerivationPath, ExtendedPrivateKey, HardenedDerivationPath,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationSpec {
    pub algorithm: KeyAlgorithm,
    pub purpose: KeyPurpose,
    pub namespace: u32,
    pub index: u32,
}
impl DerivationSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.purpose, KeyPurpose::Ssh | KeyPurpose::WebAuthn),
            "root and runtime working keys use their existing derivations"
        );
        ensure!(
            self.namespace < 1 << 31 && self.index < 1 << 31,
            "derivation index exceeds hardened domain"
        );
        Ok(())
    }
    pub fn path(&self) -> Result<String> {
        self.validate()?;
        let purpose = match self.purpose {
            KeyPurpose::Ssh => 1,
            KeyPurpose::WebAuthn => 2,
            _ => unreachable!(),
        };
        let algorithm = match self.algorithm {
            KeyAlgorithm::Ed25519 => 1,
            KeyAlgorithm::P256 => 2,
        };
        Ok(format!(
            "m/10016H/{purpose}H/{algorithm}H/{}H/{}H",
            self.namespace, self.index
        ))
    }
    /// Explicit initializer/authorized C1 operation only. A chip handle cannot
    /// call this without seed: unsupported H derivation must fail, not export.
    pub fn derive(&self, seed: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
        derive_path(seed, self.algorithm, &self.path()?)
    }
}
fn derive_path(seed: &[u8], algorithm: KeyAlgorithm, path: &str) -> Result<Zeroizing<[u8; 32]>> {
    let hardened: HardenedDerivationPath = path.parse()?;
    ensure!(path.len() <= 160, "derivation path too long");
    match algorithm {
        KeyAlgorithm::Ed25519 => {
            type Curve = Ed25519Curve<Ed25519DalekBackend>;
            let master = ExtendedPrivateKey::<Curve>::new_slip10(seed)?;
            Ok(Slip10HardenedOnlyDerivation::derive_slip10_path(&master, &hardened)?.to_bytes())
        }
        KeyAlgorithm::P256 => {
            type Curve = Nist256p1Curve<P256Backend>;
            let path: DerivationPath = path.parse()?;
            let master = ExtendedPrivateKey::<Curve>::new_slip10(seed)?;
            Ok(Slip10NonHardenedDerivation::derive_slip10_path(&master, &path)?.to_bytes())
        }
    }
}

