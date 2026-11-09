use std::{
    any::Any,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

use polar_storage_core::StorageOperationId;
use rusqlite::{Connection, OpenFlags};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::SqliteRuntimeError;

type BoxResult = Result<Box<dyn Any + Send>, SqliteRuntimeError>;
type ReaderOperation = Box<dyn FnOnce(&Connection) -> BoxResult + Send>;

enum ReaderCommand {
    Execute(ReaderOperation, oneshot::Sender<BoxResult>),
    Shutdown(oneshot::Sender<()>),
}

struct ReaderWorker {
    sender: mpsc::Sender<ReaderCommand>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct ReaderWorkerPoolInner {
    workers: Vec<ReaderWorker>,
    next_worker: AtomicUsize,
    accepting: AtomicBool,
}

/// Fixed, bounded, runtime-owned SQLite reader workers for one file target.
#[derive(Clone)]
pub(crate) struct ReaderWorkerPool {
    inner: Arc<ReaderWorkerPoolInner>,
}

impl ReaderWorkerPool {
    pub(crate) fn new(
        path: PathBuf,
        busy_timeout: Duration,
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, SqliteRuntimeError> {
        let mut workers = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (sender, mut receiver) = mpsc::channel(queue_capacity);
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker_path = path.clone();
            let thread = std::thread::Builder::new()
                .name(format!("polar-sqlite-reader-{worker_index}"))
                .spawn(move || match open_reader(&worker_path, busy_timeout) {
                    Ok(connection) => {
                        let _ = ready_tx.send(Ok(()));
                        while let Some(command) = receiver.blocking_recv() {
                            match command {
                                ReaderCommand::Execute(operation, response) => {
                                    let _ = response.send(operation(&connection));
                                }
                                ReaderCommand::Shutdown(done) => {
                                    let _ = done.send(());
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(SqliteRuntimeError::Driver(error)));
                    }
                })
                .map_err(|_| SqliteRuntimeError::Closed)?;
            match ready_rx.recv().map_err(|_| SqliteRuntimeError::Closed)? {
                Ok(()) => workers.push(ReaderWorker {
                    sender,
                    thread: Mutex::new(Some(thread)),
                }),
                Err(error) => {
                    drop(sender);
                    let _ = thread.join();
                    return Err(error);
                }
            }
        }
        Ok(Self {
            inner: Arc::new(ReaderWorkerPoolInner {
                workers,
                next_worker: AtomicUsize::new(0),
                accepting: AtomicBool::new(true),
            }),
        })
    }

    pub(crate) async fn read_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        let _operation_id = operation_id;
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(SqliteRuntimeError::Closed);
        }
        let index =
            self.inner.next_worker.fetch_add(1, Ordering::Relaxed) % self.inner.workers.len();
        let worker = &self.inner.workers[index];
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .sender
            .send(ReaderCommand::Execute(
                box_operation(operation),
                response_tx,
            ))
            .await
            .map_err(|_| SqliteRuntimeError::Closed)?;
        downcast(
            response_rx
                .await
                .map_err(|_| SqliteRuntimeError::Closed)??,
        )
    }

    pub(crate) fn read_blocking_with_operation<T, F>(
        &self,
        operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                return tokio::task::block_in_place(|| {
                    self.read_blocking_inner(operation_id, operation)
                });
            }
            let pool = self.clone();
            let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
            let _join = tokio::task::spawn_blocking(move || {
                let result = pool.read_blocking_inner(operation_id, operation);
                let _ = result_tx.send(result);
            });
            return result_rx.recv().map_err(|_| SqliteRuntimeError::Closed)?;
        }
        self.read_blocking_inner(operation_id, operation)
    }

    fn read_blocking_inner<T, F>(
        &self,
        _operation_id: StorageOperationId,
        operation: F,
    ) -> Result<T, SqliteRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(SqliteRuntimeError::Closed);
        }
        let index =
            self.inner.next_worker.fetch_add(1, Ordering::Relaxed) % self.inner.workers.len();
        let worker = &self.inner.workers[index];
        let (response_tx, response_rx) = oneshot::channel();
        worker
            .sender
            .blocking_send(ReaderCommand::Execute(
                box_operation(operation),
                response_tx,
            ))
            .map_err(|_| SqliteRuntimeError::Closed)?;
        downcast(
            response_rx
                .blocking_recv()
                .map_err(|_| SqliteRuntimeError::Closed)??,
        )
    }

    pub(crate) async fn shutdown(&self) -> Result<(), SqliteRuntimeError> {
        if !self.inner.accepting.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        for worker in &self.inner.workers {
            let (done_tx, done_rx) = oneshot::channel();
            worker
                .sender
                .send(ReaderCommand::Shutdown(done_tx))
                .await
                .map_err(|_| SqliteRuntimeError::Closed)?;
            done_rx.await.map_err(|_| SqliteRuntimeError::Closed)?;
        }
        for worker in &self.inner.workers {
            let thread = { worker.thread.lock().expect("reader thread lock").take() };
            if let Some(thread) = thread {
                tokio::task::spawn_blocking(move || thread.join())
                    .await
                    .map_err(|_| SqliteRuntimeError::Closed)?
                    .map_err(|_| SqliteRuntimeError::Closed)?;
            }
        }
        Ok(())
    }
}

fn open_reader(path: &PathBuf, busy_timeout: Duration) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(busy_timeout)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(connection)
}

fn box_operation<T, F>(operation: F) -> ReaderOperation
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, rusqlite::Error> + Send + 'static,
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
