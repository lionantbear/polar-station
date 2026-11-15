use crate::{HostDatabase, StoreError};
use polar_station_inbox::{InboxMessage, RETENTION_MS};
use rusqlite::{params, OptionalExtension};

pub struct SqliteInboxStore {
    database: HostDatabase,
}

impl SqliteInboxStore {
    pub fn from_database(database: HostDatabase) -> Result<Self, InboxStoreError> {
        database.runtime()?;
        Ok(Self { database })
    }
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, InboxStoreError> {
        Self::from_database(HostDatabase::new(path.as_ref().to_path_buf()))
    }
    pub fn open_in_memory() -> Result<Self, InboxStoreError> {
        Self::from_database(HostDatabase::new("sqlite::memory:".into()))
    }
    pub fn save(&self, message: &InboxMessage) -> Result<(), InboxStoreError> {
        let created_at =
            i64::try_from(message.created_at_ms).map_err(|_| InboxStoreError::Timestamp)?;
        let id = message.id.as_str().to_owned();
        let payload = serde_json::to_string(message)?;
        self.database.runtime()?.write_blocking(move |connection| {
            connection.execute("INSERT INTO station_inbox_message(id,created_at_ms,payload_json) VALUES (?1,?2,?3)
                ON CONFLICT(id) DO UPDATE SET created_at_ms=excluded.created_at_ms,payload_json=excluded.payload_json",
                params![id,created_at,payload])?;
            Ok(())
        })?;
        Ok(())
    }
    pub fn page(
        &self,
        before_created_at_ms: Option<u64>,
        limit: usize,
    ) -> Result<Vec<InboxMessage>, InboxStoreError> {
        let limit = limit.clamp(1, 200) as i64;
        let before = i64::try_from(before_created_at_ms.unwrap_or(u64::MAX)).unwrap_or(i64::MAX);
        let payloads = self.database.runtime()?.read_blocking(move |connection| {
            let mut statement = connection.prepare(
                "SELECT payload_json FROM station_inbox_message
                WHERE record_kind='terminal' AND created_at_ms < ?1 ORDER BY created_at_ms DESC,id DESC LIMIT ?2",
            )?;
            let rows =
                statement.query_map(params![before, limit], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        payloads
            .into_iter()
            .map(|p| Ok(serde_json::from_str(&p)?))
            .collect()
    }
    pub fn get(&self, id: &str) -> Result<Option<InboxMessage>, InboxStoreError> {
        let id = id.to_owned();
        let payload = self.database.runtime()?.read_blocking(move |connection| {
            connection
                .query_row(
                    "SELECT payload_json FROM station_inbox_message WHERE id=?1 AND record_kind='terminal'",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })?;
        payload.map(|p| Ok(serde_json::from_str(&p)?)).transpose()
    }
    pub fn prune(&self, now_ms: u64) -> Result<usize, InboxStoreError> {
        let cutoff = i64::try_from(now_ms.saturating_sub(RETENTION_MS))
            .map_err(|_| InboxStoreError::Timestamp)?;
        Ok(self.database.runtime()?.write_blocking(move |connection| {
            connection.execute(
                "DELETE FROM station_inbox_message WHERE created_at_ms < ?1",
                [cutoff],
            )
        })?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InboxStoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] polar_sqlite_runtime::SqliteRuntimeError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Inbox timestamp is outside SQLite range")]
    Timestamp,
}
