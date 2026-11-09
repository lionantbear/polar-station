use super::*;
use rusqlite::{params, OptionalExtension, Row};
fn decode(r: &Row<'_>) -> rusqlite::Result<ManagedNodeRecord> {
    let peer: String = polar_sqlite_runtime::row::get(r, 0)?;
    let by: String = polar_sqlite_runtime::row::get(r, 2)?;
    Ok(ManagedNodeRecord {
        peer_node_id: NodeId::from_hex(&peer).map_err(sql_conversion)?,
        recorded_at: UnixTimestamp::from_secs(polar_sqlite_runtime::row::get::<i64>(r, 1)? as u64),
        recorded_by: NodeId::from_hex(&by).map_err(sql_conversion)?,
    })
}
impl VaultSqliteStore {
    pub async fn put_managed_node(&self, r: &ManagedNodeRecord) -> Result<()> {
        let (r, now) = (r.clone(), unix_now());
        self.runtime
            .write_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.put_managed_node",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.execute(
                        CANONICAL_SQL_F25FADD628C2,
                        params![
                            r.peer_node_id.to_hex(),
                            r.recorded_at.as_secs() as i64,
                            r.recorded_by.to_hex(),
                            now
                        ],
                    )?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
    pub async fn get_managed_node(&self, id: &NodeId) -> Result<Option<ManagedNodeRecord>> {
        let id = id.to_hex();
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.get_managed_node").expect("stable storage operation ID"), move|db|db.query_row("SELECT peer_node_id,recorded_at,recorded_by FROM managed_nodes WHERE peer_node_id=?1",[id],decode).optional()).await.map_err(Into::into)
    }
    pub async fn list_managed_nodes(&self) -> Result<Vec<ManagedNodeRecord>> {
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.list_managed_nodes").expect("stable storage operation ID"), |db|db.prepare_cached("SELECT peer_node_id,recorded_at,recorded_by FROM managed_nodes ORDER BY peer_node_id")?.query_map([],decode)?.collect()).await.map_err(Into::into)
    }
    pub async fn revoke_managed_node(&self, id: &NodeId) -> Result<()> {
        let id = id.to_hex();
        self.runtime
            .write_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.revoke_managed_node",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.execute(CANONICAL_SQL_0B62D423212F, [id])?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
}
// storage-operation-catalog: vault.managed.node
// Canonical persistence query literals for Archive15 P5.

const CANONICAL_SQL_0B62D423212F: &str = r"DELETE FROM managed_nodes WHERE peer_node_id=?1";
const CANONICAL_SQL_F25FADD628C2: &str = r"INSERT INTO managed_nodes(peer_node_id,recorded_at,recorded_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?4) ON CONFLICT(peer_node_id) DO UPDATE SET recorded_at=excluded.recorded_at,recorded_by=excluded.recorded_by,updated_at=excluded.updated_at";
