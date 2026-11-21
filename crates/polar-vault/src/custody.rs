//! Narrow encrypted inventory for an offline change of trust provider.
use crate::{SealedAccessRelation, VaultCipher, VaultCiphertext};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedGenerationWorkflow {
    pub name: String,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCustodyRows {
    pub relations: Vec<SealedAccessRelation>,
    pub incoming: Vec<SealedGenerationWorkflow>,
    pub outgoing: Vec<SealedGenerationWorkflow>,
}

impl SealedGenerationWorkflow {
    /// These historical scopes predate authenticated row names. Consumers must
    /// compare the name and verify the signed packets before using a workflow.
    pub async fn open_batch(cipher: &dyn VaultCipher, rows: &[Self]) -> Result<Vec<Vec<u8>>> {
        let sealed: Vec<_> = rows
            .iter()
            .map(|r| VaultCiphertext {
                nonce: &r.nonce,
                ciphertext: &r.ciphertext,
            })
            .collect();
        let plain = cipher.open_batch(&sealed).await?;
        ensure!(
            plain.len() == rows.len(),
            "incomplete historical workflow decryption"
        );
        Ok(plain)
    }
}
