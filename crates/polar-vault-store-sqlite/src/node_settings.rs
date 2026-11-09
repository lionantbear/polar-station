use super::*;
use rusqlite::{params, OptionalExtension};
impl VaultSqliteStore {
    pub async fn put_node_setting(&self, key: &str, value: &str) -> Result<()> {
        let (key, value, now) = (key.to_owned(), value.to_owned(), unix_now());
        self.runtime.write_with_operation(polar_storage_core::StorageOperationId::new("storage.polar-vault-store-sqlite.put_node_setting").expect("stable storage operation ID"), move|db|{db.execute("INSERT INTO node_settings(setting_key,setting_value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(setting_key) DO UPDATE SET setting_value=excluded.setting_value,updated_at=excluded.updated_at",params![key,value,now])?;Ok(())}).await?;
        Ok(())
    }
    pub async fn get_node_setting(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        self.runtime
            .read_with_operation(
                polar_storage_core::StorageOperationId::new(
                    "storage.polar-vault-store-sqlite.get_node_setting",
                )
                .expect("stable storage operation ID"),
                move |db| {
                    db.query_row(
                        "SELECT setting_value FROM node_settings WHERE setting_key=?1",
                        [key],
                        |r| polar_sqlite_runtime::row::get(r, 0),
                    )
                    .optional()
                },
            )
            .await
            .map_err(Into::into)
    }
}
// storage-operation-catalog: vault.node.settings
