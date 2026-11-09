//! Authenticated public state shared by all existing persistence entry points.
//! This cache never contains private keys or invitation bearer tokens.
use crate::{operations::decode_material, PolarVault, VaultCiphertext};
use anyhow::{anyhow, Result};
use std::{collections::BTreeMap, sync::RwLock};

pub(crate) const SCOPES: &[&str] = &[
    "local_chat_authority_v1",
    "access_plane_trust",
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
                for ((scope, name, _, _), value) in rows.into_iter().zip(plain) {
                    let value = decode_material(&scope, &name, value)?;
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
