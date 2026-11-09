use std::{
    any::Any,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use polar_storage_core::{
    StorageError, StorageErrorKind, StorageOperationId, StorageRuntimeMetricEvent,
    StorageRuntimeMetrics, StorageRuntimeMetricsSnapshot,
};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::{
    config::SqliteRuntimeConfig, migration, readers::ReaderWorkerPool, SqliteDurability,
    SqliteMigration,
};

type BoxResult = Result<Box<dyn Any + Send>, SqliteRuntimeError>;
type Operation = Box<dyn FnOnce(&mut Connection) -> BoxResult + Send>;
type MigrationFingerprint = Vec<(u8, String, u32, String, String, String, u8)>;

enum Command {
    Execute(Operation, oneshot::Sender<BoxResult>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug)]
pub enum SqliteRuntimeError {
    Driver(rusqlite::Error),
    Migration(crate::SqliteMigrationError),
    AlreadyOwned(PathBuf),
    RuntimeAlreadyPublished(PathBuf),
    DurabilityConflict(PathBuf),
    ReadUnavailableForMemory,
    Closed,
    ResponseType,
}

impl std::fmt::Display for SqliteRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Driver(error) => error.fmt(formatter),
            Self::Migration(error) => error.fmt(formatter),
            Self::AlreadyOwned(path) => write!(formatter, "SQLite writer already owns {}", path.display()),
            Self::RuntimeAlreadyPublished(path) => write!(
                formatter,
                "SQLite runtime is already published for {}; migrations require bootstrap before publication",
                path.display()
            ),
            Self::DurabilityConflict(path) => write!(formatter, "SQLite durability conflicts with existing writer for {}", path.display()),
            Self::ReadUnavailableForMemory => formatter.write_str(
                "SQLite reader workers require a file-backed canonical target",
            ),
            Self::Closed => formatter.write_str("SQLite runtime is closed"),
            Self::ResponseType => formatter.write_str("SQLite runtime returned an unexpected response type"),
        }
    }
}

impl std::error::Error for SqliteRuntimeError {}

impl From<SqliteRuntimeError> for StorageError {
    fn from(error: SqliteRuntimeError) -> Self {
        let kind = match error {
            SqliteRuntimeError::AlreadyOwned(_)
            | SqliteRuntimeError::DurabilityConflict(_)
            | SqliteRuntimeError::RuntimeAlreadyPublished(_) => StorageErrorKind::Conflict,
            SqliteRuntimeError::ReadUnavailableForMemory => StorageErrorKind::Unavailable,
            SqliteRuntimeError::Closed => StorageErrorKind::ShuttingDown,
            SqliteRuntimeError::Driver(_)
            | SqliteRuntimeError::Migration(_)
            | SqliteRuntimeError::ResponseType => StorageErrorKind::Internal,
        };
        StorageError::new(
            kind,
            error.to_string(),
            None,
            None,
            polar_storage_core::RetryDisposition::Never,
        )
    }
}

impl From<rusqlite::Error> for SqliteRuntimeError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Driver(value)
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<RuntimeInner>>> {
    static VALUE: OnceLock<Mutex<HashMap<PathBuf, Weak<RuntimeInner>>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct RuntimeInner {
    path: PathBuf,
    migration_fingerprint: MigrationFingerprint,
    sender: mpsc::Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
    readers: Option<ReaderWorkerPool>,
    accepting: AtomicBool,
    shutdown_started: AtomicBool,
    metrics: StorageRuntimeMetrics,
    writer_queue_capacity: usize,
    durability: SqliteDurability,
}

/// Cloneable handle to the unique writer owner for one canonical SQLite file.
#[derive(Clone)]
pub struct SqliteRuntime {
    inner: Arc<RuntimeInner>,
}

pub enum TransactionOutcome<T> {
    Commit(T),
    Rollback(T),
}

impl SqliteRuntime {
    fn default_operation(id: &'static str) -> StorageOperationId {
        StorageOperationId::new(id).expect("runtime operation IDs are valid")
    }

    pub fn open(
        path: impl AsRef<Path>,
        config: SqliteRuntimeConfig,
        migrations: Vec<SqliteMigration>,
    ) -> Result<Self, SqliteRuntimeError> {
        config.validate().map_err(|_| SqliteRuntimeError::Closed)?;
        let memory = path.as_ref() == Path::new("sqlite::memory:");
        let path = canonical_location(path.as_ref())?;
        let migration_fingerprint = migration_fingerprint(&migrations);
        let mut registry_guard = (!memory).then(|| registry().lock().expect("runtime registry"));
        if let Some(existing) = registry_guard
            .as_ref()
            .and_then(|items| items.get(&path))
            .and_then(Weak::upgrade)
        {
            if existing.accepting.load(Ordering::Acquire) {
                if existing.durability != config.durability {
                    return Err(SqliteRuntimeError::DurabilityConflict(path));
                }
                if migrations.is_empty()
                    || migrations_are_already_bootstrapped(
                        &existing.migration_fingerprint,
                        &migration_fingerprint,
                    )
                {
                    return Ok(Self { inner: existing });
                }
                return Err(SqliteRuntimeError::RuntimeAlreadyPublished(path));
            }
            registry_guard
                .as_mut()
                .expect("file-backed runtime has registry guard")
                .remove(&path);
        }

        let writer_queue_capacity = config.queue_capacity.max(1);
        let (sender, mut receiver) = mpsc::channel(writer_queue_capacity);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker_path = path.clone();
        let busy_timeout = config.busy_timeout;
        let durability = config.durability;
        let thread = std::thread::Builder::new()
            .name("polar-sqlite-writer".into())
            .spawn(move || {
                match initialize(&worker_path, memory, busy_timeout, durability, &migrations) {
                    Ok(mut connection) => {
                        let _ = ready_tx.send(Ok(()));
                        while let Some(command) = receiver.blocking_recv() {
                            match command {
                                Command::Execute(operation, response) => {
                                    let _ = response.send(operation(&mut connection));
                                }
                                Command::Shutdown(done) => {
                                    let _ = connection
                                        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                                    let _ = done.send(());
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(|_| SqliteRuntimeError::Closed)?;
        if let Err(error) = ready_rx.recv().map_err(|_| SqliteRuntimeError::Closed)? {
            let _ = thread.join();
            return Err(error);
        }

        let readers = if memory {
            None
        } else {
            match ReaderWorkerPool::new(
                path.clone(),
                config.busy_timeout,
                config.reader_workers,
                config.reader_queue_capacity,
            ) {
                Ok(readers) => Some(readers),
                Err(error) => {
                    drop(sender);
                    let _ = thread.join();
                    return Err(error);
                }
            }
        };
        let inner = Arc::new(RuntimeInner {
            path: path.clone(),
            migration_fingerprint,
            sender,
            thread: Mutex::new(Some(thread)),
            readers,
            accepting: AtomicBool::new(true),
            shutdown_started: AtomicBool::new(false),
            metrics: StorageRuntimeMetrics::default(),
            writer_queue_capacity,
            durability,
        });
        if let Some(items) = registry_guard.as_mut() {
            items.insert(path, Arc::downgrade(&inner));
        }
        Ok(Self { inner })
    }

    pub async fn read<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        self.read_with_operation(Self::default_operation("storage.sqlite.read"), operation)
            .await
    }

    pub async fn read_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        if !self.inner.accepting.load(Ordering::Acquire) {
            self.inner.metrics.reject(&operation_id);
            return Err(SqliteRuntimeError::Closed);
        }
        if let Some(readers) = self.inner.readers.as_ref() {
            let result = readers
                .read_with_operation(operation_id.clone(), operation)
                .await;
            self.inner.metrics.observe(StorageRuntimeMetricEvent {
                operation_id: operation_id.clone(),
                queue_depth: 0,
                admission_wait: Duration::ZERO,
                transaction_duration: None,
                pool_acquire: None,
                copy_bytes: 0,
                runtime_dispatches: 1,
                statements: 1,
                transactions: 0,
                rows_read: 0,
                rows_written: 0,
            });
            if result.is_ok() {
                self.inner.metrics.complete(&operation_id);
            }
            result
        } else {
            // A private in-memory SQLite target cannot be opened by the
            // file-backed reader workers. The test-only topology still uses
            // this read API so production call sites never retain a writer
            // read path merely to support an in-memory fixture.
            self.write_with_operation(operation_id, move |connection| operation(connection))
                .await
        }
    }

    pub fn read_blocking<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        self.read_blocking_with_operation(Self::default_operation("storage.sqlite.read"), operation)
    }

    pub fn read_blocking_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        if !self.inner.accepting.load(Ordering::Acquire) {
            self.inner.metrics.reject(&operation_id);
            return Err(SqliteRuntimeError::Closed);
        }
        if let Some(readers) = self.inner.readers.as_ref() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                    return tokio::task::block_in_place(|| {
                        readers.read_blocking_with_operation(operation_id, operation)
                    });
                }
            }
            return readers.read_blocking_with_operation(operation_id, operation);
        }
        self.write_blocking_with_operation(operation_id, move |connection| operation(connection))
    }

    pub async fn write<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        self.write_with_operation(Self::default_operation("storage.sqlite.write"), operation)
            .await
    }

    pub async fn write_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let admission_started = Instant::now();
        if !self.inner.accepting.load(Ordering::Acquire) {
            self.inner.metrics.reject(&operation_id);
            return Err(SqliteRuntimeError::Closed);
        }
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .sender
            .send(Command::Execute(box_operation(operation), response_tx))
            .await
            .map_err(|_| SqliteRuntimeError::Closed)?;
        self.inner.metrics.observe(StorageRuntimeMetricEvent {
            operation_id: operation_id.clone(),
            queue_depth: self
                .inner
                .writer_queue_capacity
                .saturating_sub(self.inner.sender.capacity()) as u64,
            admission_wait: admission_started.elapsed(),
            transaction_duration: None,
            pool_acquire: None,
            copy_bytes: 0,
            runtime_dispatches: 1,
            statements: 1,
            transactions: 0,
            rows_read: 0,
            rows_written: 0,
        });
        let value = response_rx
            .await
            .map_err(|_| SqliteRuntimeError::Closed)??;
        self.inner.metrics.complete(&operation_id);
        downcast(value)
    }

    pub fn write_blocking<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        self.write_blocking_with_operation(
            Self::default_operation("storage.sqlite.write"),
            operation,
        )
    }

    pub fn write_blocking_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        // Some legacy synchronous domain ports are invoked from async host
        // composition. Their execution still belongs to this runtime's one
        // writer; temporarily leaving the Tokio worker prevents `blocking_*`
        // from panicking while preserving the synchronous port contract.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                return tokio::task::block_in_place(|| {
                    self.write_blocking_inner(operation_id, operation)
                });
            }
            let runtime = self.clone();
            let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
            let _join = tokio::task::spawn_blocking(move || {
                let result = runtime.write_blocking_inner(operation_id, operation);
                let _ = result_tx.send(result);
            });
            return result_rx.recv().map_err(|_| SqliteRuntimeError::Closed)?;
        }
        self.write_blocking_inner(operation_id, operation)
    }

    fn write_blocking_inner<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let admission_started = Instant::now();
        if !self.inner.accepting.load(Ordering::Acquire) {
            self.inner.metrics.reject(&operation_id);
            return Err(SqliteRuntimeError::Closed);
        }
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .sender
            .blocking_send(Command::Execute(box_operation(operation), response_tx))
            .map_err(|_| SqliteRuntimeError::Closed)?;
        self.inner.metrics.observe(StorageRuntimeMetricEvent {
            operation_id: operation_id.clone(),
            queue_depth: self
                .inner
                .writer_queue_capacity
                .saturating_sub(self.inner.sender.capacity()) as u64,
            admission_wait: admission_started.elapsed(),
            transaction_duration: None,
            pool_acquire: None,
            copy_bytes: 0,
            runtime_dispatches: 1,
            statements: 1,
            transactions: 0,
            rows_read: 0,
            rows_written: 0,
        });
        let value = response_rx
            .blocking_recv()
            .map_err(|_| SqliteRuntimeError::Closed)??;
        self.inner.metrics.complete(&operation_id);
        downcast(value)
    }

    pub async fn transaction<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        self.transaction_with_operation(
            Self::default_operation("storage.sqlite.transaction"),
            operation,
        )
        .await
    }

    pub async fn transaction_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let metrics = self.inner.metrics.clone();
        self.write_with_operation(operation_id.clone(), move |connection| {
            let started = Instant::now();
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = operation(&transaction)?;
            transaction.commit()?;
            metrics.record_transaction_duration(&operation_id, started.elapsed());
            Ok(value)
        })
        .await
    }

    pub fn transaction_blocking<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let operation_id = Self::default_operation("storage.sqlite.transaction");
        let metrics = self.inner.metrics.clone();
        self.write_blocking_with_operation(operation_id.clone(), move |connection| {
            let started = Instant::now();
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = operation(&transaction)?;
            transaction.commit()?;
            metrics.record_transaction_duration(&operation_id, started.elapsed());
            Ok(value)
        })
    }

    pub fn transaction_blocking_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let metrics = self.inner.metrics.clone();
        self.write_blocking_with_operation(operation_id.clone(), move |connection| {
            let started = Instant::now();
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = operation(&transaction)?;
            transaction.commit()?;
            metrics.record_transaction_duration(&operation_id, started.elapsed());
            Ok(value)
        })
    }

    pub async fn controlled_transaction<T, F>(&self, operation: F) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<TransactionOutcome<T>, rusqlite::Error>
            + Send
            + 'static,
    {
        self.write(move |connection| controlled(connection, operation))
            .await
    }

    pub fn controlled_transaction_blocking<T, F>(
        &self,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<TransactionOutcome<T>, rusqlite::Error>
            + Send
            + 'static,
    {
        self.write_blocking(move |connection| controlled(connection, operation))
    }

    pub async fn shutdown(&self) -> Result<(), SqliteRuntimeError> {
        if self.inner.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.accepting.store(false, Ordering::Release);
        let (done_tx, done_rx) = oneshot::channel();
        self.inner
            .sender
            .send(Command::Shutdown(done_tx))
            .await
            .map_err(|_| SqliteRuntimeError::Closed)?;
        done_rx.await.map_err(|_| SqliteRuntimeError::Closed)?;
        if let Some(readers) = &self.inner.readers {
            readers.shutdown().await?;
        }
        let thread = { self.inner.thread.lock().expect("writer thread lock").take() };
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|_| SqliteRuntimeError::Closed)?
                .map_err(|_| SqliteRuntimeError::Closed)?;
        }
        if self.inner.path != Path::new("sqlite::memory:") {
            registry()
                .lock()
                .expect("runtime registry")
                .remove(&self.inner.path);
        }
        Ok(())
    }

    pub fn metrics(&self) -> StorageRuntimeMetricsSnapshot {
        self.inner.metrics.snapshot()
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn durability(&self) -> SqliteDurability {
        self.inner.durability
    }
}

fn migration_fingerprint(migrations: &[SqliteMigration]) -> MigrationFingerprint {
    migrations
        .iter()
        .map(|migration| {
            let descriptor = &migration.descriptor;
            (
                descriptor.backend as u8,
                descriptor.component.clone(),
                descriptor.version,
                descriptor.name.clone(),
                descriptor.path.clone(),
                descriptor.sha256.clone(),
                descriptor.transaction_mode as u8,
            )
        })
        .collect()
}

fn migrations_are_already_bootstrapped(
    published: &MigrationFingerprint,
    requested: &MigrationFingerprint,
) -> bool {
    requested
        .iter()
        .all(|migration| published.iter().any(|candidate| candidate == migration))
}

fn box_operation<T, F>(operation: F) -> Operation
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
{
    Box::new(move |connection| {
        operation(connection)
            .map(|value| Box::new(value) as Box<dyn Any + Send>)
            .map_err(SqliteRuntimeError::Driver)
    })
}

fn downcast<T: Send + 'static>(value: Box<dyn Any + Send>) -> Result<T, SqliteRuntimeError> {
    value
        .downcast::<T>()
        .map(|value| *value)
        .map_err(|_| SqliteRuntimeError::ResponseType)
}

fn controlled<T, F>(connection: &mut Connection, operation: F) -> Result<T, rusqlite::Error>
where
    F: FnOnce(&Transaction<'_>) -> Result<TransactionOutcome<T>, rusqlite::Error>,
{
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match operation(&transaction)? {
        TransactionOutcome::Commit(value) => {
            transaction.commit()?;
            Ok(value)
        }
        TransactionOutcome::Rollback(value) => {
            transaction.rollback()?;
            Ok(value)
        }
    }
}

fn canonical_location(path: &Path) -> Result<PathBuf, SqliteRuntimeError> {
    if path == Path::new("sqlite::memory:") {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        return Ok(PathBuf::from(format!(
            "sqlite::memory:{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| SqliteRuntimeError::Closed)?
            .join(path)
    };
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|_| SqliteRuntimeError::Closed)?;
    }
    let parent = absolute
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .unwrap_or_else(|| absolute.parent().unwrap_or(Path::new("/")).to_owned());
    Ok(parent.join(absolute.file_name().unwrap_or_default()))
}

fn initialize(
    path: &Path,
    memory: bool,
    busy_timeout: Duration,
    durability: SqliteDurability,
    migrations: &[SqliteMigration],
) -> Result<Connection, SqliteRuntimeError> {
    let mut connection = if memory {
        Connection::open_in_memory()?
    } else {
        Connection::open(path)?
    };
    connection.busy_timeout(busy_timeout)?;
    if !memory {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let synchronous = match durability {
            SqliteDurability::Normal => "NORMAL",
            SqliteDurability::Full => "FULL",
        };
        connection.pragma_update(None, "synchronous", synchronous)?;
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    migration::apply(&mut connection, migrations).map_err(SqliteRuntimeError::Migration)?;
    Ok(connection)
}
