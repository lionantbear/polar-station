//! Bounded bridge from blocking native reads to the existing CAS AsyncRead port.
//! One outstanding read per stream, with an explicit Host-shared job budget.
use std::{
    future::Future,
    io::{self, Read},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    runtime::Handle,
    sync::{AcquireError, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

/// Share this budget across a Host's native location bindings. Buffers are
/// bounded per live stream; only max_jobs native jobs may be outstanding.
#[derive(Clone)]
pub struct ReadBudget {
    permits: Arc<Semaphore>,
    chunk_bytes: usize,
}
impl ReadBudget {
    pub fn new(max_jobs: usize, chunk_bytes: usize) -> polar_cas::Result<Self> {
        if !(1..=128).contains(&max_jobs) || !(1..=1024 * 1024).contains(&chunk_bytes) {
            return Err(polar_cas::Error::Invalid(
                "native read budget out of range".into(),
            ));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(max_jobs)),
            chunk_bytes,
        })
    }
    /// Hold one slot for an entire blocking native operation, including a
    /// bounded multi-member open. The worker must own it until it finishes.
    pub async fn acquire(&self) -> polar_cas::Result<OwnedSemaphorePermit> {
        self.permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| polar_cas::Error::Backend("native read budget closed".into()))
    }
}
type PermitFuture =
    Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;
type ReadJob<R> = JoinHandle<(R, Vec<u8>, io::Result<usize>)>;

/// R owns its native member AND prepared file/pin. An in-flight job owns R and
/// the permit: dropping the AsyncRead cannot unpin an active native operation.
pub(crate) struct BlockingReader<R> {
    reader: Option<R>,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    error: Option<(io::ErrorKind, String, bool)>,
    waiting: Option<PermitFuture>,
    pending: Option<ReadJob<R>>,
    budget: ReadBudget,
    runtime: Handle,
}
impl<R: Read + Send + Unpin + 'static> BlockingReader<R> {
    pub(crate) fn new(reader: R, budget: ReadBudget) -> io::Result<Self> {
        let runtime = Handle::try_current().map_err(io::Error::other)?;
        Ok(Self {
            reader: Some(reader),
            buffer: Vec::new(),
            start: 0,
            end: 0,
            eof: false,
            error: None,
            waiting: None,
            pending: None,
            budget,
            runtime,
        })
    }
    fn fail(&mut self, error: io::Error) -> Poll<io::Result<()>> {
        let integrity = matches!(
            error
                .get_ref()
                .and_then(|e| e.downcast_ref::<polar_cas::Error>()),
            Some(polar_cas::Error::Integrity)
        );
        self.error = Some((error.kind(), error.to_string(), integrity));
        self.waiting = None;
        self.reader = None;
        Poll::Ready(Err(error))
    }
}
impl<R: Read + Send + Unpin + 'static> AsyncRead for BlockingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if let Some((kind, message, integrity)) = &this.error {
                let error = if *integrity {
                    io::Error::new(*kind, polar_cas::Error::Integrity)
                } else {
                    io::Error::new(*kind, message.clone())
                };
                return Poll::Ready(Err(error));
            }
            if this.start < this.end {
                let n = out.remaining().min(this.end - this.start);
                out.put_slice(&this.buffer[this.start..this.start + n]);
                this.start += n;
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            if let Some(job) = this.pending.as_mut() {
                let completed = match Pin::new(job).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => result,
                };
                this.pending = None;
                match completed {
                    Err(error) => {
                        return this.fail(io::Error::other(format!(
                            "native read worker failed: {error}"
                        )))
                    }
                    Ok((reader, buffer, result)) => {
                        this.reader = Some(reader);
                        this.buffer = buffer;
                        match result {
                            Err(error) => return this.fail(error),
                            Ok(n) if n > this.buffer.len() => {
                                return this.fail(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "native reader over-reported length",
                                ))
                            }
                            Ok(n) => {
                                this.start = 0;
                                this.end = n;
                                this.eof = n == 0;
                            }
                        }
                    }
                }
                continue;
            }
            if this.waiting.is_none() {
                this.waiting = Some(Box::pin(this.budget.permits.clone().acquire_owned()));
            }
            let permit = match this
                .waiting
                .as_mut()
                .expect("initialized permit future")
                .as_mut()
                .poll(cx)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => {
                    return this.fail(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "read budget closed",
                    ))
                }
                Poll::Ready(Ok(permit)) => permit,
            };
            this.waiting = None;
            let Some(mut reader) = this.reader.take() else {
                return this.fail(io::Error::other("native reader state lost"));
            };
            let mut buffer = std::mem::take(&mut this.buffer);
            buffer.resize(this.budget.chunk_bytes, 0);
            this.pending = Some(this.runtime.spawn_blocking(move || {
                let _permit = permit;
                let result = loop {
                    match reader.read(&mut buffer) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        result => break result,
                    }
                };
                (reader, buffer, result)
            }));
        }
    }
}

/// Reuse the bounded native bridge for an independently checked pack member.
impl crate::Member {
    pub fn into_async_reader(self, budget: ReadBudget) -> std::io::Result<polar_cas::Reader> {
        Ok(Box::pin(BlockingReader::new(self, budget)?))
    }
}


