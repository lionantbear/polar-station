//! Authenticated public state shared by all existing persistence entry points.
//! This cache never contains private keys or invitation bearer tokens.
use crate::{operations::decode_material, PolarVault, VaultCiphertext};
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::RwLock};

pub(crate) const SCOPES: &[&str] = &[
    "local_chat_authority_v1",
    "access_plane_trust",
    "access_generation_approval",
    "access_generation_outbound",
    "access_generation_material",
    "access_initial_generation",
    "access_bind_requests",
    "realm_enrollment",
    "realm_member_trust",
    "management_request_v1",
    "bootstrap_invitation_metadata",
    "reception_admission_grant",
    "realm_enrollment_token_use",
];
pub(crate) fn cached_scope(scope: &str) -> bool {
    SCOPES.contains(&scope)
}
#[derive(Default)]
pub(crate) struct PublicState {
    pub revision: u64,
    pub records: BTreeMap<(String, String), String>,
    pub versions: BTreeMap<(String, String), [u8; 32]>,
}

impl PolarVault {
    pub(crate) async fn public_state(&self) -> Result<&RwLock<PublicState>> {
        self.public_state
            .get_or_try_init(|| async {
                let mut rows = Vec::new();
                for scope in SCOPES {
                    for (name, nonce, ciphertext) in
                        self.store.named_secret_material((*scope).into()).await?
                    {
                        rows.push(((*scope).to_string(), name, nonce, ciphertext));
                    }
                }
                let sealed = rows
                    .iter()
                    .map(|(_, _, nonce, ciphertext)| VaultCiphertext { nonce, ciphertext })
                    .collect::<Vec<_>>();
                let plain = self.cipher.open_batch(&sealed).await?;
                anyhow::ensure!(plain.len() == rows.len(), "invalid public state batch");
                let mut state = PublicState::default();
                for ((scope, name, nonce, ciphertext), value) in rows.into_iter().zip(plain) {
                    let value = decode_material(&scope, &name, value)?;
                    state.versions.insert(
                        (scope.clone(), name.clone()),
                        material_version(&nonce, &ciphertext),
                    );
                    state.records.insert((scope, name), value);
                }
                Ok(RwLock::new(state))
            })
            .await
    }

    /// Versioned authenticated public data for domain indexes. No secret data.
    pub async fn authenticated_public_state_snapshot(
        &self,
    ) -> Result<crate::AuthenticatedPublicStateSnapshot> {
        let state = self
            .public_state()
            .await?
            .read()
            .map_err(|_| anyhow!("public state poisoned"))?;
        Ok((
            state.revision,
            state
                .records
                .iter()
                .map(|((scope, name), value)| (scope.clone(), name.clone(), value.clone()))
                .collect(),
        ))
    }

    pub async fn authenticated_public_state_revision(&self) -> Result<u64> {
        Ok(self
            .public_state()
            .await?
            .read()
            .map_err(|_| anyhow!("public state poisoned"))?
            .revision)
    }
}

/// Version of an already authenticated public authorization record. This is
/// not a secret/token export, and is stable when the same ciphertext is reopened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPublicStateVersion {
    pub scope: String,
    pub name: String,
    pub value: String,
    pub revision: [u8; 32],
}

pub(crate) fn material_version(nonce: &[u8], ciphertext: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"PolarPublicStateRevision");
    hash.update((nonce.len() as u64).to_be_bytes());
    hash.update(nonce);
    hash.update(ciphertext);
    hash.finalize().into()
}

impl PolarVault {
    /// One lock snapshots values and their durable versions. No new decrypt or
    /// database query after startup. Callers must still serialize policy changes
    /// with their approval commit; this snapshot does not itself grant rights.
    pub async fn authenticated_public_state_versions(
        &self,
    ) -> Result<Vec<AuthenticatedPublicStateVersion>> {
        let state = self
            .public_state()
            .await?
            .read()
            .map_err(|_| anyhow!("public state poisoned"))?;
        state
            .records
            .iter()
            .map(|((scope, name), value)| {
                Ok(AuthenticatedPublicStateVersion {
                    scope: scope.clone(),
                    name: name.clone(),
                    value: value.clone(),
                    revision: *state
                        .versions
                        .get(&(scope.clone(), name.clone()))
                        .ok_or_else(|| anyhow!("public state version missing"))?,
                })
            })
            .collect()
    }
}
