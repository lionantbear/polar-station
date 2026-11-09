use super::*;
use rusqlite::{params, OptionalExtension, Row};
fn decode(r: &Row<'_>) -> rusqlite::Result<CapabilitySecurityState> {
    Ok(CapabilitySecurityState {
        name: polar_sqlite_runtime::row::get(r, 0)?,
        status: polar_sqlite_runtime::row::get(r, 1)?,
        version: polar_sqlite_runtime::row::get(r, 2)?,
        installed_at: polar_sqlite_runtime::row::get::<Option<i64>>(r, 3)?.map(|v| v as u64),
        last_healthcheck_at: polar_sqlite_runtime::row::get::<Option<i64>>(r, 4)?.map(|v| v as u64),
        auth_present: polar_sqlite_runtime::row::get::<i64>(r, 5)? != 0,
        auth_sha256: polar_sqlite_runtime::row::get(r, 6)?,
    })
}
impl VaultSqliteStore {
    pub async fn put_capability_state(&self, s: &CapabilitySecurityState) -> Result<()> {
        let (s, now) = (s.clone(), unix_now());
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_capability_state").expect("stable storage operation ID"), move|db|{db.execute("INSERT INTO capability_security_state(name,status,version,installed_at,last_healthcheck_at,auth_present,auth_sha256,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(name) DO UPDATE SET status=excluded.status,version=excluded.version,installed_at=excluded.installed_at,last_healthcheck_at=excluded.last_healthcheck_at,auth_present=excluded.auth_present,auth_sha256=excluded.auth_sha256,updated_at=excluded.updated_at",params![s.name,s.status,s.version,s.installed_at.map(|v|v as i64),s.last_healthcheck_at.map(|v|v as i64),s.auth_present,s.auth_sha256,now])?;Ok(())}).await?;
        Ok(())
    }
    pub async fn get_capability_state(
        &self,
        name: &str,
    ) -> Result<Option<CapabilitySecurityState>> {
        let name = name.to_owned();
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.get_capability_state").expect("stable storage operation ID"), move|db|db.query_row("SELECT name,status,version,installed_at,last_healthcheck_at,auth_present,auth_sha256 FROM capability_security_state WHERE name=?1",[name],decode).optional()).await.map_err(Into::into)
    }
    pub async fn list_capability_states(&self) -> Result<Vec<CapabilitySecurityState>> {
        self.runtime.read_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.list_capability_states").expect("stable storage operation ID"), |db|db.prepare_cached("SELECT name,status,version,installed_at,last_healthcheck_at,auth_present,auth_sha256 FROM capability_security_state ORDER BY name")?.query_map([],decode)?.collect()).await.map_err(Into::into)
    }
    pub async fn delete_capability_state(&self, name: &str) -> Result<()> {
        let name = name.to_owned();
        self.runtime
            .write_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.delete_capability_state",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.execute(
                        "DELETE FROM capability_security_state WHERE name=?1",
                        [name],
                    )?;
                    Ok(())
                },
            )
            .await?;
        Ok(())
    }
}
// storage-operation-catalog: vault.capability.security
