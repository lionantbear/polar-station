//! Shared physical Host database. Domain validation stays with each caller.
use polar_sqlite_runtime::{SqliteMigration, SqliteRuntime, SqliteRuntimeConfig};
use polar_storage_migration::{MigrationDescriptor, StorageBackend, TransactionMode};
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
mod codex_control;
mod connector;
pub use connector::{ConnectorDeliveryRow, ConnectorInboundRow, ConnectorStreamRow};
mod inbox;
mod learning;
pub use learning::SqliteLearningStore;
pub use codex_control::{
    advance_codex_binding_revisions, CodexBindingRow, CodexCheckpointRow,
    CodexCompletionWithNotifications, CodexNotificationRow, CodexSubmissionRow, ImessageReceiptRow,
    SqliteCodexStore, SubmissionTransition,
};
pub use inbox::{InboxStoreError, SqliteInboxStore};

#[derive(Clone)]
pub struct HostDatabase {
    path: PathBuf,
    runtime: Arc<Mutex<Option<SqliteRuntime>>>,
}

impl std::fmt::Debug for HostDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDatabase")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Closed registration of Host document owners; not caller-supplied SQL keys.
#[derive(Clone, Copy, Debug)]
pub enum HostDocument {
    Voice,
    TranscreationPreferences,
    Settings,
    SessionRuntime,
    Resources,
    CodexControl,
    ImessageSender,
    MessageConnectors,
}

impl HostDocument {
    fn key(self) -> &'static str {
        match self {
            Self::TranscreationPreferences => "station.transcreation.preferences.v1",
            Self::Voice => "station.voice.v1",
            Self::Settings => "station.settings.v1",
            Self::SessionRuntime => "station.session_runtime.v1",
            Self::Resources => "station.resources.v1",
            Self::CodexControl => "station.codex_control.v1",
            Self::ImessageSender => "station.imessage_sender.v1",
            Self::MessageConnectors => "station.message_connectors.v1",
        }
    }
}

impl HostDatabase {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            runtime: Arc::new(Mutex::new(None)),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn runtime(&self) -> Result<SqliteRuntime, StoreError> {
        let mut slot = self.runtime.lock().map_err(|_| StoreError::Poisoned)?;
        if let Some(runtime) = slot.as_ref() {
            return Ok(runtime.clone());
        }
        if self.path != Path::new("sqlite::memory:") {
            if !self.path.is_absolute() {
                return Err(StoreError::InvalidPath);
            }
            let parent = self.path.parent().ok_or(StoreError::InvalidPath)?;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&self.path) {
                Ok(file) => file.sync_all()?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(&self.path)?.file_type().is_file() {
                        return Err(StoreError::InvalidPath);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let sql = include_str!("schema.sql");
        let codex_sql = include_str!("codex_schema.sql");
        let thread_identity_sql = include_str!("codex_thread_identity.sql");
        let telegram_ack_sql = include_str!("telegram_acknowledgements.sql");
        let retention_sql = include_str!("codex_retention.sql");
        let codex_inbox_sql = include_str!("codex_inbox.sql");
        let optional_session_sql = include_str!("codex_optional_session.sql");
        let connector_sql = include_str!("connector_streams.sql");
        let direct_input_sql = include_str!("codex_direct_input.sql");
        let runtime =
            SqliteRuntime::open(
                &self.path,
                SqliteRuntimeConfig {
                    reader_workers: 1,
                    ..Default::default()
                },
                vec![
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 1,
                        name: "initial".into(),
                        path: "crates/polar-station-store-sqlite/src/schema.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 2,
                        name: "codex-control-and-channels".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_schema.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(codex_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: codex_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 3,
                        name: "codex-stable-thread-identity".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_thread_identity.sql"
                            .into(),
                        sha256: format!("{:x}", Sha256::digest(thread_identity_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: thread_identity_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 4,
                        name: "telegram-acknowledgements".into(),
                        path: "crates/polar-station-store-sqlite/src/telegram_acknowledgements.sql"
                            .into(),
                        sha256: format!("{:x}", Sha256::digest(telegram_ack_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: telegram_ack_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 5,
                        name: "codex-resolved-history-retention".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_retention.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(retention_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: retention_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 6,
                        name: "codex-inbox-origin".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_inbox.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(codex_inbox_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: codex_inbox_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 7,
                        name: "codex-optional-tmux-association".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_optional_session.sql"
                            .into(),
                        sha256: format!("{:x}", Sha256::digest(optional_session_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: optional_session_sql,
                    post_apply: None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(), version: 8,
                        name: "connector-streams".into(),
                        path: "crates/polar-station-store-sqlite/src/connector_streams.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(connector_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    }, sql:connector_sql, post_apply:None,
                },
                SqliteMigration {
                    descriptor: MigrationDescriptor {
                        backend: StorageBackend::Sqlite,
                        component: "station-host".into(),
                        version: 9,
                        name: "codex-immediate-input".into(),
                        path: "crates/polar-station-store-sqlite/src/codex_direct_input.sql".into(),
                        sha256: format!("{:x}", Sha256::digest(direct_input_sql.as_bytes())),
                        transaction_mode: TransactionMode::Required,
                    },
                    sql: direct_input_sql,
                    post_apply: None,
                },
            ],
            )?;
        *slot = Some(runtime.clone());
        Ok(runtime)
    }
}

pub fn read_document(
    connection: &Connection,
    document: HostDocument,
) -> rusqlite::Result<Option<Vec<u8>>> {
    connection
        .query_row(
            "SELECT payload FROM station_document WHERE key=?1",
            [document.key()],
            |row| row.get(0),
        )
        .optional()
}

pub fn write_document(
    connection: &Connection,
    document: HostDocument,
    payload: &[u8],
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO station_document(key,payload) VALUES (?1,?2)
        ON CONFLICT(key) DO UPDATE SET payload=excluded.payload",
        rusqlite::params![document.key(), payload],
    )?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Inbox(#[from] polar_station_inbox::InboxError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Host database requires an absolute regular-file path")]
    InvalidPath,
    #[error("Host database initialization lock poisoned")]
    Poisoned,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Runtime(#[from] polar_sqlite_runtime::SqliteRuntimeError),
}
