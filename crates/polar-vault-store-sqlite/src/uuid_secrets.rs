//! Atomic UUID persistence in the existing Vault database. No plaintext here.
use super::*;
use polar_vault::{SecretId, SecretKind, SecretMetadata, SecretQuery, StoredSecret};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

fn operation(name: &str) -> polar_storage_core::StorageOperationId {
    polar_storage_core::StorageOperationId::new(format!("storage.polar-vault-store-sqlite.{name}"))
        .expect("stable UUID storage operation")
}
fn read_id(value: String) -> rusqlite::Result<SecretId> {
    SecretId::from_uuid(Uuid::parse_str(&value).map_err(sql_conversion)?)
        .map_err(|_| sql_conversion(std::io::Error::other("invalid secret UUID")))
}
fn sql_integer(value: u64) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}
fn read_positive(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = polar_sqlite_runtime::row::get(row, index)?;
    let value = u64::try_from(value).map_err(sql_conversion)?;
    if value == 0 {
        return Err(sql_conversion(std::io::Error::other(
            "invalid credential counter",
        )));
    }
    Ok(value)
}
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSecret> {
    let id = read_id(row.get(0)?)?;
    let owner: String = row.get(1)?;
    let kind = SecretKind::try_from(row.get::<_, u16>(2)?)
        .map_err(|_| sql_conversion(std::io::Error::other("invalid secret kind")))?;
    let vault_id: String = row.get(6)?;
    Ok(StoredSecret {
        metadata: SecretMetadata {
            id,
            name: row.get(10)?,
            owner: NodeId::from_hex(&owner).map_err(sql_conversion)?,
            kind,
            version: read_positive(row, 3)?,
            revision: read_positive(row, 4)?,
            revoked: row.get(5)?,
            device_only: row.get(11)?,
        },
        vault_id: Uuid::parse_str(&vault_id).map_err(sql_conversion)?,
        key_generation: read_positive(row, 7)?,
        nonce: row.get(8)?,
        ciphertext: row.get(9)?,
    })
}
fn write_version(tx: &rusqlite::Transaction<'_>, r: &StoredSecret) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO vault_secret_version(secret_id,version,vault_id,key_generation,nonce,ciphertext) VALUES(?1,?2,?3,?4,?5,?6)",
        params![r.metadata.id.to_string(),sql_integer(r.metadata.version)?,r.vault_id.to_string(),sql_integer(r.key_generation)?,r.nonce,r.ciphertext])?;
    Ok(())
}
pub(super) fn write_key_directory(
    db: &rusqlite::Connection,
    owner: NodeId,
    expected: u64,
    record: &polar_vault::keys::directory::StoredDirectory,
) -> rusqlite::Result<bool> {
    let expected = sql_integer(expected)?;
    let revision = sql_integer(record.revision)?;
    if expected == 0 {
        Ok(db.execute("INSERT INTO vault_key_directory(owner_node_id,revision,nonce,ciphertext) VALUES(?1,?2,?3,?4) ON CONFLICT(owner_node_id) DO NOTHING",params![owner.to_hex(),revision,record.nonce,record.ciphertext])?==1)
    } else {
        Ok(db.execute("UPDATE vault_key_directory SET revision=?1,nonce=?2,ciphertext=?3 WHERE owner_node_id=?4 AND revision=?5",params![revision,record.nonce,record.ciphertext,owner.to_hex(),expected])?==1)
    }
}
fn write_initial(
    tx: &rusqlite::Transaction<'_>,
    record: &StoredSecret,
    import: Uuid,
) -> rusqlite::Result<()> {
    let m = &record.metadata;
    tx.execute("INSERT INTO vault_secret(secret_id,owner_node_id,kind,current_version,revision,revoked,name,device_only) VALUES(?1,?2,?3,1,1,0,?4,?5)",params![m.id.to_string(),m.owner.to_hex(),m.kind as u16,m.name,m.device_only])?;
    write_version(tx, record)?;
    tx.execute(
        "INSERT INTO vault_secret_import(owner_node_id,import_id,secret_id) VALUES(?1,?2,?3)",
        params![m.owner.to_hex(), import.to_string(), m.id.to_string()],
    )?;
    Ok(())
}
fn validate_initial(record: &StoredSecret, import: Uuid) -> Result<()> {
    anyhow::ensure!(
        record.metadata.version == 1
            && record.metadata.revision == 1
            && !record.metadata.revoked
            && !import.is_nil(),
        "invalid initial credential record"
    );
    record.authenticated_context()?;
    Ok(())
}
impl VaultSqliteStore {
    pub(super) async fn uuid_query(
        &self,
        owner: NodeId,
        query: SecretQuery,
    ) -> Result<Vec<SecretMetadata>> {
        anyhow::ensure!(
            (1..=100).contains(&query.limit),
            "invalid credential query limit"
        );
        Ok(self.runtime.read_with_operation(operation("query_uuid_secrets"), move |db| {
            let mut statement = db.prepare_cached("SELECT secret_id,name,kind,current_version,revision,revoked,device_only FROM vault_secret WHERE owner_node_id=?1 AND (?2 IS NULL OR secret_id=?2) AND (?3 IS NULL OR instr(name,?3)>0) AND (?4 IS NULL OR kind=?4) AND (?5=1 OR revoked=0) AND (?6 IS NULL OR secret_id>?6) ORDER BY secret_id LIMIT ?7")?;
            let rows = statement.query_map(params![owner.to_hex(),query.id.map(|id|id.to_string()),
                query.name_contains,query.kind.map(|kind|kind as u16),query.include_revoked,
                query.after.map(|id|id.to_string()),i64::from(query.limit)+1], |row| {
                Ok(SecretMetadata { id: read_id(row.get(0)?)?, name: row.get(1)?, owner,
                    kind: SecretKind::try_from(row.get::<_,u16>(2)?)
                        .map_err(|_|sql_conversion(std::io::Error::other("invalid secret kind")))?,
                    version: read_positive(row,3)?, revision: read_positive(row,4)?, revoked: row.get(5)?, device_only:row.get(6)? })
            })?;
            rows.collect()
        }).await?)
    }
    pub(super) async fn uuid_identity(&self) -> Result<Uuid> {
        let value = self.runtime.transaction_with_operation(operation("uuid_vault_identity"), |tx| {
            let proposed = polar_identity::generation::new_uuid_v7().to_string();
            tx.execute("INSERT INTO vault_meta(key,value,updated_at) VALUES('uuid_vault_id',?1,?2) ON CONFLICT(key) DO NOTHING",params![proposed,unix_now()])?;
            tx.query_row("SELECT value FROM vault_meta WHERE key='uuid_vault_id'", [], |row| row.get::<_,String>(0))
        }).await?;
        let id = Uuid::parse_str(&value)?;
        anyhow::ensure!(!id.is_nil(), "invalid Vault identity");
        Ok(id)
    }
    pub(super) async fn uuid_load(&self, id: SecretId) -> Result<Option<StoredSecret>> {
        Ok(self.runtime.read_with_operation(operation("load_uuid_secret"), move |db| {
            db.query_row("SELECT s.secret_id,s.owner_node_id,s.kind,s.current_version,s.revision,s.revoked,v.vault_id,v.key_generation,v.nonce,v.ciphertext,s.name,s.device_only FROM vault_secret s JOIN vault_secret_version v ON v.secret_id=s.secret_id AND v.version=s.current_version WHERE s.secret_id=?1",
                [id.to_string()], decode).optional()
        }).await?)
    }
    pub(super) async fn uuid_import_lookup(
        &self,
        owner: NodeId,
        import: Uuid,
    ) -> Result<Option<SecretId>> {
        Ok(self.runtime.read_with_operation(operation("find_uuid_import"), move |db| {
            db.query_row("SELECT secret_id FROM vault_secret_import WHERE owner_node_id=?1 AND import_id=?2",
                params![owner.to_hex(),import.to_string()], |row| read_id(row.get(0)?)).optional()
        }).await?)
    }
    pub(super) async fn uuid_insert(&self, record: StoredSecret, import: Uuid) -> Result<SecretId> {
        validate_initial(&record, import)?;
        Ok(self.runtime.transaction_with_operation(operation("insert_uuid_secret"), move |tx| {
            let m = &record.metadata;
            let existing = tx.query_row("SELECT secret_id FROM vault_secret_import WHERE owner_node_id=?1 AND import_id=?2",
                params![m.owner.to_hex(),import.to_string()], |row| read_id(row.get(0)?)).optional()?;
            if let Some(id) = existing { return Ok(id); }
            write_initial(tx,&record,import)?;
            Ok(m.id)
        }).await?)
    }
    pub(super) async fn uuid_create_with_directory(
        &self,
        secret: StoredSecret,
        import: Uuid,
        expected: u64,
        directory: polar_vault::keys::directory::StoredDirectory,
    ) -> Result<bool> {
        validate_initial(&secret, import)?;
        anyhow::ensure!(
            expected.checked_add(1) == Some(directory.revision),
            "invalid directory revision"
        );
        Ok(self
            .runtime
            .transaction_with_operation(operation("create_uuid_secret_with_directory"), move |tx| {
                if !write_key_directory(tx, secret.metadata.owner, expected, &directory)? {
                    return Ok(false);
                }
                write_initial(tx, &secret, import)?;
                Ok(true)
            })
            .await?)
    }
    pub(super) async fn uuid_rotate(&self, record: StoredSecret, expected: u64) -> Result<bool> {
        record.authenticated_context()?;
        anyhow::ensure!(
            record.metadata.version > 1
                && !record.metadata.revoked
                && expected.checked_add(1) == Some(record.metadata.revision),
            "invalid rotated credential record"
        );
        Ok(self.runtime.transaction_with_operation(operation("rotate_uuid_secret"), move |tx| {
            let m = &record.metadata;
            let changed = tx.execute("UPDATE vault_secret SET current_version=?1,revision=?2 WHERE secret_id=?3 AND owner_node_id=?4 AND kind=?5 AND revision=?6 AND revoked=0 AND current_version=?7 AND device_only=?8",
                params![sql_integer(m.version)?,sql_integer(m.revision)?,m.id.to_string(),m.owner.to_hex(),m.kind as u16,sql_integer(expected)?,sql_integer(m.version-1)?,m.device_only])?;
            if changed == 0 { return Ok(false); }
            write_version(tx, &record)?;
            Ok(true)
        }).await?)
    }
    pub(super) async fn uuid_revoke(
        &self,
        id: SecretId,
        owner: NodeId,
        expected: u64,
    ) -> Result<bool> {
        anyhow::ensure!(expected < i64::MAX as u64, "invalid secret revision");
        Ok(self.runtime.write_with_operation(operation("revoke_uuid_secret"), move |db| {
            Ok(db.execute("UPDATE vault_secret SET revoked=1,revision=revision+1 WHERE secret_id=?1 AND owner_node_id=?2 AND revision=?3 AND revoked=0",
                params![id.to_string(),owner.to_hex(),sql_integer(expected)?])? == 1)
        }).await?)
    }
}
