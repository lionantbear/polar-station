//! SQLite adapter for the driver-free `polar-vault` domain facade.
//! Implementation owns vault commands, queries, codecs and migrations.
// storage-operation-catalog: vault.repository
//!
//! Bootstrap supplies the shared runtime after applying the complete vault
//! migration plan. This adapter owns SQL and row mapping only.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use async_trait::async_trait;
use polar_identity::{BindingPublicKey, NodeId, NodeName, UnixTimestamp};
use polar_sqlite_runtime::{SqliteMigration, SqliteRuntime};
use polar_storage_migration::{MigrationDescriptor, StorageBackend, TransactionMode};
use polar_trust::{
    ControlAuthorityRecord, ControlScope, InboundTrustState, ManagedNodeRecord, OutboundTrustState,
    TrustRecord,
};
use polar_vault::{
    CapabilitySecurityState, ManagementRequestRecord, ManagementRequestStatus, VaultStore,
};

const INITIAL_MIGRATION_SQL: &str =
    include_str!("../../../migrations/sqlite/vault/V0001__initial.sql");
const LEGACY_REPAIR_MIGRATION_SQL: &str =
    include_str!("../../../migrations/sqlite/vault/V0002__legacy_management_request_repair.sql");
const UUID_SECRETS_SQL: &str =
    include_str!("../../../migrations/sqlite/vault/V0003__uuid_secrets.sql");
#[path = "uuid_secrets.rs"]
mod uuid_secrets;

#[derive(Clone)]
pub struct VaultSqliteStore {
    runtime: Arc<SqliteRuntime>,
}

impl VaultSqliteStore {
    pub fn from_runtime(runtime: Arc<SqliteRuntime>) -> Self {
        Self { runtime }
    }

    pub fn migrations() -> Vec<SqliteMigration> {
        vec![
            SqliteMigration {
                descriptor: MigrationDescriptor {
                    backend: StorageBackend::Sqlite,
                    component: "vault".into(),
                    version: 1,
                    name: "initial".into(),
                    path: "migrations/sqlite/vault/V0001__initial.sql".into(),
                    sha256: "726d43b71a5e6f4c7d0e47e79feece4373636fdefe5f4f0d2e31e53937293e7e"
                        .into(),
                    transaction_mode: TransactionMode::Required,
                },
                sql: INITIAL_MIGRATION_SQL,
                post_apply: None,
            },
            SqliteMigration {
                descriptor: MigrationDescriptor {
                    backend: StorageBackend::Sqlite,
                    component: "vault".into(),
                    version: 2,
                    name: "legacy_management_request_repair".into(),
                    path: "migrations/sqlite/vault/V0002__legacy_management_request_repair.sql"
                        .into(),
                    sha256: "8d79b67612960065c9faff1902f582df495ae3f33a238f78ed991cbf97508ab0"
                        .into(),
                    transaction_mode: TransactionMode::Required,
                },
                sql: LEGACY_REPAIR_MIGRATION_SQL,
                post_apply: Some(upgrade_legacy_columns),
            },
            SqliteMigration {
                descriptor: MigrationDescriptor {
                    backend: StorageBackend::Sqlite,
                    component: "vault".into(),
                    version: 3,
                    name: "uuid_secrets".into(),
                    path: "migrations/sqlite/vault/V0003__uuid_secrets.sql".into(),
                    sha256: "23f176fa92421346e6690f2e1d6b03352d1414ee4920df7736905688bb4a41e9"
                        .into(),
                    transaction_mode: TransactionMode::Required,
                },
                sql: UUID_SECRETS_SQL,
                post_apply: None,
            },
            SqliteMigration {
                descriptor: MigrationDescriptor {
                    backend: StorageBackend::Sqlite,
                    component: "vault".into(),
                    version: 4,
                    name: "credential_names".into(),
                    path: "migrations/sqlite/vault/V0004__credential_names.sql".into(),
                    sha256: "75d720e3ed28b171c2aff8c09074060f67cf3ace3b8a57ca4c13587fe9cbceac"
                        .into(),
                    transaction_mode: TransactionMode::Required,
                },
                sql: include_str!("../../../migrations/sqlite/vault/V0004__credential_names.sql"),
                post_apply: None,
            },
        ]
    }

    async fn put_admin_secret_row(
        &self,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()> {
        let now = unix_now();
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_admin_secret_row").expect("stable storage operation ID"), move |db| {
            db.execute(
                "INSERT INTO admin_secrets(secret_name,nonce,ciphertext,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(secret_name) DO UPDATE SET nonce=excluded.nonce,ciphertext=excluded.ciphertext,updated_at=excluded.updated_at",
                rusqlite::params![name, nonce, ciphertext, now],
            )?;
            Ok(())
        }).await?;
        Ok(())
    }

    async fn admin_secret_row(&self, name: String) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.admin_secret_row",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.query_row(
                        "SELECT nonce,ciphertext FROM admin_secrets WHERE secret_name=?1",
                        [name],
                        |row| {
                            Ok((
                                polar_sqlite_runtime::row::get(row, 0)?,
                                polar_sqlite_runtime::row::get(row, 1)?,
                            ))
                        },
                    )
                    .optional()
                },
            )
            .await?)
    }

    async fn raw_admin_ciphertext_row(&self, name: String) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.raw_admin_ciphertext_row",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.query_row(
                        "SELECT ciphertext FROM admin_secrets WHERE secret_name=?1",
                        [name],
                        |row| polar_sqlite_runtime::row::get(row, 0),
                    )
                    .optional()
                },
            )
            .await?)
    }

    async fn put_secret_material_row(
        &self,
        scope: String,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()> {
        let now = unix_now();
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_secret_material_row").expect("stable storage operation ID"), move |db| {
            db.execute(
                "INSERT INTO secret_material(secret_scope,secret_name,nonce,ciphertext,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(secret_scope,secret_name) DO UPDATE SET nonce=excluded.nonce,ciphertext=excluded.ciphertext,updated_at=excluded.updated_at",
                rusqlite::params![scope, name, nonce, ciphertext, now],
            )?;
            Ok(())
        }).await?;
        Ok(())
    }

    async fn secret_material_row(
        &self,
        scope: String,
        name: String,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        use rusqlite::OptionalExtension;
        Ok(self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.secret_material_row").expect("stable storage operation ID"), move |db| {
            db.query_row(
                "SELECT nonce,ciphertext FROM secret_material WHERE secret_scope=?1 AND secret_name=?2",
                rusqlite::params![scope, name],
                |row| Ok((polar_sqlite_runtime::row::get(row, 0)?, polar_sqlite_runtime::row::get(row, 1)?)),
            ).optional()
        }).await?)
    }

    async fn delete_secret_material_row(&self, scope: String, name: String) -> Result<()> {
        self.runtime
            .write_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.delete_secret_material_row",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.execute(
                        "DELETE FROM secret_material WHERE secret_scope=?1 AND secret_name=?2",
                        rusqlite::params![scope, name],
                    )?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }

    async fn named_secret_material_rows(
        &self,
        scope: String,
    ) -> Result<Vec<(String, Vec<u8>, Vec<u8>)>> {
        Ok(self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.named_secret_material_rows").expect("stable storage operation ID"), move |db| {
            db.prepare_cached("SELECT secret_name,nonce,ciphertext FROM secret_material WHERE secret_scope=?1")?
                .query_map([scope], |row| Ok((polar_sqlite_runtime::row::get(row, 0)?, polar_sqlite_runtime::row::get(row, 1)?, polar_sqlite_runtime::row::get(row, 2)?)))?
                .collect()
        }).await?)
    }

    async fn named_secret_material_by_scope_prefix_rows(
        &self,
        prefix: String,
        name: String,
    ) -> Result<Vec<(String, Vec<u8>, Vec<u8>)>> {
        let pattern = format!("{prefix}%");
        Ok(self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.named_secret_material_by_scope_prefix_rows").expect("stable storage operation ID"), move |db| {
            db.prepare_cached("SELECT secret_scope,nonce,ciphertext FROM secret_material WHERE secret_scope LIKE ?1 AND secret_name=?2")?
                .query_map(rusqlite::params![pattern, name], |row| Ok((polar_sqlite_runtime::row::get(row, 0)?, polar_sqlite_runtime::row::get(row, 1)?, polar_sqlite_runtime::row::get(row, 2)?)))?
                .collect()
        }).await?)
    }

    async fn secret_scopes_with_prefix_rows(&self, prefix: String) -> Result<Vec<String>> {
        let pattern = format!("{prefix}%");
        Ok(self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.secret_scopes_with_prefix_rows").expect("stable storage operation ID"), move |db| {
            db.prepare_cached("SELECT DISTINCT secret_scope FROM secret_material WHERE secret_scope LIKE ?1 ORDER BY secret_scope")?
                .query_map([pattern], |row| polar_sqlite_runtime::row::get(row, 0))?
                .collect()
        }).await?)
    }
}

#[async_trait]
impl VaultStore for VaultSqliteStore {
    async fn query_uuid_secrets(
        &self,
        owner: NodeId,
        query: polar_vault::SecretQuery,
    ) -> Result<Vec<polar_vault::SecretMetadata>> {
        self.uuid_query(owner, query).await
    }
    async fn uuid_vault_identity(&self) -> Result<uuid::Uuid> {
        self.uuid_identity().await
    }
    async fn load_uuid_secret(
        &self,
        id: polar_vault::SecretId,
    ) -> Result<Option<polar_vault::StoredSecret>> {
        self.uuid_load(id).await
    }
    async fn find_uuid_import(
        &self,
        owner: NodeId,
        import: uuid::Uuid,
    ) -> Result<Option<polar_vault::SecretId>> {
        self.uuid_import_lookup(owner, import).await
    }
    async fn insert_uuid_secret(
        &self,
        record: polar_vault::StoredSecret,
        import: uuid::Uuid,
    ) -> Result<polar_vault::SecretId> {
        self.uuid_insert(record, import).await
    }
    async fn rotate_uuid_secret(
        &self,
        record: polar_vault::StoredSecret,
        revision: u64,
    ) -> Result<bool> {
        self.uuid_rotate(record, revision).await
    }
    async fn revoke_uuid_secret(
        &self,
        id: polar_vault::SecretId,
        owner: NodeId,
        revision: u64,
    ) -> Result<bool> {
        self.uuid_revoke(id, owner, revision).await
    }
    async fn put_admin_secret(
        &self,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()> {
        self.put_admin_secret_row(name, nonce, ciphertext).await
    }
    async fn admin_secret(&self, name: String) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        self.admin_secret_row(name).await
    }
    async fn raw_admin_ciphertext(&self, name: String) -> Result<Option<Vec<u8>>> {
        self.raw_admin_ciphertext_row(name).await
    }
    async fn put_capability_state(&self, state: CapabilitySecurityState) -> Result<()> {
        VaultSqliteStore::put_capability_state(self, &state).await
    }
    async fn get_capability_state(&self, name: String) -> Result<Option<CapabilitySecurityState>> {
        VaultSqliteStore::get_capability_state(self, &name).await
    }
    async fn list_capability_states(&self) -> Result<Vec<CapabilitySecurityState>> {
        VaultSqliteStore::list_capability_states(self).await
    }
    async fn delete_capability_state(&self, name: String) -> Result<()> {
        VaultSqliteStore::delete_capability_state(self, &name).await
    }
    async fn put_control_authority(&self, record: ControlAuthorityRecord) -> Result<()> {
        VaultSqliteStore::put_control_authority(self, &record).await
    }
    async fn get_control_authority(&self, id: NodeId) -> Result<Option<ControlAuthorityRecord>> {
        VaultSqliteStore::get_control_authority(self, &id).await
    }
    async fn revoke_control_authority(&self, id: NodeId) -> Result<()> {
        VaultSqliteStore::revoke_control_authority(self, &id).await
    }
    async fn materialize_control_bus_ready_peer_pair(
        &self,
        local: NodeId,
        peer: NodeId,
        at: UnixTimestamp,
        scope: ControlScope,
    ) -> Result<()> {
        VaultSqliteStore::materialize_control_bus_ready_peer_pair(self, local, peer, at, scope)
            .await
    }
    async fn clear_control_bus_ready_peer_pair(&self, peer: NodeId) -> Result<()> {
        VaultSqliteStore::clear_control_bus_ready_peer_pair(self, &peer).await
    }
    async fn put_managed_node(&self, record: ManagedNodeRecord) -> Result<()> {
        VaultSqliteStore::put_managed_node(self, &record).await
    }
    async fn get_managed_node(&self, id: NodeId) -> Result<Option<ManagedNodeRecord>> {
        VaultSqliteStore::get_managed_node(self, &id).await
    }
    async fn list_managed_nodes(&self) -> Result<Vec<ManagedNodeRecord>> {
        VaultSqliteStore::list_managed_nodes(self).await
    }
    async fn revoke_managed_node(&self, id: NodeId) -> Result<()> {
        VaultSqliteStore::revoke_managed_node(self, &id).await
    }
    async fn put_management_request(&self, record: ManagementRequestRecord) -> Result<()> {
        VaultSqliteStore::put_management_request(self, &record).await
    }
    async fn get_management_request(&self, id: String) -> Result<Option<ManagementRequestRecord>> {
        VaultSqliteStore::get_management_request(self, &id).await
    }
    async fn list_management_requests(&self) -> Result<Vec<ManagementRequestRecord>> {
        VaultSqliteStore::list_management_requests(self).await
    }
    async fn latest_management_request_for_peer(
        &self,
        id: NodeId,
    ) -> Result<Option<ManagementRequestRecord>> {
        VaultSqliteStore::latest_management_request_for_peer(self, &id).await
    }
    async fn resolve_management_request(
        &self,
        id: String,
        status: ManagementRequestStatus,
        at: UnixTimestamp,
        response: Option<String>,
    ) -> Result<()> {
        VaultSqliteStore::resolve_management_request(self, &id, status, at, response.as_deref())
            .await
    }
    async fn put_node_setting(&self, key: String, value: String) -> Result<()> {
        VaultSqliteStore::put_node_setting(self, &key, &value).await
    }
    async fn get_node_setting(&self, key: String) -> Result<Option<String>> {
        VaultSqliteStore::get_node_setting(self, &key).await
    }
    async fn put_runtime_dedup(&self, key: String, value: String, ttl: Option<u64>) -> Result<()> {
        VaultSqliteStore::put_runtime_dedup(self, &key, &value, ttl).await
    }
    async fn get_runtime_dedup(&self, key: String) -> Result<Option<String>> {
        VaultSqliteStore::get_runtime_dedup(self, &key).await
    }
    async fn put_secret_material(
        &self,
        scope: String,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()> {
        self.put_secret_material_row(scope, name, nonce, ciphertext)
            .await
    }
    async fn insert_secret_material(
        &self,
        scope: String,
        name: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<()> {
        let now = unix_now();
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("vault.secret.initialize")?, move |db| {
            db.execute(
                "INSERT INTO secret_material(secret_scope,secret_name,nonce,ciphertext,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(secret_scope,secret_name) DO NOTHING",
                rusqlite::params![scope,name,nonce,ciphertext,now],
            )?;
            Ok(())
        }).await?;
        Ok(())
    }
    async fn secret_material(
        &self,
        scope: String,
        name: String,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        self.secret_material_row(scope, name).await
    }
    async fn delete_secret_material(&self, scope: String, name: String) -> Result<()> {
        self.delete_secret_material_row(scope, name).await
    }
    async fn named_secret_material(
        &self,
        scope: String,
    ) -> Result<Vec<(String, Vec<u8>, Vec<u8>)>> {
        self.named_secret_material_rows(scope).await
    }
    async fn named_secret_material_by_scope_prefix(
        &self,
        prefix: String,
        name: String,
    ) -> Result<Vec<(String, Vec<u8>, Vec<u8>)>> {
        self.named_secret_material_by_scope_prefix_rows(prefix, name)
            .await
    }
    async fn secret_scopes_with_prefix(&self, prefix: String) -> Result<Vec<String>> {
        self.secret_scopes_with_prefix_rows(prefix).await
    }
    async fn secret_material_keys(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new("vault.secret.material_keys")?,
                |db| {
                    db.prepare_cached("SELECT secret_scope,secret_name FROM secret_material")?
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect()
                },
            )
            .await?)
    }
    async fn put_trust_record(&self, record: TrustRecord) -> Result<()> {
        VaultSqliteStore::put_trust_record(self, &record).await
    }
    async fn get_trust_record(&self, id: NodeId) -> Result<Option<TrustRecord>> {
        VaultSqliteStore::get_trust_record(self, &id).await
    }
    async fn list_trust_records(&self) -> Result<Vec<TrustRecord>> {
        VaultSqliteStore::list_trust_records(self).await
    }
    async fn count_trust_records(&self) -> Result<i64> {
        VaultSqliteStore::count_trust_records(self).await
    }
}

fn upgrade_legacy_columns(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let columns = tx
        .prepare_cached("PRAGMA table_info(management_requests)")?
        .query_map([], |row| polar_sqlite_runtime::row::get::<String>(row, 1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, definition) in [
        ("target_channel_id", "target_channel_id TEXT"),
        ("target_thread_id", "target_thread_id TEXT"),
        ("target_inhouse_polar_id", "target_inhouse_polar_id TEXT"),
    ] {
        if !columns.iter().any(|column| column == name) {
            tx.execute_batch(&format!(
                "ALTER TABLE management_requests ADD COLUMN {definition}"
            ))?;
        }
    }
    tx.execute_batch("DROP TABLE IF EXISTS approved_management_peer_routes")?;
    Ok(())
}

fn sql_conversion(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
fn encode_outbound(value: OutboundTrustState) -> &'static str {
    match value {
        OutboundTrustState::None => "none",
        OutboundTrustState::Pending => "pending",
        OutboundTrustState::Trusted => "trusted",
        OutboundTrustState::Revoked => "revoked",
    }
}
fn encode_inbound(value: InboundTrustState) -> &'static str {
    match value {
        InboundTrustState::None => "none",
        InboundTrustState::Pending => "pending",
        InboundTrustState::Trusted => "trusted",
        InboundTrustState::Revoked => "revoked",
    }
}
fn encode_control_scope(value: ControlScope) -> &'static str {
    match value {
        ControlScope::ReadOnly => "read_only",
        ControlScope::Full => "full",
    }
}
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_secs() as i64
}

#[path = "capability_security.rs"]
mod capability_security;
#[path = "control_authority.rs"]
mod control_authority;
#[path = "managed_nodes.rs"]
mod managed_nodes;
#[path = "management_requests.rs"]
mod management_requests;
#[path = "node_settings.rs"]
mod node_settings;
#[path = "runtime_dedup.rs"]
mod runtime_dedup;
#[path = "trust_state.rs"]
mod trust_state;




