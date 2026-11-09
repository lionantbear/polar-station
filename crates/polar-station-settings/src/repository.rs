use crate::{SettingsSectionUpdateV1, SettingsV1, ValidationError};
use polar_station_store_sqlite::{read_document, write_document, HostDatabase, HostDocument};
use rusqlite::{Connection, TransactionBehavior};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SettingsRepository {
    database: HostDatabase,
}

impl SettingsRepository {
    pub fn load_voice(&self) -> Result<polar_dictate::VoiceSettings, RepositoryError> {
        self.load_module(HostDocument::Voice)
    }
    pub fn update_voice(
        &self,
        expected_revision: u64,
        value: polar_dictate::VoiceSettings,
    ) -> Result<polar_dictate::VoiceSettings, UpdateError> {
        self.update_module(HostDocument::Voice, expected_revision, value)
    }

    pub fn load_message_connectors(
        &self,
    ) -> Result<crate::MessageConnectorSettingsV1, RepositoryError> {
        self.load_module(HostDocument::MessageConnectors)
    }
    pub fn update_message_connectors(
        &self,
        expected_revision: u64,
        settings: crate::MessageConnectorSettingsV1,
    ) -> Result<crate::MessageConnectorSettingsV1, UpdateError> {
        self.update_module(HostDocument::MessageConnectors, expected_revision, settings)
    }
    pub fn load_codex_control(&self) -> Result<crate::CodexControlSettingsV1, RepositoryError> {
        self.load_module(HostDocument::CodexControl)
    }

    pub fn update_codex_control(
        &self,
        expected_revision: u64,
        settings: crate::CodexControlSettingsV1,
    ) -> Result<crate::CodexControlSettingsV1, UpdateError> {
        self.update_module(HostDocument::CodexControl, expected_revision, settings)
    }

    pub fn load_imessage_sender(&self) -> Result<crate::ImessageSenderSettingsV1, RepositoryError> {
        self.load_module(HostDocument::ImessageSender)
    }

    pub fn update_imessage_sender(
        &self,
        expected_revision: u64,
        settings: crate::ImessageSenderSettingsV1,
    ) -> Result<crate::ImessageSenderSettingsV1, UpdateError> {
        self.update_module(HostDocument::ImessageSender, expected_revision, settings)
    }

    fn load_module<T: ControlDocument>(
        &self,
        document: HostDocument,
    ) -> Result<T, RepositoryError> {
        self.database
            .runtime()?
            .read_blocking(move |connection| Ok(load_control_document(connection, document)))?
    }

    fn update_module<T: ControlDocument>(
        &self,
        document: HostDocument,
        expected_revision: u64,
        mut value: T,
    ) -> Result<T, UpdateError> {
        value.validate()?;
        self.database
            .runtime()
            .map_err(RepositoryError::from)?
            .write_blocking(move |connection| {
                Ok((|| {
                    let transaction = connection
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .map_err(RepositoryError::from)?;
                    let current: T = load_control_document(&transaction, document)?;
                    if current.revision() != expected_revision {
                        return Err(UpdateError::RevisionConflict {
                            current_revision: current.revision(),
                        });
                    }
                    value.set_revision(
                        expected_revision
                            .checked_add(1)
                            .ok_or(RepositoryError::RevisionExhausted)?,
                    );
                    if value.invalidates_codex_bindings(&current)
                        && !polar_station_store_sqlite::advance_codex_binding_revisions(
                            &transaction,
                        )
                        .map_err(RepositoryError::from)?
                    {
                        return Err(RepositoryError::RevisionExhausted.into());
                    }
                    write_document(
                        &transaction,
                        document,
                        &serde_json::to_vec(&value).map_err(RepositoryError::from)?,
                    )
                    .map_err(RepositoryError::from)?;
                    transaction.commit().map_err(RepositoryError::from)?;
                    Ok(value)
                })())
            })
            .map_err(RepositoryError::from)?
    }
    pub fn new(path: PathBuf) -> Self {
        Self {
            database: HostDatabase::new(path),
        }
    }
    pub fn from_database(database: HostDatabase) -> Self {
        Self { database }
    }
    pub fn path(&self) -> &Path {
        self.database.path()
    }

    pub fn load_or_initialize(&self) -> Result<SettingsV1, RepositoryError> {
        self.database
            .runtime()?
            .write_blocking(|connection| Ok(initialize(connection)))?
    }

    pub fn load(&self) -> Result<Option<SettingsV1>, RepositoryError> {
        self.database
            .runtime()?
            .read_blocking(|connection| Ok(load(connection)))?
    }

    pub fn update_section(
        &self,
        expected_revision: u64,
        update: SettingsSectionUpdateV1,
    ) -> Result<SettingsV1, UpdateError> {
        self.database
            .runtime()
            .map_err(RepositoryError::from)?
            .write_blocking(move |connection| {
                Ok(update_settings(connection, expected_revision, update))
            })
            .map_err(RepositoryError::from)?
    }
}

// Closed implementation for registered module documents; no arbitrary keys.
trait ControlDocument:
    serde::Serialize + serde::de::DeserializeOwned + Default + Send + 'static
{
    fn validate(&self) -> Result<(), ValidationError>;
    fn revision(&self) -> u64;
    fn set_revision(&mut self, revision: u64);
    fn invalidates_codex_bindings(&self, _previous: &Self) -> bool {
        false
    }
}

impl ControlDocument for polar_dictate::VoiceSettings {
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate()
            .map_err(|e| ValidationError::Voice(e.to_string()))
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
}
impl ControlDocument for crate::CodexControlSettingsV1 {
    fn invalidates_codex_bindings(&self, previous: &Self) -> bool {
        self.enabled != previous.enabled
            || self.telegram != previous.telegram
            || self.imessage_enabled != previous.imessage_enabled
            || self.imessage_sender != previous.imessage_sender
            || self.host_connections != previous.host_connections
    }
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate()
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
}
impl ControlDocument for crate::ImessageSenderSettingsV1 {
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate()
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
}
impl ControlDocument for crate::MessageConnectorSettingsV1 {
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate()
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
}
fn load_control_document<T: ControlDocument>(
    connection: &Connection,
    document: HostDocument,
) -> Result<T, RepositoryError> {
    let value: T = read_document(connection, document)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or_default();
    value.validate()?;
    Ok(value)
}

fn initialize(connection: &mut Connection) -> Result<SettingsV1, RepositoryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let settings = match load(&transaction)? {
        Some(settings) => settings,
        None => {
            let settings = SettingsV1::default();
            save(&transaction, &settings)?;
            settings
        }
    };
    transaction.commit()?;
    Ok(settings)
}

fn update_settings(
    connection: &mut Connection,
    expected_revision: u64,
    update: SettingsSectionUpdateV1,
) -> Result<SettingsV1, UpdateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(RepositoryError::from)?;
    let mut settings = load(&transaction)?.unwrap_or_default();
    if settings.revision != expected_revision {
        return Err(UpdateError::RevisionConflict {
            current_revision: settings.revision,
        });
    }
    update.apply(&mut settings)?;
    settings.revision = settings
        .revision
        .checked_add(1)
        .ok_or(RepositoryError::RevisionExhausted)?;
    save(&transaction, &settings)?;
    transaction.commit().map_err(RepositoryError::from)?;
    Ok(settings)
}

fn load(connection: &Connection) -> Result<Option<SettingsV1>, RepositoryError> {
    read_document(connection, HostDocument::Settings)?
        .map(|bytes| {
            let settings: SettingsV1 = serde_json::from_slice(&bytes)?;
            settings.validate()?;
            Ok(settings)
        })
        .transpose()
}

fn save(connection: &Connection, settings: &SettingsV1) -> Result<(), RepositoryError> {
    settings.validate()?;
    write_document(
        connection,
        HostDocument::Settings,
        &serde_json::to_vec(settings)?,
    )?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("settings revision is exhausted")]
    RevisionExhausted,
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Store(#[from] polar_station_store_sqlite::StoreError),
    #[error(transparent)]
    Runtime(#[from] polar_sqlite_runtime::SqliteRuntimeError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    InvalidDocument(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("settings changed; current revision is {current_revision}")]
    RevisionConflict { current_revision: u64 },
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
}
