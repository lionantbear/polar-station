use super::*;
use rusqlite::{params, OptionalExtension, Row};
fn state_error(kind: &str, value: &str) -> rusqlite::Error {
    sql_conversion(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("invalid {kind} trust state {value}"),
    ))
}
fn decode(r: &Row<'_>) -> rusqlite::Result<TrustRecord> {
    let id: String = polar_sqlite_runtime::row::get(r, 0)?;
    let name: String = polar_sqlite_runtime::row::get(r, 1)?;
    let key: String = polar_sqlite_runtime::row::get(r, 2)?;
    let out: String = polar_sqlite_runtime::row::get(r, 3)?;
    let input: String = polar_sqlite_runtime::row::get(r, 4)?;
    Ok(TrustRecord {
        peer_node_id: NodeId::from_hex(&id).map_err(sql_conversion)?,
        peer_node_name: NodeName::parse(&name).map_err(sql_conversion)?,
        peer_binding_pubkey: BindingPublicKey::from_hex(&key).map_err(sql_conversion)?,
        outbound: match out.as_str() {
            "none" => OutboundTrustState::None,
            "pending" => OutboundTrustState::Pending,
            "trusted" => OutboundTrustState::Trusted,
            "revoked" => OutboundTrustState::Revoked,
            _ => return Err(state_error("outbound", &out)),
        },
        inbound: match input.as_str() {
            "none" => InboundTrustState::None,
            "pending" => InboundTrustState::Pending,
            "trusted" => InboundTrustState::Trusted,
            "revoked" => InboundTrustState::Revoked,
            _ => return Err(state_error("inbound", &input)),
        },
        identity_change_pending: polar_sqlite_runtime::row::get::<i64>(r, 5)? != 0,
        approved_at: polar_sqlite_runtime::row::get::<Option<i64>>(r, 6)?
            .map(|v| UnixTimestamp::from_secs(v as u64)),
        revoked_at: polar_sqlite_runtime::row::get::<Option<i64>>(r, 7)?
            .map(|v| UnixTimestamp::from_secs(v as u64)),
    })
}
impl VaultSqliteStore {
    pub async fn put_trust_record(&self, r: &TrustRecord) -> Result<()> {
        let (r, now) = (r.clone(), unix_now());
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_trust_record").expect("stable storage operation ID"), move|db|{db.execute("INSERT INTO trust_nodes(peer_node_id,peer_node_name,binding_pubkey,outbound_state,inbound_state,identity_change_pending,approved_at,revoked_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9) ON CONFLICT(peer_node_id) DO UPDATE SET peer_node_name=excluded.peer_node_name,binding_pubkey=excluded.binding_pubkey,outbound_state=excluded.outbound_state,inbound_state=excluded.inbound_state,identity_change_pending=excluded.identity_change_pending,approved_at=excluded.approved_at,revoked_at=excluded.revoked_at,updated_at=excluded.updated_at",params![r.peer_node_id.to_hex(),r.peer_node_name.as_str(),r.peer_binding_pubkey.to_hex(),encode_outbound(r.outbound),encode_inbound(r.inbound),r.identity_change_pending,r.approved_at.map(|v|v.as_secs() as i64),r.revoked_at.map(|v|v.as_secs() as i64),now])?;Ok(())}).await?;
        Ok(())
    }
    pub async fn get_trust_record(&self, id: &NodeId) -> Result<Option<TrustRecord>> {
        let id = id.to_hex();
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.get_trust_record").expect("stable storage operation ID"), move|db|db.query_row("SELECT peer_node_id,peer_node_name,binding_pubkey,outbound_state,inbound_state,identity_change_pending,approved_at,revoked_at FROM trust_nodes WHERE peer_node_id=?1",[id],decode).optional()).await.map_err(Into::into)
    }
    pub async fn list_trust_records(&self) -> Result<Vec<TrustRecord>> {
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.list_trust_records").expect("stable storage operation ID"), |db|db.prepare_cached("SELECT peer_node_id,peer_node_name,binding_pubkey,outbound_state,inbound_state,identity_change_pending,approved_at,revoked_at FROM trust_nodes ORDER BY peer_node_name")?.query_map([],decode)?.collect()).await.map_err(Into::into)
    }
    pub async fn count_trust_records(&self) -> Result<i64> {
        self.runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.count_trust_records",
                )
                .expect("stable storage operation ID"),
                |db| {
                    db.query_row("SELECT COUNT(*) FROM trust_nodes", [], |r| {
                        polar_sqlite_runtime::row::get(r, 0)
                    })
                },
            )
            .await
            .map_err(Into::into)
    }
}
// storage-operation-catalog: vault.trust.state
