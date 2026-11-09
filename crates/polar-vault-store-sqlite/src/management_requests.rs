use super::*;
use rusqlite::{params, OptionalExtension, Row};
const SELECT:&str="SELECT request_id,requester_node_id,requester_node_name,manager_node_id,channel_id,thread_id,request_message_id,response_message_id,request_transport_plane,target_channel_id,target_thread_id,target_inhouse_polar_id,requested_scope,request_text,endpoint_hint,status,requested_at,resolved_at FROM management_requests";
fn invalid(message: String) -> rusqlite::Error {
    sql_conversion(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}
fn decode(r: &Row<'_>) -> rusqlite::Result<ManagementRequestRecord> {
    let requester: String = polar_sqlite_runtime::row::get(r, 1)?;
    let manager: String = polar_sqlite_runtime::row::get(r, 3)?;
    let scope: String = polar_sqlite_runtime::row::get(r, 12)?;
    let status: String = polar_sqlite_runtime::row::get(r, 15)?;
    Ok(ManagementRequestRecord {
        request_id: polar_sqlite_runtime::row::get(r, 0)?,
        requester_node_id: NodeId::from_hex(&requester).map_err(sql_conversion)?,
        requester_node_name: polar_sqlite_runtime::row::get(r, 2)?,
        manager_node_id: NodeId::from_hex(&manager).map_err(sql_conversion)?,
        channel_id: polar_sqlite_runtime::row::get(r, 4)?,
        thread_id: polar_sqlite_runtime::row::get(r, 5)?,
        request_message_id: polar_sqlite_runtime::row::get(r, 6)?,
        response_message_id: polar_sqlite_runtime::row::get(r, 7)?,
        request_transport_plane: polar_sqlite_runtime::row::get(r, 8)?,
        target_channel_id: polar_sqlite_runtime::row::get(r, 9)?,
        target_thread_id: polar_sqlite_runtime::row::get(r, 10)?,
        target_inhouse_polar_id: polar_sqlite_runtime::row::get(r, 11)?,
        requested_scope: match scope.as_str() {
            "read_only" => ControlScope::ReadOnly,
            "full" => ControlScope::Full,
            _ => return Err(invalid(format!("invalid requested control scope {scope}"))),
        },
        request_text: polar_sqlite_runtime::row::get(r, 13)?,
        endpoint_hint: polar_sqlite_runtime::row::get(r, 14)?,
        status: match status.as_str() {
            "pending" => ManagementRequestStatus::Pending,
            "approved" => ManagementRequestStatus::Approved,
            "rejected" => ManagementRequestStatus::Rejected,
            _ => {
                return Err(invalid(format!(
                    "invalid management request status {status}"
                )))
            }
        },
        requested_at: UnixTimestamp::from_secs(polar_sqlite_runtime::row::get::<i64>(r, 16)? as u64),
        resolved_at: polar_sqlite_runtime::row::get::<Option<i64>>(r, 17)?
            .map(|v| UnixTimestamp::from_secs(v as u64)),
    })
}
impl VaultSqliteStore {
    pub async fn put_management_request(&self, r: &ManagementRequestRecord) -> Result<()> {
        let (r, now) = (r.clone(), unix_now());
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_management_request").expect("stable storage operation ID"), move|db|{db.execute("INSERT INTO management_requests(request_id,requester_node_id,requester_node_name,manager_node_id,channel_id,thread_id,request_message_id,response_message_id,request_transport_plane,target_channel_id,target_thread_id,target_inhouse_polar_id,requested_scope,request_text,endpoint_hint,status,requested_at,resolved_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?19) ON CONFLICT(request_id) DO UPDATE SET requester_node_name=excluded.requester_node_name,manager_node_id=excluded.manager_node_id,channel_id=excluded.channel_id,thread_id=excluded.thread_id,request_message_id=excluded.request_message_id,response_message_id=excluded.response_message_id,request_transport_plane=excluded.request_transport_plane,target_channel_id=excluded.target_channel_id,target_thread_id=excluded.target_thread_id,target_inhouse_polar_id=excluded.target_inhouse_polar_id,requested_scope=excluded.requested_scope,request_text=excluded.request_text,endpoint_hint=excluded.endpoint_hint,status=excluded.status,requested_at=excluded.requested_at,resolved_at=excluded.resolved_at,updated_at=excluded.updated_at",params![r.request_id,r.requester_node_id.to_hex(),r.requester_node_name,r.manager_node_id.to_hex(),r.channel_id,r.thread_id,r.request_message_id,r.response_message_id,r.request_transport_plane,r.target_channel_id,r.target_thread_id,r.target_inhouse_polar_id,encode_control_scope(r.requested_scope),r.request_text,r.endpoint_hint,r.status.as_str(),r.requested_at.as_secs() as i64,r.resolved_at.map(|v|v.as_secs() as i64),now])?;Ok(())}).await?;
        Ok(())
    }
    pub async fn get_management_request(
        &self,
        id: &str,
    ) -> Result<Option<ManagementRequestRecord>> {
        let (id, sql) = (id.to_owned(), format!("{SELECT} WHERE request_id=?1"));
        self.runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.get_management_request",
                )
                .expect("stable storage operation ID"),
                move |db| db.query_row(&sql, [id], decode).optional(),
            )
            .await
            .map_err(Into::into)
    }
    pub async fn list_management_requests(&self) -> Result<Vec<ManagementRequestRecord>> {
        let sql=format!("{SELECT} ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END,requested_at DESC,request_id DESC");
        self.runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.list_management_requests",
                )
                .expect("stable storage operation ID"),
                move |db| db.prepare_cached(&sql)?.query_map([], decode)?.collect(),
            )
            .await
            .map_err(Into::into)
    }
    pub async fn latest_management_request_for_peer(
        &self,
        id: &NodeId,
    ) -> Result<Option<ManagementRequestRecord>> {
        let(id,sql)=(id.to_hex(),format!("{SELECT} WHERE requester_node_id=?1 OR manager_node_id=?1 ORDER BY requested_at DESC,request_id DESC LIMIT 1"));
        self.runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.latest_management_request_for_peer",
                )
                .expect("stable storage operation ID"),
                move |db| db.query_row(&sql, [id], decode).optional(),
            )
            .await
            .map_err(Into::into)
    }
    pub async fn resolve_management_request(
        &self,
        id: &str,
        status: ManagementRequestStatus,
        at: UnixTimestamp,
        response: Option<&str>,
    ) -> Result<()> {
        let (id, response, now) = (id.to_owned(), response.map(str::to_owned), unix_now());
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.resolve_management_request").expect("stable storage operation ID"), move|db|{db.execute("UPDATE management_requests SET status=?1,resolved_at=?2,response_message_id=?3,updated_at=?4 WHERE request_id=?5",params![status.as_str(),at.as_secs() as i64,response,now,id])?;Ok(())}).await?;
        Ok(())
    }
}
// storage-operation-catalog: vault.management.request
