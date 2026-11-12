use anyhow::{Context, Result};
use async_trait::async_trait;
use polar_identity::{NodeId, UnixTimestamp};
use polar_trust::{ControlAuthorityRecord, ControlScope, ManagedNodeRecord, TrustRecord};
use std::sync::Arc;

use crate::{
    CapabilitySecurityState, ManagementRequestRecord, ManagementRequestStatus, PolarVault,
};

type CipherRow = (String, Vec<u8>, Vec<u8>);

const STATE_PREFIX: &str = "POLAR_STATE_V1\n";
fn protected_state_scope(scope: &str) -> bool {
    matches!(
        scope,
        "local_chat_authority_v1"
            | "peer_authorization_v1"
            | "access_plane_trust"
            | "access_bind_requests"
            | "realm_enrollment"
            | "realm_member_trust"
            | "realm_enrollment_token_use"
            | "bootstrap_invitation"
            | "reception_admission_grant"
            | "management_request_v1"
            | "bootstrap_invitation_metadata"
    )
}
pub(crate) fn decode_material(scope: &str, name: &str, plain: Vec<u8>) -> Result<String> {
    let value = String::from_utf8(plain)
        .with_context(|| format!("decode secret material {scope}/{name}"))?;
    if !protected_state_scope(scope) {
        return Ok(value);
    }
    if let Some(json) = value.strip_prefix(STATE_PREFIX) {
        let (stored_scope, stored_name, value): (String, String, String) =
            serde_json::from_str(json)?;
        anyhow::ensure!(
            stored_scope == scope && stored_name == name,
            "authenticated state context mismatch"
        );
        Ok(value)
    } else {
        anyhow::ensure!(
            !protected_state_scope(scope),
            "unbound legacy authorization requires verified recovery or reapproval"
        );
        Ok(value)
    }
}

/// Driver-free vault persistence contract. The domain vault owns key material
/// and ciphertext policy; adapters own rows, SQL, and backend runtime access.
#[async_trait]
pub trait VaultStore: Send + Sync {
    async fn load_key_directory(
        &self,
        _owner: NodeId,
    ) -> Result<Option<crate::keys::directory::StoredDirectory>> {
        anyhow::bail!("key directory unsupported by Vault adapter")
    }
    async fn compare_exchange_key_directory(
        &self,
        _owner: NodeId,
        _expected: u64,
        _record: crate::keys::directory::StoredDirectory,
    ) -> Result<bool> {
        anyhow::bail!("key directory unsupported by Vault adapter")
    }
    async fn query_uuid_secrets(
        &self,
        _owner: NodeId,
        _query: crate::SecretQuery,
    ) -> Result<Vec<crate::SecretMetadata>> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn uuid_vault_identity(&self) -> Result<uuid::Uuid> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn load_uuid_secret(&self, _id: crate::SecretId) -> Result<Option<crate::StoredSecret>> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn find_uuid_import(
        &self,
        _owner: NodeId,
        _import: uuid::Uuid,
    ) -> Result<Option<crate::SecretId>> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn insert_uuid_secret(
        &self,
        _record: crate::StoredSecret,
        _import: uuid::Uuid,
    ) -> Result<crate::SecretId> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn rotate_uuid_secret(
        &self,
        _record: crate::StoredSecret,
        _revision: u64,
    ) -> Result<bool> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn revoke_uuid_secret(
        &self,
        _id: crate::SecretId,
        _owner: NodeId,
        _revision: u64,
    ) -> Result<bool> {
        anyhow::bail!("UUID credentials unsupported by Vault adapter")
    }
    async fn put_admin_secret(
        &self,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()>;
    async fn admin_secret(&self, name: String) -> Result<Option<(Vec<u8>, Vec<u8>)>>;
    async fn raw_admin_ciphertext(&self, name: String) -> Result<Option<Vec<u8>>>;

    async fn put_capability_state(&self, state: CapabilitySecurityState) -> Result<()>;
    async fn get_capability_state(&self, name: String) -> Result<Option<CapabilitySecurityState>>;
    async fn list_capability_states(&self) -> Result<Vec<CapabilitySecurityState>>;
    async fn delete_capability_state(&self, name: String) -> Result<()>;

    async fn put_control_authority(&self, record: ControlAuthorityRecord) -> Result<()>;
    async fn get_control_authority(&self, id: NodeId) -> Result<Option<ControlAuthorityRecord>>;
    async fn revoke_control_authority(&self, id: NodeId) -> Result<()>;
    async fn materialize_control_bus_ready_peer_pair(
        &self,
        local: NodeId,
        peer: NodeId,
        at: UnixTimestamp,
        scope: ControlScope,
    ) -> Result<()>;
    async fn clear_control_bus_ready_peer_pair(&self, peer: NodeId) -> Result<()>;

    async fn put_managed_node(&self, record: ManagedNodeRecord) -> Result<()>;
    async fn get_managed_node(&self, id: NodeId) -> Result<Option<ManagedNodeRecord>>;
    async fn list_managed_nodes(&self) -> Result<Vec<ManagedNodeRecord>>;
    async fn revoke_managed_node(&self, id: NodeId) -> Result<()>;

    async fn put_management_request(&self, record: ManagementRequestRecord) -> Result<()>;
    async fn get_management_request(&self, id: String) -> Result<Option<ManagementRequestRecord>>;
    async fn list_management_requests(&self) -> Result<Vec<ManagementRequestRecord>>;
    async fn latest_management_request_for_peer(
        &self,
        id: NodeId,
    ) -> Result<Option<ManagementRequestRecord>>;
    async fn resolve_management_request(
        &self,
        id: String,
        status: ManagementRequestStatus,
        at: UnixTimestamp,
        response: Option<String>,
    ) -> Result<()>;

    async fn put_node_setting(&self, key: String, value: String) -> Result<()>;
    async fn get_node_setting(&self, key: String) -> Result<Option<String>>;
    async fn put_runtime_dedup(&self, key: String, value: String, ttl: Option<u64>) -> Result<()>;
    async fn get_runtime_dedup(&self, key: String) -> Result<Option<String>>;

    async fn put_secret_material(
        &self,
        scope: String,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()>;
    async fn insert_secret_material(
        &self,
        _scope: String,
        _name: String,
        _nonce: Vec<u8>,
        _ciphertext: Vec<u8>,
    ) -> Result<()> {
        Err(anyhow::anyhow!(
            "atomic secret initialization is unsupported by this vault adapter"
        ))
    }
    async fn secret_material(
        &self,
        scope: String,
        name: String,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>>;
    async fn delete_secret_material(&self, scope: String, name: String) -> Result<()>;
    async fn named_secret_material(&self, scope: String) -> Result<Vec<CipherRow>>;
    async fn named_secret_material_by_scope_prefix(
        &self,
        prefix: String,
        name: String,
    ) -> Result<Vec<CipherRow>>;
    async fn secret_scopes_with_prefix(&self, prefix: String) -> Result<Vec<String>>;
    async fn secret_material_keys(&self) -> Result<Vec<(String, String)>>;

    async fn put_trust_record(&self, record: TrustRecord) -> Result<()>;
    async fn get_trust_record(&self, id: NodeId) -> Result<Option<TrustRecord>>;
    async fn list_trust_records(&self) -> Result<Vec<TrustRecord>>;
    async fn count_trust_records(&self) -> Result<i64>;
}

impl PolarVault {
    /// Constructs the domain vault from an already-published, driver-opaque
    /// persistence adapter. Bootstrap owns migrations and runtime creation.
    pub fn from_store(
        store: Arc<dyn VaultStore>,
        seed: polar_identity::IdentitySeed,
    ) -> Result<Self> {
        Ok(Self::from_cipher(
            store,
            Arc::new(crate::SoftwareVaultCipher::new(&seed)?),
        ))
    }

    pub fn from_cipher(store: Arc<dyn VaultStore>, cipher: Arc<dyn crate::VaultCipher>) -> Self {
        Self {
            store,
            cipher,
            authorization: Default::default(),
            local_chat_changes: tokio::sync::watch::channel(0).0,
            secret_changes: tokio::sync::watch::channel(0).0,
            key_directory: Default::default(),
            authorization_write: Default::default(),
            material_index: Default::default(),
            material_write: Default::default(),
            management_write: Default::default(),
            public_state: Default::default(),
        }
    }

    pub async fn put_admin_secret(&self, name: &str, secret: &str) -> Result<()> {
        let (nonce, ciphertext) = self.cipher.seal(secret.as_bytes()).await?;
        self.store
            .put_admin_secret(name.to_owned(), nonce.to_vec(), ciphertext)
            .await
    }

    pub async fn get_admin_secret(&self, name: &str) -> Result<Option<String>> {
        let Some((nonce, ciphertext)) = self.store.admin_secret(name.to_owned()).await? else {
            return Ok(None);
        };
        let plain = self
            .cipher
            .open(&nonce, &ciphertext)
            .await
            .with_context(|| format!("decrypt admin secret {name}"))?;
        Ok(Some(
            String::from_utf8(plain).with_context(|| format!("decode admin secret {name}"))?,
        ))
    }

    pub async fn admin_secret_present(&self, name: &str) -> Result<bool> {
        Ok(self.get_admin_secret(name).await?.is_some())
    }

    pub async fn raw_admin_ciphertext(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.store.raw_admin_ciphertext(name.to_owned()).await
    }

    pub async fn put_capability_state(&self, state: &CapabilitySecurityState) -> Result<()> {
        self.store.put_capability_state(state.clone()).await
    }
    pub async fn get_capability_state(
        &self,
        name: &str,
    ) -> Result<Option<CapabilitySecurityState>> {
        self.store.get_capability_state(name.to_owned()).await
    }
    pub async fn list_capability_states(&self) -> Result<Vec<CapabilitySecurityState>> {
        self.store.list_capability_states().await
    }
    pub async fn delete_capability_state(&self, name: &str) -> Result<()> {
        self.store.delete_capability_state(name.to_owned()).await
    }

    pub async fn put_management_request(&self, record: &ManagementRequestRecord) -> Result<()> {
        let _writer = self.management_write.lock().await;
        self.persist_management_request(record).await
    }
    async fn persist_management_request(&self, record: &ManagementRequestRecord) -> Result<()> {
        self.put_secret_material(
            "management_request_v1",
            &record.request_id,
            &serde_json::to_string(record)?,
        )
        .await
    }
    pub async fn get_management_request(
        &self,
        id: &str,
    ) -> Result<Option<ManagementRequestRecord>> {
        self.get_secret_material("management_request_v1", id)
            .await?
            .map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }
    pub async fn list_management_requests(&self) -> Result<Vec<ManagementRequestRecord>> {
        let mut records = self
            .list_secret_material_in_scope("management_request_v1")
            .await?
            .into_iter()
            .map(|json| serde_json::from_str::<ManagementRequestRecord>(&json))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        records.sort_by(|a, b| {
            (a.status != ManagementRequestStatus::Pending)
                .cmp(&(b.status != ManagementRequestStatus::Pending))
                .then(b.requested_at.as_secs().cmp(&a.requested_at.as_secs()))
                .then(b.request_id.cmp(&a.request_id))
        });
        Ok(records)
    }
    pub async fn latest_management_request_for_peer(
        &self,
        id: &NodeId,
    ) -> Result<Option<ManagementRequestRecord>> {
        Ok(self
            .list_management_requests()
            .await?
            .into_iter()
            .filter(|record| record.manager_node_id == *id || record.requester_node_id == *id)
            .max_by(|a, b| {
                a.requested_at
                    .as_secs()
                    .cmp(&b.requested_at.as_secs())
                    .then(a.request_id.cmp(&b.request_id))
            }))
    }
    pub async fn resolve_management_request(
        &self,
        id: &str,
        status: ManagementRequestStatus,
        at: UnixTimestamp,
        response: Option<&str>,
    ) -> Result<()> {
        let _writer = self.management_write.lock().await;
        if let Some(mut record) = self.get_management_request(id).await? {
            record.status = status;
            record.resolved_at = Some(at);
            record.response_message_id = response.map(str::to_owned);
            self.persist_management_request(&record).await?;
        }
        Ok(())
    }

    pub async fn put_node_setting(&self, key: &str, value: &str) -> Result<()> {
        self.store
            .put_node_setting(key.to_owned(), value.to_owned())
            .await
    }
    pub async fn get_node_setting(&self, key: &str) -> Result<Option<String>> {
        self.store.get_node_setting(key.to_owned()).await
    }
    pub async fn put_runtime_dedup(&self, key: &str, value: &str, ttl: Option<u64>) -> Result<()> {
        self.store
            .put_runtime_dedup(key.to_owned(), value.to_owned(), ttl)
            .await
    }
    pub async fn get_runtime_dedup(&self, key: &str) -> Result<Option<String>> {
        self.store.get_runtime_dedup(key.to_owned()).await
    }

    pub async fn put_secret_material(&self, scope: &str, name: &str, secret: &str) -> Result<()> {
        let _writer = self.material_write.lock().await;
        let index = self.material_names().await?;
        let public = if crate::public_state::cached_scope(scope) {
            Some(self.public_state().await?)
        } else {
            None
        };
        let bound;
        let plain = if protected_state_scope(scope) {
            bound = format!(
                "{STATE_PREFIX}{}",
                serde_json::to_string(&(scope, name, secret))?
            );
            bound.as_bytes()
        } else {
            secret.as_bytes()
        };
        let (nonce, ciphertext) = self.cipher.seal(plain).await?;
        self.store
            .put_secret_material(
                scope.to_owned(),
                name.to_owned(),
                nonce.to_vec(),
                ciphertext,
            )
            .await?;
        index
            .write()
            .map_err(|_| anyhow::anyhow!("material index poisoned"))?
            .insert((scope.to_owned(), name.to_owned()));
        if let Some(public) = public {
            let mut state = public
                .write()
                .map_err(|_| anyhow::anyhow!("public state poisoned"))?;
            state
                .records
                .insert((scope.into(), name.into()), secret.into());
            state.revision += 1;
        }
        Ok(())
    }
    pub async fn get_secret_material(&self, scope: &str, name: &str) -> Result<Option<String>> {
        if crate::public_state::cached_scope(scope) {
            return Ok(self
                .public_state()
                .await?
                .read()
                .map_err(|_| anyhow::anyhow!("public state poisoned"))?
                .records
                .get(&(scope.into(), name.into()))
                .cloned());
        }
        let Some((nonce, ciphertext)) = self
            .store
            .secret_material(scope.to_owned(), name.to_owned())
            .await?
        else {
            return Ok(None);
        };
        self.decrypt_value(scope, name, &nonce, &ciphertext)
            .await
            .map(Some)
    }
    pub async fn delete_secret_material(&self, scope: &str, name: &str) -> Result<()> {
        let _writer = self.material_write.lock().await;
        let index = self.material_names().await?;
        let public = if crate::public_state::cached_scope(scope) {
            Some(self.public_state().await?)
        } else {
            None
        };
        self.store
            .delete_secret_material(scope.to_owned(), name.to_owned())
            .await?;
        index
            .write()
            .map_err(|_| anyhow::anyhow!("material index poisoned"))?
            .remove(&(scope.to_owned(), name.to_owned()));
        if let Some(public) = public {
            let mut state = public
                .write()
                .map_err(|_| anyhow::anyhow!("public state poisoned"))?;
            state.records.remove(&(scope.into(), name.into()));
            state.revision += 1;
        }
        Ok(())
    }
    pub async fn list_secret_material_in_scope(&self, scope: &str) -> Result<Vec<String>> {
        Ok(self
            .list_secret_material_named_in_scope(scope)
            .await?
            .into_iter()
            .map(|(_, value)| value)
            .collect())
    }
    pub async fn list_secret_material_named_in_scope(
        &self,
        scope: &str,
    ) -> Result<Vec<(String, String)>> {
        if crate::public_state::cached_scope(scope) {
            return Ok(self
                .public_state()
                .await?
                .read()
                .map_err(|_| anyhow::anyhow!("public state poisoned"))?
                .records
                .iter()
                .filter(|((stored_scope, _), _)| stored_scope == scope)
                .map(|((_, name), value)| (name.clone(), value.clone()))
                .collect());
        }
        let rows = self.store.named_secret_material(scope.to_owned()).await?;
        let records = rows
            .iter()
            .map(|(_, nonce, ciphertext)| crate::VaultCiphertext { nonce, ciphertext })
            .collect::<Vec<_>>();
        let plain = self.cipher.open_batch(&records).await?;
        anyhow::ensure!(
            plain.len() == rows.len(),
            "invalid vault batch result count"
        );
        rows.into_iter()
            .zip(plain)
            .map(|((name, _, _), value)| Ok((name.clone(), decode_material(scope, &name, value)?)))
            .collect()
    }
    pub async fn list_secret_material_named_in_scopes_with_prefix(
        &self,
        prefix: &str,
        name: &str,
    ) -> Result<Vec<String>> {
        let rows = self
            .store
            .named_secret_material_by_scope_prefix(prefix.to_owned(), name.to_owned())
            .await?;
        let records = rows
            .iter()
            .map(|(_, nonce, ciphertext)| crate::VaultCiphertext { nonce, ciphertext })
            .collect::<Vec<_>>();
        let plain = self.cipher.open_batch(&records).await?;
        anyhow::ensure!(
            plain.len() == rows.len(),
            "invalid vault batch result count"
        );
        rows.into_iter()
            .zip(plain)
            .map(|((scope, _, _), value)| decode_material(&scope, name, value))
            .collect()
    }
    pub async fn list_secret_material_scopes_with_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<String>> {
        self.store
            .secret_scopes_with_prefix(prefix.to_owned())
            .await
    }
    async fn decrypt_value(
        &self,
        scope: &str,
        name: &str,
        nonce: &[u8],
        cipher: &[u8],
    ) -> Result<String> {
        let plain = self
            .cipher
            .open(nonce, cipher)
            .await
            .with_context(|| format!("decrypt secret material {scope}/{name}"))?;
        decode_material(scope, name, plain)
    }

    pub(crate) async fn material_names(
        &self,
    ) -> Result<&std::sync::RwLock<std::collections::HashSet<(String, String)>>> {
        self.material_index
            .get_or_try_init(|| async {
                Ok(std::sync::RwLock::new(
                    self.store
                        .secret_material_keys()
                        .await?
                        .into_iter()
                        .collect(),
                ))
            })
            .await
    }
    /// Presence is public metadata. Actual consumers still authenticate keys.
    pub async fn secret_material_present(&self, scope: &str, name: &str) -> Result<bool> {
        Ok(self
            .material_names()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("material index poisoned"))?
            .contains(&(scope.to_owned(), name.to_owned())))
    }
}
