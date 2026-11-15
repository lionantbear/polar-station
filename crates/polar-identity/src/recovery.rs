//! Polar Recovery v1: encode the existing 32-byte identity seed as entropy.
//! This deliberately does not use BIP39's wallet seed/passphrase derivation.
use bip39::{Language, Mnemonic};
use zeroize::Zeroizing;

use crate::IdentitySeed;

pub const RECOVERY_FORMAT: &str = "polar-recovery-v1";

/// Only local, explicitly authorized recovery interactions may expose these words.
/// No Display/Serialize implementation: logging the wrapper must not disclose them.
pub struct RecoveryWords(Mnemonic);

impl std::fmt::Debug for RecoveryWords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryWords([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid Polar Recovery v1 words: expected 24 English words with a valid checksum")]
pub struct InvalidRecoveryWords;

impl RecoveryWords {
    pub fn from_seed(seed: &IdentitySeed) -> Self {
        Self(
            Mnemonic::from_entropy_in(Language::English, seed.as_bytes())
                .expect("32-byte identity entropy is valid"),
        )
    }

    pub fn parse(words: &str) -> Result<Self, InvalidRecoveryWords> {
        // English words require no Unicode normalization. Keep input bounded and
        // reject wallet mnemonics of other lengths rather than padding/truncating.
        if words.len() > 512 || !words.is_ascii() || words.split_whitespace().count() != 24 {
            return Err(InvalidRecoveryWords);
        }
        Mnemonic::parse_in_normalized(Language::English, words)
            .map(Self)
            .map_err(|_| InvalidRecoveryWords)
    }

    pub fn restore_seed(&self) -> IdentitySeed {
        let (entropy, length) = self.0.to_entropy_array();
        let entropy = Zeroizing::new(entropy);
        debug_assert_eq!(length, 32);
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&entropy[..32]);
        IdentitySeed::from_bytes(*seed)
    }

    /// The caller owns the local display boundary; never send this to ordinary RPC/logs.
    pub fn expose_for_local_recovery(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.to_string())
    }
}
