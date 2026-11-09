//! Bounded native capture transport. The Host owns audio, providers and results.
use crate::dictate_client::DictateClient;
use futures_util::future::{AbortHandle, AbortRegistration, Abortable};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::{mpsc, watch};

pub type ProgressCallback = extern "C" fn(*mut std::ffi::c_void, *const u8, usize, u64, *const u8, usize);
#[derive(Clone, Copy)]
struct Progress {
    callback: ProgressCallback,
    context: usize,
}
fn report_progress(
    progress: &Mutex<Option<Progress>>,
    id: &str,
    response: &Value,
    preview: bool,
) -> anyhow::Result<()> {
    let mut receipt: polar_dictate::SaveReceipt = serde_json::from_value(response["receipt"].clone())?;
    anyhow::ensure!(
        receipt.recording_id == id
            && receipt.saved_through_sample <= polar_dictate::MAX_RECORDING_SAMPLES,
        "dictate.invalid_save_receipt"
    );
    if !preview { receipt.transcript_preview.clear(); }
    if let Ok(progress) = progress.lock() {
        if let Some(progress) = *progress {
            (progress.callback)(
                progress.context as *mut std::ffi::c_void,
                id.as_ptr(),
                id.len(),
                receipt.saved_through_sample,
                receipt.transcript_preview.as_ptr(),
                receipt.transcript_preview.len(),
            );
        }
    }
    Ok(())
}
pub struct DictateSpeech {
    progress: Arc<Mutex<Option<Progress>>>,
    playback: Mutex<Vec<tempfile::NamedTempFile>>,
    client: DictateClient,
    runtime: Option<tokio::runtime::Runtime>,
    active: Mutex<Option<SpeechWork>>,
    capture: Mutex<Option<Capture>>,
    drains: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
struct SpeechWork {
    operation_id: String,
    abort: AbortHandle,
    registration: Option<AbortRegistration>,
    cancelled: Arc<AtomicBool>,
    finished: Option<watch::Receiver<bool>>,
}
struct Capture {
    id: String,
    sender: mpsc::Sender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
    error: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<Result<(), String>>,
    cancelled: Arc<AtomicBool>,
    finished: watch::Receiver<bool>,
}
impl DictateSpeech {
    pub fn local() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client: DictateClient::default(),
            progress: Arc::new(Mutex::new(None)),
            playback: Mutex::new(Vec::new()),
            runtime: Some(runtime),
            active: Mutex::new(None),
            capture: Mutex::new(None),
            drains: Mutex::new(Vec::new()),
        })
    }
    
    /// Registration/removal synchronizes with in-flight callbacks. The native
    /// owner unregisters before destroying its borrowed callback context.
    pub fn set_progress(&self, callback: Option<ProgressCallback>, context: usize) {
        if let Ok(mut progress) = self.progress.lock() {
            *progress = callback.map(|callback| Progress { callback, context });
        }
    }
    pub fn request(&self, payload: Value) -> Result<Value, String> {
        if payload.get("op").and_then(Value::as_str) == Some("export_audio") {
            let id = payload
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Missing recording ID")?;
            let start = payload
                .get("start_sample")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let pcm=self.runtime.as_ref().ok_or("Voice client stopped")?.block_on(async {
                let response=self.client.call(json!({"op":"get","id":id})).await?;
                let record:polar_dictate::Recording=serde_json::from_value(response["recording"].clone())?;
                anyhow::ensure!(record.capture!=polar_dictate::CaptureState::Recording && start<=record.saved_samples,"Stop recording before export");
                let end=payload.get("sample_count").and_then(Value::as_u64).map(|n|start.saturating_add(n)).unwrap_or(record.saved_samples).min(record.saved_samples);
                let mut pcm=Vec::new();let mut position=start;
                while position<end {
                    let count=(end-position).min(16000);
                    let response=self.client.call(json!({"op":"audio","id":id,"start_sample":position,"sample_count":count})).await?;
                    let bytes:Vec<u8>=serde_json::from_value(response["pcm"].clone())?;
                    anyhow::ensure!(bytes.len() as u64==count*2,"Saved audio range is incomplete");
                    pcm.extend_from_slice(&bytes);position+=count;
                }
                Ok::<_,anyhow::Error>(pcm)
            }).map_err(|e|e.to_string())?;
            return self.local_wav(&pcm, payload.get("path").and_then(Value::as_str));
        }

        self.runtime
            .as_ref()
            .ok_or("Voice client stopped")?
            .block_on(self.client.call(payload))
            .map_err(|e| e.to_string())
    }
    pub fn local_wav(&self, pcm: &[u8], destination: Option<&str>) -> Result<Value, String> {
        use std::io::Write;
        let wave = polar_dictate::pcm_wav(pcm).map_err(|e| e.to_string())?;
        if let Some(path) = destination {
            let path = std::path::Path::new(path);
            if !path.is_absolute() {
                return Err("Export destination must be absolute".into());
            }
            let parent = path.parent().ok_or("Export destination unavailable")?;
            let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
            file.write_all(&wave).map_err(|e| e.to_string())?;
            file.as_file().sync_all().map_err(|e| e.to_string())?;
            file.persist(path).map_err(|e| e.to_string())?;
            return Ok(json!({"path":path}));
        }
        let mut file = tempfile::Builder::new()
            .prefix("polar-dictate-playback-")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        file.write_all(&wave).map_err(|e| e.to_string())?;
        let path = file.path().to_owned();
        let mut files = self
            .playback
            .lock()
            .map_err(|_| "Voice playback unavailable")?;
        files.clear();
        files.push(file);
        Ok(json!({"path":path}))
    }
    /// Start the durable owner before handing it microphone frames. Context is
    /// captured now; a later focus change cannot change the pending operation.
    pub fn begin(
        &self,
        id: &str,
        context: Option<String>,
        cleanup: bool,
        is_test: bool,
    ) -> Result<(), String> {
        if !polar_dictate::identifier(id) {
            return Err("Invalid Dictate operation".into());
        }
        let mut capture = self.capture.lock().map_err(|_| "Voice client stopped")?;
        if capture.is_some() {
            return Err("A recording is already active".into());
        }
        let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(256);
        let pending = Arc::new(AtomicUsize::new(0));
        let queued = pending.clone();
        let error = Arc::new(Mutex::new(None));
        let failure = error.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_writer = cancelled.clone();
        let (done, finished) = watch::channel(false);
        let operation = id.to_owned();
        let client = self.client.clone();
        let progress = self.progress.clone();
        let task=self.runtime.as_ref().ok_or("Voice client stopped")?.spawn(async move {
            let mut preview=None;
            let result=async {
                client.call(json!({"op":"start","id":operation,"now_ms":now_ms(),"is_test":is_test,"cleanup":cleanup,"context":context})).await?;
                let observer=client.clone();let observer_id=operation.clone();let observer_progress=progress.clone();
                preview=Some(tokio::spawn(async move {
                    observer.observe(&observer_id, |response| report_progress(&observer_progress,&observer_id,&response,true)).await
                }));
                if cancelled_writer.load(Ordering::Acquire) { client.call(json!({"op":"pause","id":operation})).await?; }
                while let Some(chunk)=receiver.recv().await {
                    let size=chunk.len();
                    let response=client.call(json!({"op":"feed","id":operation,"pcm":chunk})).await?;
                    report_progress(&progress,&operation,&response,false)?;
                    queued.fetch_sub(size,Ordering::AcqRel);
                }
                let operation_kind = if cancelled_writer.load(Ordering::Acquire) { "cancel" } else { "stop" };
                let payload=if operation_kind=="cancel" { json!({"op":"cancel","id":operation}) } else {json!({"op":"stop","id":operation,"reason":null})};
                let stopped=client.call(payload).await?;
                if let Ok(record)=serde_json::from_value::<polar_dictate::Recording>(stopped["recording"].clone()) {
                    report_progress(&progress,&operation,&json!({"receipt":record.receipt()}),false)?;
                }
                Ok::<_,anyhow::Error>(())
            }.await.map_err(|e|e.to_string());
            if let Err(message)=&result {
                if let Ok(mut error)=failure.lock(){*error=Some(message.clone());}
                let _=client.call(json!({"op":"stop","id":operation,"reason":"Audio storage stopped; only acknowledged audio is recoverable"})).await;
            }
            if let Some(preview)=preview {
                if result.is_err() { preview.abort(); }
                else if !matches!(preview.await,Ok(Ok(()))) {
                    tracing::warn!("Dictate preview observation ended; durable result remains available");
                }
            }
            let _=done.send(true);
            result
        });
        *capture = Some(Capture {
            id: id.into(),
            sender,
            pending,
            error,
            task,
            cancelled,
            finished,
        });
        Ok(())
    }
    /// Bounded enqueue without network I/O (at most two seconds unacknowledged).
    pub fn append(&self, id: &str, pcm: &[u8]) -> Result<(), String> {
        if pcm.is_empty() || !pcm.len().is_multiple_of(2) || pcm.len() > 32000 {
            return Err("Invalid Dictate PCM audio block".into());
        }
        let capture = self.capture.lock().map_err(|_| "Voice client stopped")?;
        let capture = capture
            .as_ref()
            .filter(|c| c.id == id)
            .ok_or("Recording is not active")?;
        if let Some(error) = capture
            .error
            .lock()
            .map_err(|_| "Voice client stopped")?
            .as_ref()
        {
            return Err(error.clone());
        }
        if capture
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(pcm.len()).filter(|n| *n <= 64000)
            })
            .is_err()
        {
            return Err(
                "Audio storage cannot keep up; recording stopped, saved audio retained".into(),
            );
        }
        if capture.sender.try_send(pcm.to_vec()).is_err() {
            capture.pending.fetch_sub(pcm.len(), Ordering::AcqRel);
            return Err("Audio storage queue is unavailable; saved audio retained".into());
        }
        Ok(())
    }
    pub fn reserve(&self, id: &str) -> Result<(), String> {
        if !polar_dictate::identifier(id) {
            return Err("Invalid Dictate operation".into());
        }
        let mut active = self.active.lock().map_err(|_| "Voice client stopped")?;
        if active.is_some() {
            return Err("Dictate is already processing audio".into());
        }
        let (cancelled, finished) = self
            .capture
            .lock()
            .map_err(|_| "Voice client stopped")?
            .as_ref()
            .filter(|capture| capture.id == id)
            .map(|capture| (capture.cancelled.clone(), Some(capture.finished.clone())))
            .unwrap_or_else(|| (Arc::new(AtomicBool::new(false)), None));
        let (abort, registration) = AbortHandle::new_pair();
        *active = Some(SpeechWork {
            operation_id: id.into(),
            abort,
            registration: Some(registration),
            cancelled,
            finished,
        });
        Ok(())
    }
    pub fn transcribe_reserved(&self, id: String, audio: Vec<u8>) -> Result<String, String> {
        let registration = {
            let mut active = self.active.lock().map_err(|_| "Voice client stopped")?;
            active
                .as_mut()
                .filter(|w| w.operation_id == id)
                .and_then(|w| w.registration.take())
                .ok_or("Dictate operation was not reserved")?
        };
        struct Release<'a>(&'a Mutex<Option<SpeechWork>>);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                if let Ok(mut active) = self.0.lock() {
                    *active = None;
                }
            }
        }
        let _release = Release(&self.active);
        let capture = {
            let mut capture = self.capture.lock().map_err(|_| "Voice client stopped")?;
            if capture.as_ref().is_some_and(|c| c.id == id) {
                capture.take()
            } else {
                None
            }
        };
        self.runtime.as_ref().ok_or("Voice client stopped")?.block_on(Abortable::new(async {
            if let Some(capture)=capture {
                drop(capture.sender); capture.task.await.map_err(|_|"Audio writer stopped; saved audio retained")??;
            }else{
                // Compatibility for an older native capture adapter. New adapters
                // stream blocks from the first second, never use this batch path.
                if !(9600..=19_200_000).contains(&audio.len()) || !audio.len().is_multiple_of(2){return Err("Invalid Dictate PCM audio".into());}
                self.client.call(json!({"op":"start","id":id,"now_ms":now_ms(),"cleanup":true,"context":null})).await.map_err(|e|e.to_string())?;
                for (i,block) in audio.chunks(32000).enumerate(){
                    self.client.call(json!({"op":"append","block":{"recording_id":id,"sequence":i,"start_sample":i*16000,"pcm":block}})).await.map_err(|e|e.to_string())?;
                }
                self.client.call(json!({"op":"stop","id":id,"reason":null})).await.map_err(|e|e.to_string())?;
            }
            let value=self.client.call(json!({"op":"wait","id":id,"stage":"transcription"})).await.map_err(|e|e.to_string())?;
            let mut record:polar_dictate::Recording=serde_json::from_value(value).map_err(|e|e.to_string())?;
            // The isolated Voice test reports both stages together; production
            // capture returns original immediately so Use Original stays live.
            if record.is_test && record.original().is_some() {
                record=serde_json::from_value(self.client.call(json!({"op":"wait","id":id})).await.map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
            }
            record.original().ok_or_else(||record.error.or_else(||record.segments.iter().find_map(|s|s.error.clone()))
                .unwrap_or_else(||"Recording saved. Retry unfinished segments in Settings → Voice → Recordings".into()))
        },registration)).map_err(|_|"Dictate cancelled; saved audio retained".to_string())?
    }
    pub fn cancel(&self, id: &str) {
        let mut finished = None;
        if let Ok(active) = self.active.lock() {
            if let Some(work) = active.as_ref().filter(|w| w.operation_id == id) {
                work.cancelled.store(true, Ordering::Release);
                finished = work.finished.clone();
                work.abort.abort();
            }
        }
        let capture = self.capture.lock().ok().and_then(|mut c| {
            if c.as_ref().is_some_and(|c| c.id == id) {
                c.take()
            } else {
                None
            }
        });
        if let Some(capture) = capture {
            capture.cancelled.store(true, Ordering::Release);
            finished = Some(capture.finished.clone());
            drop(capture.sender);
            // The writer continues draining its bounded queue on the client runtime.
        }
        if let Some(runtime) = &self.runtime {
            let id = id.to_owned();
            let client = self.client.clone();
            let drain = runtime.spawn(async move {
                let _ = client.call(json!({"op":"pause","id":id})).await;
                if let Some(mut finished) = finished {
                    while !*finished.borrow_and_update() {
                        if finished.changed().await.is_err() {
                            break;
                        }
                    }
                }
                let _ = client.call(json!({"op":"cancel","id":id})).await;
            });
            if let Ok(mut drains) = self.drains.lock() {
                drains.retain(|task| !task.is_finished());
                drains.push(drain);
            }
        }
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
impl Drop for DictateSpeech {
    fn drop(&mut self) {
        let id = self
            .capture
            .get_mut()
            .ok()
            .and_then(|c| c.as_ref().map(|c| c.id.clone()));
        if let Some(id) = id {
            self.cancel(&id);
        }
        let id = self
            .active
            .get_mut()
            .ok()
            .and_then(|c| c.as_ref().map(|c| c.operation_id.clone()));
        if let Some(id) = id {
            self.cancel(&id);
        }
        if let Some(runtime) = self.runtime.take() {
            let drains = self
                .drains
                .get_mut()
                .map(std::mem::take)
                .unwrap_or_default();
            // Native teardown waits for the final durable ACK. A wedged Host is
            // bounded; it never turns a missing ACK into a successful save.
            let _ = std::thread::spawn(move || {
                runtime.block_on(async {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        for task in drains {
                            let _ = task.await;
                        }
                    })
                    .await;
                });
                runtime.shutdown_timeout(std::time::Duration::from_millis(100));
            })
            .join();
        }
    }
}
