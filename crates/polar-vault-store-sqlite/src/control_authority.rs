use super::*;
use rusqlite::{params, OptionalExtension, Row};
fn decode(r: &Row<'_>) -> rusqlite::Result<ControlAuthorityRecord> {
    let peer: String = polar_sqlite_runtime::row::get(r, 0)?;
    let by: String = polar_sqlite_runtime::row::get(r, 2)?;
    let scope: String = polar_sqlite_runtime::row::get(r, 3)?;
    Ok(ControlAuthorityRecord {
        peer_node_id: NodeId::from_hex(&peer).map_err(sql_conversion)?,
        granted_at: UnixTimestamp::from_secs(polar_sqlite_runtime::row::get::<i64>(r, 1)? as u64),
        granted_by: NodeId::from_hex(&by).map_err(sql_conversion)?,
        scope: match scope.as_str() {
            "read_only" => ControlScope::ReadOnly,
            "full" => ControlScope::Full,
            _ => {
                return Err(sql_conversion(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid control scope {scope}"),
                )))
            }
        },
    })
}
fn put_authority(
    tx: &rusqlite::Transaction<'_>,
    r: &ControlAuthorityRecord,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO control_authority(peer_node_id,granted_at,granted_by,scope,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(peer_node_id) DO UPDATE SET granted_at=excluded.granted_at,granted_by=excluded.granted_by,scope=excluded.scope,updated_at=excluded.updated_at",params![r.peer_node_id.to_hex(),r.granted_at.as_secs() as i64,r.granted_by.to_hex(),encode_control_scope(r.scope),now])?;
    Ok(())
}
impl VaultSqliteStore {
    pub async fn put_control_authority(&self, r: &ControlAuthorityRecord) -> Result<()> {
        let (r, now) = (r.clone(), unix_now());
        self.runtime
            .transaction_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.put_control_authority",
                )
                .expect("stable storage operation ID"),
                move |tx| put_authority(tx, &r, now),
            )
            .await?;
        Ok(())
    }
    pub async fn get_control_authority(
        &self,
        id: &NodeId,
    ) -> Result<Option<ControlAuthorityRecord>> {
        let id = id.to_hex();
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.get_control_authority").expect("stable storage operation ID"), move|db|db.query_row("SELECT peer_node_id,granted_at,granted_by,scope FROM control_authority WHERE peer_node_id=?1",[id],decode).optional()).await.map_err(Into::into)
    }
    pub async fn revoke_control_authority(&self, id: &NodeId) -> Result<()> {
        let id = id.to_hex();
        self.runtime
            .write_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.revoke_control_authority",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.execute(CANONICAL_SQL_27AE43C2C3BE, [id])?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
    pub async fn materialize_control_bus_ready_peer_pair(
        &self,
        local: NodeId,
        peer: NodeId,
        at: UnixTimestamp,
        scope: ControlScope,
    ) -> Result<()> {
        self.runtime
            .transaction_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.materialize_control_bus_ready_peer_pair",
                )
                .expect("stable storage operation ID"),
                move |tx| {
                    let now = unix_now();
                    put_authority(
                        tx,
                        &ControlAuthorityRecord {
                            peer_node_id: peer,
                            granted_at: at,
                            granted_by: local,
                            scope,
                        },
                        now,
                    )?;
                    tx.execute(
                        CANONICAL_SQL_F25FADD628C2,
                        params![peer.to_hex(), at.as_secs() as i64, local.to_hex(), now],
                    )?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
    pub async fn clear_control_bus_ready_peer_pair(&self, peer: &NodeId) -> Result<()> {
        let peer = peer.to_hex();
        self.runtime
            .transaction_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.clear_control_bus_ready_peer_pair",
                )
                .expect("stable storage operation ID"),
                move |tx| {
                    tx.execute(CANONICAL_SQL_27AE43C2C3BE, [&peer])?;
                    tx.execute(CANONICAL_SQL_0B62D423212F, [peer])?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
}
// storage-operation-catalog: vault.control.authority
// Canonical persistence query literals for Archive15 P5.

const CANONICAL_SQL_0B62D423212F: &str = r"DELETE FROM managed_nodes WHERE peer_node_id=?1";
const CANONICAL_SQL_27AE43C2C3BE: &str = r"DELETE FROM control_authority WHERE peer_node_id=?1";
const CANONICAL_SQL_F25FADD628C2: &str = r"INSERT INTO managed_nodes(peer_node_id,recorded_at,recorded_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?4) ON CONFLICT(peer_node_id) DO UPDATE SET recorded_at=excluded.recorded_at,recorded_by=excluded.recorded_by,updated_at=excluded.updated_at";
