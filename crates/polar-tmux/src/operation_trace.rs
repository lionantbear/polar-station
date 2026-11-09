//! Opt-in operation timings on the existing tracing API. No terminal payloads.
//! Producers only enqueue bounded records; a private writer owns disk I/O.
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Once, OnceLock,
    },
};
use tracing::{
    field::{Field, Visit},
    span::{Attributes, Id, Record},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, registry::LookupSpan, Layer};

const TARGET: &str = "polar_station_operation";
const QUEUE_CAPACITY: usize = 4096;
const EVENT_LIMIT: u64 = 65_536;
const RETENTION_FILES: usize = 3;
const CALL_NAME_LIMIT: usize = 512;
type Fields = BTreeMap<String, Value>;

/// Same OS monotonic clock for Host and native adapters on this machine.
pub fn monotonic_ns() -> u64 {
    #[cfg(unix)]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a valid writable timespec and an OS constant clock ID.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } == 0 {
            return (time.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(time.tv_nsec as u64);
        }
    }
    0
}

fn clock_domain() -> String {
    use sha2::{Digest, Sha256};
    let boot = std::fs::read("/proc/sys/kernel/random/boot_id").ok();
    #[cfg(target_os = "macos")]
    let boot = boot.or_else(|| {
        let mut value = [0_u8; 128];
        let mut size = value.len();
        // SAFETY: sysctl writes at most size bytes into this array; no new value.
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.bootsessionuuid".as_ptr(),
                value.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0 && size <= value.len()).then(|| value[..size].to_vec())
    });
    match boot {
        Some(boot) => format!("boot-{:x}", Sha256::digest(boot)),
        // Never pretend unrelated process clocks have a shared identity.
        None => format!("process-{}-{}", std::process::id(), monotonic_ns()),
    }
}

#[derive(Clone, Serialize)]
struct TraceRecord {
    sequence: u64,
    timestamp_ns: u64,
    kind: &'static str,
    span_id: u64,
    parent_span_id: Option<u64>,
    name: String,
    fields: Fields,
}

enum TraceMessage {
    Record(TraceRecord),
    Finish(mpsc::Sender<()>),
}

static PROCESS_SINK: OnceLock<Arc<Sink>> = OnceLock::new();

struct Sink {
    tx: mpsc::SyncSender<TraceMessage>,
    next: AtomicU64,
    dropped: Arc<AtomicU64>,
}
impl Sink {
    fn emit(&self, mut record: TraceRecord) {
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        record.sequence = sequence;
        record.timestamp_ns = monotonic_ns();
        if self.tx.try_send(TraceMessage::Record(record)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
struct SpanData {
    id: u64,
    parent: Option<u64>,
    name: String,
    fields: Fields,
}
impl SpanData {
    fn record(&self, kind: &'static str) -> TraceRecord {
        TraceRecord {
            sequence: 0,
            timestamp_ns: 0,
            kind,
            span_id: self.id,
            parent_span_id: self.parent,
            name: self.name.clone(),
            fields: self.fields.clone(),
        }
    }
}

struct TraceLayer {
    sink: Arc<Sink>,
    next_span: AtomicU64,
}
#[derive(Default, Serialize)]
struct CallCounts {
    begins: u64,
    events: u64,
    ends: u64,
}

// Owned by the existing writer. No producer locks or file I/O.
struct TraceWriter {
    directory: PathBuf,
    instance: String,
    clock: String,
    limit: u64,
    index: u64,
    retained: VecDeque<PathBuf>,
    writer: BufWriter<File>,
    written: u64,
    total: u64,
    calls: BTreeMap<String, CallCounts>,
}
impl TraceWriter {
    fn create_file(
        directory: &Path,
        instance: &str,
        index: u64,
    ) -> io::Result<(PathBuf, BufWriter<File>)> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = directory.join(format!("operations-{instance}-part-{index:08}.jsonl"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok((path, BufWriter::new(file)))
    }

    fn open(directory: &Path, limit: u64) -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let instance = format!("{}-{}", std::process::id(), monotonic_ns());
        let (path, writer) = Self::create_file(directory, &instance, 0)?;
        let mut result = Self {
            directory: directory.into(),
            instance,
            clock: clock_domain(),
            limit: limit.max(1),
            index: 0,
            retained: VecDeque::from([path]),
            writer,
            written: 0,
            total: 0,
            calls: BTreeMap::new(),
        };
        result.header(1, 0)?;
        Ok(result)
    }

    fn header(&mut self, first_sequence: u64, dropped: u64) -> io::Result<()> {
        serde_json::to_writer(
            &mut self.writer,
            &serde_json::json!({
            "schema_version":"polar.station.operation-trace.v1", "clock_domain":self.clock,
            "process_id":std::process::id(), "process_instance":self.instance,
            "event_limit":self.limit, "file_index":self.index, "first_sequence":first_sequence,
            "retention_files":RETENTION_FILES, "prior_written_events":self.total,
            "prior_cumulative_calls":self.calls, "dropped_events":dropped}),
        )?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    fn seal(&mut self, dropped: u64) -> io::Result<()> {
        serde_json::to_writer(
            &mut self.writer,
            &serde_json::json!({"kind":"sealed",
            "written_events":self.written, "total_written_events":self.total,
            "cumulative_calls":self.calls, "dropped_events":dropped, "timestamp_ns":monotonic_ns()}),
        )?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    fn record(&mut self, record: TraceRecord, dropped: u64) -> io::Result<()> {
        if self.written == self.limit {
            self.seal(dropped)?;
            self.index += 1;
            let (path, writer) = Self::create_file(&self.directory, &self.instance, self.index)?;
            self.writer = writer;
            self.written = 0;
            self.retained.push_back(path);
            self.header(record.sequence, dropped)?;
            if self.retained.len() > RETENTION_FILES {
                if let Some(oldest) = self.retained.pop_front() {
                    std::fs::remove_file(oldest)?;
                }
            }
        }
        serde_json::to_writer(&mut self.writer, &record)?;
        self.writer.write_all(b"\n")?;
        self.written += 1;
        self.total += 1;
        let key = if self.calls.contains_key(&record.name) || self.calls.len() < CALL_NAME_LIMIT {
            record.name
        } else {
            "__other_names__".into()
        };
        let counts = self.calls.entry(key).or_default();
        let counter = match record.kind {
            "begin" => &mut counts.begins,
            "end" => &mut counts.ends,
            _ => &mut counts.events,
        };
        *counter = counter.saturating_add(1);
        Ok(())
    }

    fn checkpoint(&mut self, dropped: u64) -> io::Result<()> {
        serde_json::to_writer(
            &mut self.writer,
            &serde_json::json!({"kind":"checkpoint",
            "written_events":self.written,"total_written_events":self.total,"dropped_events":dropped}),
        )?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}

impl TraceLayer {
    fn open(directory: &Path, limit: u64) -> io::Result<Self> {
        let mut writer = TraceWriter::open(directory, limit)?;
        let (tx, rx) = mpsc::sync_channel::<TraceMessage>(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer_dropped = dropped.clone();
        std::thread::Builder::new()
            .name("station-trace".into())
            .spawn(move || {
                let mut write = || -> io::Result<()> {
                    while let Ok(first) = rx.recv() {
                        for message in std::iter::once(first).chain(rx.try_iter().take(255)) {
                            match message {
                                TraceMessage::Record(record) => {
                                    writer.record(record, writer_dropped.load(Ordering::Relaxed))?
                                }
                                TraceMessage::Finish(done) => {
                                    writer.seal(writer_dropped.load(Ordering::Relaxed))?;
                                    let _ = done.send(());
                                    return Ok(());
                                }
                            }
                        }
                        writer.checkpoint(writer_dropped.load(Ordering::Relaxed))?;
                    }
                    writer.seal(writer_dropped.load(Ordering::Relaxed))
                };
                if let Err(error) = write() {
                    eprintln!("Station operation trace write failed: {error}");
                }
            })?;
        Ok(Self {
            sink: Arc::new(Sink {
                tx,
                next: AtomicU64::new(1),
                dropped,
            }),
            next_span: AtomicU64::new(1),
        })
    }
}

fn finish_sink(sink: &Sink) {
    let (done, completed) = mpsc::channel();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut message = TraceMessage::Finish(done);
    loop {
        match sink.tx.try_send(message) {
            Ok(()) => {
                let _ = completed
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()));
                break;
            }
            Err(mpsc::TrySendError::Full(returned)) if std::time::Instant::now() < deadline => {
                message = returned;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            _ => break,
        }
    }
}

pub(crate) extern "C" fn finish_process_trace() {
    if let Some(sink) = PROCESS_SINK.get() {
        finish_sink(sink);
    }
}

#[derive(Default)]
struct FieldVisitor(Fields);
impl FieldVisitor {
    fn put(&mut self, field: &Field, value: Value) {
        if matches!(
            field.name(),
            "component"
                | "reason"
                | "section"
                | "count"
                | "event"
                | "outcome"
                | "operation_id"
                | "request_id"
                | "consumer_id"
                | "resource_id"
                | "catalog_revision"
                | "old_generation"
                | "new_generation"
                | "errno"
                | "exit_code"
                | "target_id"
                | "generation"
                | "content_generation"
                | "transport_generation"
                | "attempt"
                | "network_revision"
                | "bytes"
                | "queue_depth"
                | "rows"
                | "columns"
                | "pixel_width"
                | "pixel_height"
                | "command"
                | "sequence"
                | "native_instance_id"
                | "viewport_revision"
                | "window_id"
                | "window_columns"
                | "window_rows"
                | "ordinal"
                | "focused"
                | "unchanged"
                | "child_pid"
                | "frame_counter"
                | "presentation_ns"
                | "live"
                | "started"
                | "completed"
                | "failed"
                | "signals"
                | "signal_failed"
                | "reaped_pid"
                | "lifetime_us"
                | "stop_wait_us"
                | "wait_us"
                | "work_us"
        ) {
            self.0.insert(field.name().into(), value);
        }
    }
}
impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Value::String(value.chars().take(128).collect()));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, value.into());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, value.into());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Instrumentation only passes bounded typed identities via debug fields.
        if matches!(field.name(), "consumer_id" | "request_id" | "operation_id") {
            self.record_str(field, &format!("{value:?}"));
        }
    }
}
impl<S> Layer<S> for TraceLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().target() != TARGET {
            return;
        }
        let Some(span) = ctx.span(id) else {
            return;
        };
        let parent = span
            .scope()
            .skip(1)
            .find_map(|s| s.extensions().get::<SpanData>().cloned());
        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);
        if let Some(parent) = &parent {
            for key in [
                "operation_id",
                "request_id",
                "consumer_id",
                "target_id",
                "resource_id",
                "catalog_revision",
                "generation",
                "old_generation",
                "new_generation",
                "section",
                "reason",
            ] {
                if let Some(value) = parent.fields.get(key) {
                    visitor.0.entry(key.into()).or_insert_with(|| value.clone());
                }
            }
        }
        let data = SpanData {
            id: self.next_span.fetch_add(1, Ordering::Relaxed),
            parent: parent.map(|p| p.id),
            name: attrs.metadata().name().into(),
            fields: visitor.0,
        };
        self.sink.emit(data.record("begin"));
        span.extensions_mut().insert(data);
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(data) = extensions.get_mut::<SpanData>() {
            let mut visitor = FieldVisitor::default();
            values.record(&mut visitor);
            data.fields.extend(visitor.0);
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            if let Some(data) = span.extensions().get::<SpanData>() {
                self.sink.emit(data.record("end"));
            }
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut fields = FieldVisitor::default();
        event.record(&mut fields);
        let parent = ctx
            .event_scope(event)
            .and_then(|mut scope| scope.find_map(|s| s.extensions().get::<SpanData>().cloned()));
        if let Some(parent) = &parent {
            for key in [
                "operation_id",
                "request_id",
                "consumer_id",
                "target_id",
                "resource_id",
                "catalog_revision",
                "generation",
                "old_generation",
                "new_generation",
                "section",
                "reason",
            ] {
                if let Some(value) = parent.fields.get(key) {
                    fields.0.entry(key.into()).or_insert_with(|| value.clone());
                }
            }
        }
        let name = fields
            .0
            .remove("event")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| event.metadata().name().into());
        self.sink.emit(TraceRecord {
            sequence: 0,
            timestamp_ns: 0,
            kind: "event",
            span_id: 0,
            parent_span_id: parent.map(|p| p.id),
            name,
            fields: fields.0,
        });
    }
}

/// Host and the embedded desktop each install the same opt-in trace layer once.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let layer = std::env::var_os("POLAR_STATION_TRACE_DIR").and_then(|path| {
            match TraceLayer::open(Path::new(&path), EVENT_LIMIT) {
                Ok(layer) => {
                    let _ = PROCESS_SINK.set(layer.sink.clone());
                    // SAFETY: static C callback; it only drains this process's bounded trace queue.
                    unsafe {
                        libc::atexit(finish_process_trace);
                    }
                    Some(layer)
                }
                Err(error) => {
                    eprintln!("Station operation trace unavailable: {error}");
                    None
                }
            }
        });
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_filter(tracing_subscriber::EnvFilter::from_default_env()),
            )
            .with(
                layer.with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                    matches!(meta.target(), TARGET | "polar_dictate::latency")
                })),
            )
            .try_init();
    });
}


