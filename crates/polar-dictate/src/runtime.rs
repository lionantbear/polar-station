//! One Host-owned worker handles saved audio independently of capture/insertion.
use crate::{AudioBlock, CaptureState, Change, DictateStore, Recording, SaveReceipt, SegmentState};
use anyhow::{ensure, Result};
use async_trait::async_trait;
use polar_voice::{VoiceRuntime, VoiceTranscribeRequest};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, Notify};

/// The Host supplies a scoped, tool-free text editor using its trusted owner.
#[async_trait]
pub trait TranscriptCleanup: Send + Sync {
    async fn clean(&self, request: polar_text_edit::EditRequest) -> Result<String>;
    async fn clean_recording(&self, _recording_id: &str, request: polar_text_edit::EditRequest) -> Result<String> {
        self.clean(request).await
    }
    async fn release_recording(&self, _recording_id: &str) {}
}
pub struct DictateRuntime {
    store: Arc<dyn DictateStore>,
    voice: Arc<VoiceRuntime>,
    cleanup: Option<Arc<dyn TranscriptCleanup>>,
    settings: Option<Arc<dyn Fn() -> Result<crate::VoiceSettings> + Send + Sync>>,
    processing: Mutex<HashSet<String>>,
    contexts: Mutex<HashMap<String, String>>,
    inputs: Mutex<HashMap<String, Arc<CaptureInput>>>,
    wake: Notify,
    events: broadcast::Sender<Recording>,
}
/// Relocated native PCM tail. It owns no durable truth or second audio queue.
#[derive(Default)]
struct CaptureInput {
    audio: tokio::sync::Mutex<CaptureAudio>,
    partial: Mutex<String>,
}
#[derive(Default)]
struct CaptureAudio {
    pending: Vec<u8>,
    segmenter: crate::segmentation::PauseSegmenter,
}
struct ProcessingGuard<'a> {
    ids: &'a Mutex<HashSet<String>>,
    id: String,
}
impl Drop for ProcessingGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut ids) = self.ids.lock() {
            ids.remove(&self.id);
        }
    }
}
impl DictateRuntime {
    /// Subscribe before reading, so a result cannot be lost between snapshot
    /// and event admission. Transcription and cleanup are separate UI stages.
    pub async fn wait(&self, id: &str, stage: crate::api::WaitStage) -> Result<crate::Recording> {
        let mut events = self.subscribe();
        loop {
            let record = self.store().get(id).await?;
            let original_ready = record.original().is_some();
            let cleanup_ready = !record.cleanup_running
                && (record.cleanup_deferred
                    || !record.cleanup_enabled
                    || record.cleaned.is_some()
                    || record.cleanup_error.is_some());
            if (record.capture != crate::CaptureState::Recording && record.saved_samples == 0)
                || !record.processing_enabled
                || record.error.is_some()
                || (original_ready
                    && (matches!(stage, crate::api::WaitStage::Transcription) || cleanup_ready))
            {
                return Ok(record);
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => anyhow::bail!("Dictate service stopped; saved audio retained"),
            }
        }
    }

    pub fn new(store: Arc<dyn DictateStore>, voice: Arc<VoiceRuntime>) -> Self {
        Self {
            store,
            voice,
            cleanup: None,
            settings: None,
            processing: Mutex::new(Default::default()),
            contexts: Mutex::new(Default::default()),
            inputs: Mutex::new(Default::default()),
            wake: Notify::new(),
            events: broadcast::channel(64).0,
        }
    }
    /// Reuse the injected tool-free text editor without creating an audio record.
    pub async fn refine_text(&self, request: polar_text_edit::EditRequest) -> Result<polar_text_edit::EditResult> {
        ensure!(!request.original.trim().is_empty() && request.original.len() <= crate::MAX_TEXT_BYTES, "text.edit_invalid_input");
        let operation_id = request.operation_id.clone();
        let base_revision = request.base_revision;
        let text = tokio::time::timeout(std::time::Duration::from_secs(30), self.cleanup_text(request)).await??;
        ensure!(!text.trim().is_empty() && text.len() <= crate::MAX_TEXT_BYTES && !text.contains('\0'), "text.edit_invalid_output");
        Ok(polar_text_edit::EditResult { operation_id, base_revision, text })
    }

    pub fn with_voice_settings(mut self, settings: Arc<dyn Fn() -> Result<crate::VoiceSettings> + Send + Sync>) -> Self {
        self.settings = Some(settings);
        self
    }
    async fn cleanup_text(&self, mut request: polar_text_edit::EditRequest) -> Result<String> {
        if let (Some(settings), polar_text_edit::EditKind::CleanupTranscript { context }) = (&self.settings, &mut request.edit) {
            *context = settings()?.cleanup_context(context.take())?;
        }
        self.cleanup.as_ref().ok_or_else(|| anyhow::anyhow!("dictate.cleanup_not_configured"))?.clean(request).await
    }
    pub fn with_cleanup(mut self, cleanup: Arc<dyn TranscriptCleanup>) -> Self {
        self.cleanup = Some(cleanup);
        self
    }
    pub(crate) fn cleanup_provider(&self) -> Result<Arc<dyn TranscriptCleanup>> {
        self.cleanup.clone().ok_or_else(|| anyhow::anyhow!("dictate.cleanup_not_configured"))
    }
    pub(crate) fn voice(&self) -> &VoiceRuntime { &self.voice }
    pub(crate) fn recording_context(&self, id: &str) -> Result<Option<String>> {
        Ok(self.contexts.lock().map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?.get(id).cloned())
    }
    pub fn settings_changed(&self) { self.wake.notify_one(); }
    pub fn subscribe(&self) -> broadcast::Receiver<Recording> {
        self.events.subscribe()
    }
    pub fn store(&self) -> &Arc<dyn DictateStore> {
        &self.store
    }
    pub async fn create(&self, id: String, now_ms: u64, is_test: bool) -> Result<Recording> {
        let record = self
            .store
            .create(Recording::new(id, now_ms, is_test)?)
            .await?;
        self.publish(&record);
        Ok(record)
    }
    pub async fn create_configured(&self, mut record: Recording) -> Result<Recording> {
        if let Some(settings) = &self.settings {
            let settings = settings()?;
            record.transcription_mode = settings.transcription_mode;
            record.glossary = settings.glossary;
        }
        let context = record.context.clone();
        let record = self.store.create(record).await?;
        if record.capture == CaptureState::Recording {
            if let Some(context) = context {
                self.contexts
                    .lock()
                    .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
                    .insert(record.id.clone(), context);
            }
        }
        self.publish(&record);
        self.wake.notify_one();
        Ok(record)
    }
    pub async fn delete_recording(&self, id: &str) -> Result<()> {
        self.store.delete(id).await?;
        self.inputs.lock().map_err(|_|anyhow::anyhow!("dictate.capture_unavailable"))?.remove(id);
        self.contexts.lock().map_err(|_|anyhow::anyhow!("dictate.context_unavailable"))?.remove(id);
        Ok(())
    }
    pub async fn bind_context(&self, id: &str, context: String) -> Result<Recording> {
        // Install ephemeral data before making work runnable. The persisted digest
        // fences concurrent target bindings; mismatching context is never used.
        self.contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
            .insert(id.to_owned(), context.clone());
        let result = self.store.change(id, Change::BindContext { context }).await;
        let record = result?;
        self.publish(&record);
        self.wake.notify_one();
        Ok(record)
    }
    fn capture_input(&self, id: &str) -> Result<Arc<CaptureInput>> {
        Ok(self.inputs.lock().map_err(|_| anyhow::anyhow!("dictate.capture_unavailable"))?
            .entry(id.into()).or_default().clone())
    }
    /// Native PCM feeds do not define the durability or transcription boundary.
    pub async fn feed(&self, id: &str, pcm: Vec<u8>) -> Result<SaveReceipt> {
        ensure!(!pcm.is_empty() && pcm.len().is_multiple_of(2) && pcm.len() <= 32000, "dictate.invalid_audio_feed");
        self.store.get(id).await?; // Validate before allocating a capture resource.
        let input = self.capture_input(id)?;
        let mut input = input.audio.lock().await;
        let record = self.store.get(id).await?;
        ensure!(record.capture == CaptureState::Recording, "dictate.capture_stopped");
        ensure!(record.saved_samples + (input.pending.len() + pcm.len()) as u64 / 2 <= crate::MAX_RECORDING_SAMPLES, "dictate.recording_limit");
        input.pending.extend_from_slice(&pcm);
        while input.pending.len() >= 32000 {
            let record = self.store.get(id).await?;
            let block = AudioBlock { recording_id: id.into(), sequence: record.next_sequence,
                start_sample: record.saved_samples, pcm: input.pending[..32000].to_vec() };
            self.save_block(block, &mut input.segmenter).await?;
            input.pending.drain(..32000);
        }
        let record = self.store.get(id).await?;
        self.publish(&record); // Wakes the live reader even without a durable block.
        self.wake.notify_one();
        Ok(record.receipt())
    }
    pub async fn append(&self, block: AudioBlock) -> Result<SaveReceipt> {
        self.store.get(&block.recording_id).await?;
        let input = self.capture_input(&block.recording_id)?;
        let mut input = input.audio.lock().await;
        ensure!(input.pending.is_empty(), "dictate.mixed_audio_ingress");
        self.save_block(block, &mut input.segmenter).await
    }
    async fn save_block(&self, block: AudioBlock, segmenter: &mut crate::segmentation::PauseSegmenter) -> Result<SaveReceipt> {
        let id = block.recording_id.clone();
        let mut receipt = self.store.append(block.clone()).await?;
        let mut record = self.store.get(&id).await?;
        if record.transcription_mode == crate::TranscriptionMode::Segmented {
            for through_sample in segmenter.append(block.start_sample, &block.pcm) {
                record = self.store.change(&id, Change::SealSegment { through_sample }).await?;
            }
        }
        receipt.revision = record.revision;
        receipt.transcript_preview = record.receipt().transcript_preview;
        self.publish(&record);
        self.wake.notify_one();
        Ok(receipt)
    }
    pub(crate) async fn capture_audio(&self, id: &str, cursor: u64) -> Result<(Recording, Vec<u8>, u64)> {
        let input = self.capture_input(id)?;
        let input = input.audio.lock().await;
        let record = self.store.get(id).await?;
        let available = record.saved_samples + input.pending.len() as u64 / 2;
        ensure!(cursor <= available, "dictate.audio_cursor_invalid");
        let pcm = if cursor < record.saved_samples {
            self.store.audio(id, cursor, (record.saved_samples-cursor).min(crate::MAX_BLOCK_SAMPLES)).await?
        } else {
            input.pending[((cursor-record.saved_samples)*2) as usize..].to_vec()
        };
        Ok((record, pcm, available))
    }
    pub(crate) fn set_preview_partial(&self, id: &str, partial: &str) -> Result<()> {
        let input=self.capture_input(id)?;
        *input.partial.lock().map_err(|_|anyhow::anyhow!("dictate.preview_unavailable"))?=partial.into();
        Ok(())
    }
    pub(crate) fn publish_preview(&self, record: Recording, partial: &str) -> Result<()> {
        self.set_preview_partial(&record.id,partial)?;
        self.publish(&record);
        Ok(())
    }
    pub async fn change(&self, id: &str, change: Change) -> Result<Recording> {
        if matches!(change, Change::Stop { .. }) {
            self.store.get(id).await?;
            let input = self.capture_input(id)?;
            let mut input = input.audio.lock().await;
            if !input.pending.is_empty() {
                let record = self.store.get(id).await?;
                let block = AudioBlock { recording_id: id.into(), sequence: record.next_sequence,
                    start_sample: record.saved_samples, pcm: input.pending.clone() };
                self.save_block(block, &mut input.segmenter).await?;
                input.pending.clear();
            }
        }
        let wakes = matches!(
            change,
            Change::Stop { .. }
                | Change::ResumeProcessing
                | Change::RetrySegmented
                | Change::Accept { .. }
                | Change::CompleteDelivery { .. }
                | Change::Pin { .. }
        );
        let forgets_context = matches!(
            change,
            Change::CancelProcessing | Change::UseOriginal | Change::CleanupFinished { .. }
        );
        let record = self.store.change(id, change).await?;
        if forgets_context {
            if let Ok(mut contexts) = self.contexts.lock() {
                contexts.remove(id);
            }
        }
        self.publish(&record);
        if wakes {
            self.wake.notify_one();
        }
        Ok(record)
    }
    fn preview_projection(&self, record: Recording) -> Recording {
        let mut projection=record;
        let input=self.inputs.lock().ok().and_then(|inputs|inputs.get(&projection.id).cloned());
        if let Some(input)=input {
            if let Ok(partial)=input.partial.lock() {
                let base=projection.receipt().transcript_preview;
                projection.live_preview=Some(if partial.is_empty() {base} else if base.is_empty() {partial.clone()} else {format!("{base} {partial}")});
            };
        }
        projection
    }
    pub async fn preview_snapshot(&self, id: &str) -> Result<Recording> {
        Ok(self.preview_projection(self.store.get(id).await?))
    }
    fn publish(&self, record: &Recording) {
        let _=self.events.send(self.preview_projection(record.clone()));
    }
    /// Event-driven worker, mounted exactly once by the authenticated Host.
    /// Restart recovery disables pending work until the user explicitly retries.
    pub async fn run(&self) -> Result<()> {
        let mut deadline = None;
        self.wake.notify_one();
        loop {
            let expiry = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! { _ = self.wake.notified() => {}, _ = expiry => {} }
            let realtime=self.settings.as_ref().and_then(|settings|settings().ok())
                .is_some_and(|settings|settings.transcription_mode==crate::TranscriptionMode::Realtime);
            if let Err(error)=self.voice.prepare_streaming(realtime).await {
                tracing::warn!(target:"polar_dictate::latency",event="dictate.prepare_failed",?error,"Realtime transport preparation unavailable; capture remains independent");
            }
            let now = now_ms();
            self.store.expire(now).await?;
            deadline = None;
            let mut offset = 0;
            loop {
                let page = self.store.list(offset, 100).await?;
                let end = page.len() < 100;
                for record in page {
                    if let Some(completed) = record.completed_at_ms.filter(|_| {
                        !record.pinned
                            && !record.audio_expired
                            && record.delivery != crate::DeliveryState::Unknown
                    }) {
                        let at = tokio::time::Instant::now()
                            + std::time::Duration::from_millis(
                                completed
                                    .saturating_add(crate::RETENTION_MS)
                                    .saturating_sub(now),
                            );
                        deadline =
                            Some(deadline.map_or(at, |old: tokio::time::Instant| old.min(at)));
                    }
                    if record.processing_enabled && self.process(&record.id).await.is_err() {
                        // A cancelled/deleted record must not kill the Host worker.
                        // Storage failures leave its persisted prefix recoverable.
                        let _ = self.change(&record.id, Change::CancelProcessing).await;
                    }
                }
                if end {
                    break;
                }
                offset += 100;
            }
        }
    }
    pub async fn process(&self, id: &str) -> Result<()> {
        if !self
            .processing
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.worker_unavailable"))?
            .insert(id.to_owned())
        {
            return Ok(());
        }
        let _guard = ProcessingGuard {
            ids: &self.processing,
            id: id.to_owned(),
        };
        let result = self.process_inner(id).await;
        if self.store.get(id).await.is_ok_and(|r|r.capture!=CaptureState::Recording) {
            if let Ok(mut inputs)=self.inputs.lock() {inputs.remove(id);}
        }
        if let Some(cleanup) = &self.cleanup {
            if self.store.get(id).await.is_ok_and(|r| r.capture != CaptureState::Recording || !r.processing_enabled) {
                cleanup.release_recording(id).await;
            }
        }
        result
    }
    async fn process_inner(&self, id: &str) -> Result<()> {
        let initial = self.store.get(id).await?;
        if initial.transcription_mode == crate::TranscriptionMode::Realtime
            && initial.processing_enabled
            && (initial.capture == CaptureState::Recording || initial.segments.iter().any(|s| s.state == SegmentState::Pending)) {
            return self.process_realtime(id).await;
        }
        loop {
            let record = self.store.get(id).await?;
            if !record.processing_enabled {
                return Ok(());
            }
            let Some(segment) = record
                .segments
                .iter()
                .find(|s| s.state == SegmentState::Pending)
            else {
                let context = self
                    .contexts
                    .lock()
                    .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
                    .get(id)
                    .cloned();
                if record.original().is_some() && !record.cleanup_enabled {
                    if let Ok(mut contexts) = self.contexts.lock() {
                        contexts.remove(id);
                    }
                }
                return self.clean(&record, context).await;
            };
            let index = segment.index;
            let running = self.change(id, Change::SegmentStarted { index }).await?;
            let segment = &running.segments[index as usize];
            let attempt = segment.attempt;
            let mut events = self.subscribe();
            let response = async {
                let mut pcm = self
                    .store
                    .audio(id, segment.start_sample, segment.sample_count)
                    .await?;
                pcm.resize(pcm.len().max(polar_voice::MINIMUM_AUDIO_BYTES), 0);
                let request_id = format!("{:x}", Sha256::digest(format!("{id}:{index}:{attempt}")));
                Ok::<_, anyhow::Error>(
                    self.voice
                        .transcribe(VoiceTranscribeRequest::new(request_id, pcm))
                        .await?
                        .text,
                )
            };
            tokio::pin!(response);
            let response = loop {
                tokio::select! {
                    result=&mut response => break result,
                    event=events.recv() => {
                        let current=match event {Ok(record) if record.id==id=>record,Ok(_)=>continue,Err(_)=>self.store.get(id).await?};
                        if !current.processing_enabled || current.segments.get(index as usize).is_none_or(|s|s.attempt!=attempt || s.state!=SegmentState::Running) {return Ok(());}
                    }
                }
            };
            // Cancellation/recovery fences late provider completions.
            let current = self.store.get(id).await?;
            if current.segments[index as usize].attempt != attempt
                || current.segments[index as usize].state != SegmentState::Running
            {
                return Ok(());
            }
            let (text, error) = match response {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error.to_string())),
            };
            let record = self.change(
                id,
                Change::SegmentFinished {
                    index,
                    attempt,
                    text,
                    error,
                },
            )
            .await?;
            if record.processing_enabled {
                let parts = record.segments.iter().take_while(|s| s.state == SegmentState::Complete)
                    .filter_map(|s| s.original.clone()).collect();
                let record = self.change(id, Change::ConfirmTranscript { parts }).await?;
                if record.capture == CaptureState::Recording || !record.cleaned_prefix.is_empty() {
                    self.clean_prefix_once(&record).await?;
                }
            }
        }
    }
    pub async fn clean(&self, record: &Recording, context: Option<String>) -> Result<()> {
        if record.cleanup_deferred
            || !record.cleanup_enabled
            || record.cleaned.is_some()
            || record.cleanup_error.is_some()
            || record.cleanup_running
            || record.delivery != crate::DeliveryState::Pending
        {
            return Ok(());
        }
        let Some(original) = record.original() else {
            return Ok(());
        };
        if record.cleaned_prefix_parts == record.confirmed_transcript.len()
            && !record.cleaned_prefix.is_empty()
            && record.confirmed_transcript.join(" ").trim() == original {
            let running = self.change(&record.id, Change::CleanupStarted).await?;
            self.change(&record.id, Change::CleanupFinished { attempt: running.cleanup_attempt,
                text: Some(record.cleaned_prefix.clone()), error: None }).await?;
            return Ok(());
        }
        let mut events = self.subscribe();
        let running = match self.change(&record.id, Change::CleanupStarted).await {
            Ok(running) => running,
            Err(error) => {
                // Use Original/cancellation may win after the transcript snapshot.
                // Do not turn that successful user action into a worker failure.
                let current = self.store.get(&record.id).await?;
                if !current.cleanup_enabled || !current.processing_enabled
                    || current.delivery != crate::DeliveryState::Pending {
                    return Ok(());
                }
                return Err(error);
            }
        };
        let operation_id = format!(
            "{:x}",
            Sha256::digest(format!("{}:cleanup:{}", record.id, running.cleanup_attempt))
        );
        let request = polar_text_edit::EditRequest {
            operation_id,
            base_revision: running.revision,
            original,
            edit: polar_text_edit::EditKind::CleanupTranscript {
                context: if record.context_enabled {
                    context.filter(|text| {
                        record.context_digest.as_deref()
                            == Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
                    })
                } else {
                    None
                },
            },
        };
        let mut request = request;
        let polar_text_edit::EditKind::CleanupTranscript { context } = &mut request.edit else {
            anyhow::bail!("dictate.invalid_cleanup_kind");
        };
        *context = crate::VoiceSettings { glossary: record.glossary.clone(), context: record.context_enabled, ..Default::default() }
            .cleanup_context(context.take())?;
        let response = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            self.cleanup_provider()?.clean_recording(&record.id, request).await
        });
        tokio::pin!(response);
        let response = loop {
            tokio::select! {
                value = &mut response => break value,
                event = events.recv() => {
                    let current = match event {
                        Ok(current) if current.id == record.id => current,
                        Ok(_) => continue,
                        Err(_) => self.store.get(&record.id).await?,
                    };
                    if !current.cleanup_running || current.cleanup_attempt != running.cleanup_attempt { return Ok(()); }
                }
            }
        };
        let current = self.store.get(&record.id).await?;
        if !current.cleanup_running || current.cleanup_attempt != running.cleanup_attempt {
            return Ok(());
        }
        let (text, error) = match response {
            Ok(Ok(text)) => (Some(text), None),
            Ok(Err(error)) => (None, Some(error.to_string())),
            Err(_) => (
                None,
                Some("dictate.cleanup_timeout; original retained".into()),
            ),
        };
        self.change(
            &record.id,
            Change::CleanupFinished {
                attempt: running.cleanup_attempt,
                text,
                error,
            },
        )
        .await?;
        Ok(())
    }
    pub async fn export_wav(&self, id: &str) -> Result<Vec<u8>> {
        let record = self.store.get(id).await?;
        ensure!(
            record.capture != CaptureState::Recording,
            "dictate.stop_before_export"
        );
        let pcm = self.store.audio(id, 0, record.saved_samples).await?;
        crate::pcm_wav(&pcm)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
