use super::*;
use rusqlite::{params, OptionalExtension};
impl VaultSqliteStore {
    pub async fn put_runtime_dedup(&self, key: &str, value: &str, ttl: Option<u64>) -> Result<()> {
        let now = unix_now();
        let (key, value, expires) = (
            key.to_owned(),
            value.to_owned(),
            ttl.map(|v| now + v as i64),
        );
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_runtime_dedup").expect("stable storage operation ID"), move|db|{db.execute("INSERT INTO runtime_dedup(dedup_key,observed_value,expires_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?4) ON CONFLICT(dedup_key) DO UPDATE SET observed_value=excluded.observed_value,expires_at=excluded.expires_at,updated_at=excluded.updated_at",params![key,value,expires,now])?;Ok(())}).await?;
        Ok(())
    }
    pub async fn get_runtime_dedup(&self, key: &str) -> Result<Option<String>> {
        let (key, now) = (key.to_owned(), unix_now());
        self.runtime
            .transaction_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.get_runtime_dedup",
                )
                .expect("stable storage operation ID"),
                move |tx| {
                    let row = tx
                    .query_row(
                        "SELECT observed_value,expires_at FROM runtime_dedup WHERE dedup_key=?1",
                        [&key],
                        |r| Ok((polar_sqlite_runtime::row::get::<String>(r, 0)?, polar_sqlite_runtime::row::get::<Option<i64>>(r, 1)?)),
                    )
                    .optional()?;
                    match row {
                        Some((_, Some(expiry))) if expiry < now => {
                            tx.execute("DELETE FROM runtime_dedup WHERE dedup_key=?1", [key])?;
                            Ok(None)
                        }
                        Some((value, _)) => Ok(Some(value)),
                        None => Ok(None),
                    }
                },
            )
            .await
            .map_err(Into::into)
    }
}
// storage-operation-catalog: vault.runtime.dedup
