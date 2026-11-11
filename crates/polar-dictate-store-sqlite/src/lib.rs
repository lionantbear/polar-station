//! Durable audio acknowledgements and owner isolation using the shared SQLite runtime.
use anyhow::{ensure, Result};
use async_trait::async_trait;
use polar_dictate::{AudioBlock, CaptureState, Change, DictateStore, Recording, SaveReceipt};
use polar_sqlite_runtime::{SqliteDurability, SqliteMigration, SqliteRuntime, SqliteRuntimeConfig};
use polar_storage_migration::{MigrationDescriptor, StorageBackend, TransactionMode};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

mod history;
const DELETED_IMPORTS_SCHEMA: &str = include_str!("../migrations/004_deleted_imports.sql");
const LEGACY_RETENTION_SCHEMA: &str = include_str!("../migrations/003_legacy_retention.sql");
const HISTORY_SCHEMA: &str = include_str!("../migrations/002_history.sql");
const SCHEMA: &str = include_str!("../migrations/001_recordings.sql");
#[derive(Clone)]
pub struct SqliteDictateStore {
    db: SqliteRuntime,
    owner: String,
}
impl SqliteDictateStore {
    /// `owner` comes from authenticated Host composition, never request JSON.
    pub fn open(path: impl AsRef<std::path::Path>, owner: String) -> Result<Self> {
        ensure!(
            !owner.is_empty() && owner.len() <= 512,
            "dictate.invalid_owner"
        );
        let db = SqliteRuntime::open(
            path,
            SqliteRuntimeConfig {
                durability: SqliteDurability::Full,
                reader_workers: 1,
                ..Default::default()
            },
            vec![
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "polar-dictate".into(),
                        version: 1,
                        name: "recordings".into(),
                        path: "polar-dictate/001_recordings.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(SCHEMA)),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: SCHEMA,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "polar-dictate".into(),
                        version: 2,
                        name: "history".into(),
                        path: "polar-dictate/002_history.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(HISTORY_SCHEMA)),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: HISTORY_SCHEMA,
                    post_apply: Some(history::backfill),
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "polar-dictate".into(),
                        version: 3,
                        name: "legacy-retention".into(),
                        path: "polar-dictate/003_legacy_retention.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(LEGACY_RETENTION_SCHEMA)),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: LEGACY_RETENTION_SCHEMA,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "polar-dictate".into(),
                        version: 4,
                        name: "deleted-imports".into(),
                        path: "polar-dictate/004_deleted_imports.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(DELETED_IMPORTS_SCHEMA)),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: DELETED_IMPORTS_SCHEMA,
                    post_apply: None,
                },
            ],
        )?;
        Ok(Self { db, owner })
    }
}
fn sql_error(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(error.into())
}
fn get(db: &Connection, owner: &str, id: &str) -> Result<Recording> {
    let payload: String = db
        .query_row(
            "SELECT payload FROM dictate_recording WHERE owner=?1 AND id=?2",
            params![owner, id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("dictate.recording_missing"))?;
    Ok(serde_json::from_str(&payload)?)
}
fn update(db: &Connection, owner: &str, record: &Recording) -> Result<()> {
    ensure!(
        db.execute(
            "UPDATE dictate_recording SET payload=?3,capturing=?4 WHERE owner=?1 AND id=?2",
            params![
                owner,
                record.id,
                serde_json::to_string(record)?,
                record.capture == CaptureState::Recording
            ]
        )? == 1,
        "dictate.recording_missing"
    );
    history::index_record(db, owner, record)?;
    Ok(())
}
fn all(db: &Connection, owner: &str) -> Result<Vec<Recording>> {
    let mut statement =
        db.prepare("SELECT payload FROM dictate_recording WHERE owner=?1 AND recoverable=1")?;
    let values = statement
        .query_map([owner], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    values
        .iter()
        .map(|p| Ok(serde_json::from_str(p)?))
        .collect()
}
#[async_trait]
impl DictateStore for SqliteDictateStore {
    async fn create(&self, recording: Recording) -> Result<Recording> {
        // Only a fresh, validated record can be inserted through this port.
        let mut fresh = Recording::new(
            recording.id.clone(),
            recording.created_at_ms,
            recording.is_test,
        )?;
        fresh.set_reference(recording.reference.clone())?;
        fresh.set_project(recording.project.clone())?;
        // These are trusted Host configuration snapshots, not frontend state.
        fresh.transcription_mode = recording.transcription_mode;
        let settings = polar_dictate::VoiceSettings {
            glossary: recording.glossary.clone(),
            ..Default::default()
        };
        settings.validate()?;
        fresh.glossary = recording.glossary.clone();
        if recording.revision == 2 {
            fresh.apply(Change::Configure {
                cleanup: recording.cleanup_enabled,
                context: recording.context.clone(),
            })?;
        }
        fresh.output_mode = recording.output_mode;
        ensure!(
            serde_json::to_value(&fresh)? == serde_json::to_value(&recording)?,
            "dictate.invalid_initial_record"
        );
        let owner = self.owner.clone();
        Ok(self.db.transaction(move |tx| (|| -> Result<_> {
            if let Some(payload) = tx.query_row("SELECT payload FROM dictate_recording WHERE owner=?1 AND id=?2",
                params![owner, recording.id], |r| r.get::<_, String>(0)).optional()? {
                let existing: Recording = serde_json::from_str(&payload)?;
                ensure!(existing.created_at_ms == recording.created_at_ms && existing.is_test == recording.is_test
                    && existing.context_digest == recording.context_digest && existing.reference == recording.reference
                    && existing.project == recording.project,
                    "dictate.recording_id_conflict");
                return Ok(existing);
            }
            let text_exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dictate_text_edit WHERE owner=?1 AND id=?2)",params![owner,recording.id],|r|r.get(0))?;
            ensure!(!text_exists,"dictate.recording_id_conflict");
            tx.execute("INSERT INTO dictate_recording(owner,id,created_ms,capturing,payload) VALUES(?1,?2,?3,1,?4)",
                params![owner, recording.id, i64::try_from(recording.created_at_ms)?, serde_json::to_string(&recording)?])?;
            history::index_record(tx,&owner,&recording)?;
            Ok(recording)
        })().map_err(sql_error)).await?)
    }
    async fn get(&self, id: &str) -> Result<Recording> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self
            .db
            .read(move |db| get(db, &owner, &id).map_err(sql_error))
            .await?)
    }
    async fn usage(&self) -> Result<polar_dictate::StorageUsage> {
        let owner = self.owner.clone();
        let (recordings,text_edits,audio_bytes,history_items)=self.db.read(move |db|{
            db.query_row("SELECT (SELECT COUNT(*) FROM dictate_recording WHERE owner=?1),(SELECT COUNT(*) FROM dictate_text_edit WHERE owner=?1),(SELECT COALESCE(SUM(audio_bytes),0) FROM dictate_recording WHERE owner=?1),((SELECT COUNT(*) FROM dictate_recording WHERE owner=?1 AND visible=1)+(SELECT COUNT(*) FROM dictate_text_edit WHERE owner=?1))",[owner],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?)))
        }).await?;
        Ok(polar_dictate::StorageUsage {
            recordings: u64::try_from(recordings)?,
            text_edits: u64::try_from(text_edits)?,
            history_items: u64::try_from(history_items)?,
            audio_bytes: u64::try_from(audio_bytes)?,
        })
    }
    async fn list(&self, offset: u32, limit: u32) -> Result<Vec<Recording>> {
        let owner = self.owner.clone();
        let payloads = self.db.read(move |db| {
            let mut stmt = db.prepare("SELECT payload FROM dictate_recording WHERE owner=?1 ORDER BY created_ms DESC,id DESC LIMIT ?2 OFFSET ?3")?;
            let rows = stmt.query_map(params![owner,limit.clamp(1,200),offset], |r| r.get::<_,String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }).await?;
        payloads
            .iter()
            .map(|p| Ok(serde_json::from_str(p)?))
            .collect()
    }
    async fn append(&self, block: AudioBlock) -> Result<SaveReceipt> {
        let owner = self.owner.clone();
        Ok(self.db.transaction(move |tx| (|| -> Result<_> {
            let mut record = get(tx,&owner,&block.recording_id)?;
            let existing = tx.query_row("SELECT start_sample,pcm FROM dictate_audio_block WHERE owner=?1 AND recording_id=?2 AND sequence=?3",
                params![owner,block.recording_id,i64::try_from(block.sequence)?], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,Vec<u8>>(1)?))).optional()?;
            if let Some((start,pcm)) = existing {
                ensure!(u64::try_from(start)? == block.start_sample && pcm == block.pcm, "dictate.block_conflict");
                return Ok(record.receipt());
            }
            record.append(&block)?;
            tx.execute("INSERT INTO dictate_audio_block(owner,recording_id,sequence,start_sample,pcm,digest) VALUES(?1,?2,?3,?4,?5,?6)",
                params![owner,block.recording_id,i64::try_from(block.sequence)?,i64::try_from(block.start_sample)?,block.pcm,Sha256::digest(&block.pcm).as_slice()])?;
            update(tx,&owner,&record)?;
            Ok(record.receipt())
        })().map_err(sql_error)).await?)
    }
    async fn change(&self, id: &str, change: Change) -> Result<Recording> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self
            .db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    let mut record = get(tx, &owner, &id)?;
                    // Audio expiry must delete PCM in the same transaction.
                    let expire = matches!(change, Change::ExpireAudio { .. });
                    record.apply(change)?;
                    if expire {
                        tx.execute(
                            "DELETE FROM dictate_audio_block WHERE owner=?1 AND recording_id=?2",
                            params![owner, id],
                        )?;
                    }
                    update(tx, &owner, &record)?;
                    Ok(record)
                })()
                .map_err(sql_error)
            })
            .await?)
    }
    async fn audio(&self, id: &str, start_sample: u64, sample_count: u64) -> Result<Vec<u8>> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self.db.read(move |db| (|| -> Result<_> {
            let record = get(db,&owner,&id)?;
            ensure!(!record.audio_expired, "dictate.audio_expired");
            let end = start_sample.checked_add(sample_count).ok_or_else(|| anyhow::anyhow!("dictate.invalid_audio_range"))?;
            ensure!(start_sample <= end && end <= record.saved_samples, "dictate.invalid_audio_range");
            let mut stmt = db.prepare("SELECT start_sample,pcm,digest FROM dictate_audio_block WHERE owner=?1 AND recording_id=?2 AND start_sample<?3 AND start_sample+length(pcm)/2>?4 ORDER BY sequence")?;
            let mut rows = stmt.query(params![owner,id,i64::try_from(end)?,i64::try_from(start_sample)?])?;
            let mut result = Vec::with_capacity(sample_count as usize * 2);
            let mut cursor = start_sample;
            while let Some(row) = rows.next()? {
                let start = u64::try_from(row.get::<_,i64>(0)?)?;
                let pcm: Vec<u8> = row.get(1)?;
                let digest: Vec<u8> = row.get(2)?;
                ensure!(Sha256::digest(&pcm).as_slice() == digest && pcm.len().is_multiple_of(2), "dictate.audio_corrupt");
                let block_end = start + pcm.len() as u64 / 2;
                ensure!(start <= cursor && block_end > cursor, "dictate.audio_gap");
                let through = end.min(block_end);
                result.extend_from_slice(&pcm[((cursor-start)*2) as usize..((through-start)*2) as usize]);
                cursor = through;
            }
            ensure!(cursor == end, "dictate.audio_gap");
            Ok(result)
        })().map_err(sql_error)).await?)
    }
    async fn delete(&self, id: &str) -> Result<()> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        self.db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    if history::delete_text(tx, &owner, &id)? {
                        return Ok(());
                    }
                    let record = get(tx, &owner, &id)?;
                    ensure!(
                        record.capture != CaptureState::Recording
                            && record.delivery != polar_dictate::DeliveryState::Prepared
                            && !record.cleanup_running
                            && !record
                                .segments
                                .iter()
                                .any(|s| s.state == polar_dictate::SegmentState::Running),
                        "dictate.stop_before_delete"
                    );
                    history::mark_deleted(tx, &owner, &id)?;
                    tx.execute(
                        "DELETE FROM dictate_audio_block WHERE owner=?1 AND recording_id=?2",
                        params![owner, id],
                    )?;
                    tx.execute(
                        "DELETE FROM dictate_recording WHERE owner=?1 AND id=?2",
                        params![owner, id],
                    )?;
                    Ok(())
                })()
                .map_err(sql_error)
            })
            .await?;
        Ok(())
    }
    async fn recover(&self) -> Result<usize> {
        let owner = self.owner.clone();
        Ok(self
            .db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    let records = all(tx, &owner)?;
                    let mut count = 0;
                    for mut record in records {
                        if record.capture == CaptureState::Recording
                            || record.processing_enabled
                            || record.cleanup_running
                            || record.delivery == polar_dictate::DeliveryState::Prepared
                        {
                            record.apply(Change::Recover)?;
                            update(tx, &owner, &record)?;
                            count += 1;
                        }
                    }
                    count += history::recover_texts(tx, &owner)?;
                    Ok(count)
                })()
                .map_err(sql_error)
            })
            .await?)
    }
    async fn expire(&self, now_ms: u64) -> Result<usize> {
        Ok(self
            .cleanup_history(now_ms, polar_dictate::RetentionPolicy::default(), true)
            .await?
            .deleted as usize)
    }
    async fn text_create(
        &self,
        r: polar_dictate::TextEditRecord,
    ) -> Result<polar_dictate::TextEditRecord> {
        self.create_text(r).await
    }
    async fn text_finish(
        &self,
        id: &str,
        result: Result<String, String>,
    ) -> Result<polar_dictate::TextEditRecord> {
        self.finish_text(id, result).await
    }
    async fn item(&self, id: &str) -> Result<polar_dictate::HistoryItem> {
        self.get_item(id).await
    }
    async fn item_change(
        &self,
        id: &str,
        change: polar_dictate::ItemChange,
    ) -> Result<polar_dictate::HistoryItem> {
        self.change_item(id, change).await
    }
    async fn history(
        &self,
        cursor: Option<polar_dictate::HistoryCursor>,
        limit: u32,
    ) -> Result<polar_dictate::HistoryPage> {
        self.history_page(cursor, limit).await
    }
    async fn cleanup(
        &self,
        now_ms: u64,
        policy: polar_dictate::RetentionPolicy,
        apply: bool,
    ) -> Result<polar_dictate::CleanupReport> {
        self.cleanup_history(now_ms, policy, apply).await
    }
    async fn import_legacy(
        &self,
        entries: Vec<polar_dictate::LegacyInput>,
    ) -> Result<polar_dictate::ImportReport> {
        self.import_inputs(entries).await
    }
    async fn finish_legacy_import(&self) -> Result<()> {
        self.release_legacy_barrier().await
    }
    async fn import_issues(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<polar_dictate::LegacyInput>> {
        let owner = self.owner.clone();
        let payloads=self.db.read(move |db|{
            let mut stmt=db.prepare("SELECT payload FROM dictate_legacy_import WHERE owner=?1 AND conflict=1 ORDER BY id,digest LIMIT ?2 OFFSET ?3")?;
            let rows=stmt.query_map(params![owner,limit.clamp(1,100),offset],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>();rows
        }).await?;
        Ok(payloads
            .iter()
            .map(|s| serde_json::from_str(s))
            .collect::<Result<_, _>>()?)
    }
    async fn pending(&self) -> Result<Vec<Recording>> {
        self.pending_records().await
    }
}
