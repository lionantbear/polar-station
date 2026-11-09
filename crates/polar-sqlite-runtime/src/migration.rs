use polar_storage_migration::{
    validate_order, MigrationDescriptor, MigrationPlanError, StorageBackend, TransactionMode,
    MIGRATION_METADATA_TABLE,
};
use rusqlite::{params, Connection, TransactionBehavior};

// Runtime-owned table name: `_polar_schema_migrations`. The value itself is
// imported from the backend-neutral identity crate to prevent schema drift.

#[derive(Clone, Debug)]
pub struct SqliteMigration {
    pub descriptor: MigrationDescriptor,
    pub sql: &'static str,
    /// Optional backend-native compatibility repair that belongs to this
    /// declared migration version. It runs in the same transaction as `sql`
    /// before the version is recorded, so adapters never repair a schema after
    /// their runtime has been published.
    pub post_apply: Option<fn(&rusqlite::Transaction<'_>) -> rusqlite::Result<()>>,
}

#[derive(Debug)]
pub enum SqliteMigrationError {
    Driver(rusqlite::Error),
    Plan(MigrationPlanError),
    WrongBackend(String),
    ChecksumMismatch { component: String, version: u32 },
}

impl std::fmt::Display for SqliteMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Driver(error) => error.fmt(f),
            Self::Plan(error) => error.fmt(f),
            Self::WrongBackend(component) => {
                write!(f, "non-SQLite migration supplied for {component}")
            }
            Self::ChecksumMismatch { component, version } => write!(
                f,
                "migration checksum mismatch for {component} version {version}"
            ),
        }
    }
}

impl std::error::Error for SqliteMigrationError {}

impl From<rusqlite::Error> for SqliteMigrationError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Driver(value)
    }
}

impl From<MigrationPlanError> for SqliteMigrationError {
    fn from(value: MigrationPlanError) -> Self {
        Self::Plan(value)
    }
}

pub(crate) fn apply(
    connection: &mut Connection,
    migrations: &[SqliteMigration],
) -> Result<(), SqliteMigrationError> {
    let descriptors = migrations
        .iter()
        .map(|migration| migration.descriptor.clone())
        .collect::<Vec<_>>();
    validate_order(&descriptors)?;
    connection.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {MIGRATION_METADATA_TABLE} (component TEXT NOT NULL, version INTEGER NOT NULL, name TEXT NOT NULL, sha256 TEXT NOT NULL, applied_at_ms INTEGER NOT NULL DEFAULT (unixepoch('subsec') * 1000), PRIMARY KEY(component, version));"))?;
    for migration in migrations {
        let descriptor = &migration.descriptor;
        if descriptor.backend != StorageBackend::Sqlite {
            return Err(SqliteMigrationError::WrongBackend(
                descriptor.component.clone(),
            ));
        }
        let existing = connection
            .prepare_cached(&format!("SELECT sha256 FROM {MIGRATION_METADATA_TABLE} WHERE component = ?1 AND version = ?2"))?
            .query_row(params![descriptor.component, descriptor.version], |row| row.get::<_, String>(0));
        match existing {
            Ok(checksum) if checksum == descriptor.sha256 => continue,
            Ok(_) => {
                return Err(SqliteMigrationError::ChecksumMismatch {
                    component: descriptor.component.clone(),
                    version: descriptor.version,
                })
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(error) => return Err(error.into()),
        }
        match descriptor.transaction_mode {
            TransactionMode::Required => {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                transaction.execute_batch(migration.sql)?;
                if let Some(post_apply) = migration.post_apply {
                    post_apply(&transaction)?;
                }
                transaction.execute(
                    &CANONICAL_SQL_37F3690CCD47
                        .replace("{MIGRATION_METADATA_TABLE}", MIGRATION_METADATA_TABLE),
                    params![
                        descriptor.component,
                        descriptor.version,
                        descriptor.name,
                        descriptor.sha256
                    ],
                )?;
                transaction.commit()?;
            }
            TransactionMode::Forbidden => {
                if migration.post_apply.is_some() {
                    return Err(rusqlite::Error::InvalidQuery.into());
                }
                connection.execute_batch(migration.sql)?;
                connection.execute(
                    &CANONICAL_SQL_37F3690CCD47
                        .replace("{MIGRATION_METADATA_TABLE}", MIGRATION_METADATA_TABLE),
                    params![
                        descriptor.component,
                        descriptor.version,
                        descriptor.name,
                        descriptor.sha256
                    ],
                )?;
            }
        }
    }
    Ok(())
}
// Canonical persistence query literals for Archive15 P5.

const CANONICAL_SQL_37F3690CCD47: &str = r"INSERT INTO {MIGRATION_METADATA_TABLE}(component, version, name, sha256) VALUES (?1, ?2, ?3, ?4)";
