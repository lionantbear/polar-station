//! One Host-owned worker handles saved audio independently of capture/insertion.
use crate::{AudioBlock, CaptureState, Change, DictateStore, Recording, SaveReceipt, SegmentState};
use anyhow::{ensure, Result};
use async_trait::async_trait;
use polar_voice::VoiceRuntime;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, Notify};
use tracing::Instrument;

/// The Host supplies a scoped, tool-free text editor using its trusted owner.
#[async_trait]
pub trait TranscriptCleanup: Send + Sync {
    async fn clean(&self, request: polar_transcreation::EditRequest) -> Result<String>;
    async fn clean_recording(
        &self,
        _recording_id: &str,
        request: polar_transcreation::EditRequest,
    ) -> Result<String> {
        self.clean(request).await
    }
    async fn release_recording(&self, _recording_id: &str) {}
    /// Optional opportunity preparation; actual processing never depends on it.
    async fn prepare_recording(&self, _recording: &Recording) -> Result<()> {
        Ok(())
    }
}
pub struct DictateRuntime {
    store: Arc<dyn DictateStore>,
    voice: Arc<VoiceRuntime>,
    cleanup: Option<Arc<dyn TranscriptCleanup>>,
    settings: Option<Arc<dyn Fn() -> Result<crate::VoiceSettings> + Send + Sync>>,
    processing: Mutex<HashSet<String>>,
    contexts: Mutex<HashMap<(String, String), String>>,
    inputs: Mutex<HashMap<String, Arc<CaptureInput>>>,
    wake: Notify,
    maintenance_dirty: std::sync::atomic::AtomicBool,
    events: broadcast::Sender<crate::DictateEvent>,
}
/// Relocated native PCM tail. It owns no durable truth or second audio queue.
#[derive(Default)]
struct CaptureInput {
    audio: tokio::sync::Mutex<CaptureAudio>,
    partial: Mutex<String>,
    preparation: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for CaptureInput {
    fn drop(&mut self) {
        if let Ok(task) = self.preparation.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}
#[derive(Default)]
struct CaptureAudio {
    pending: Vec<u8>,
    segmenter: crate::segmentation::PauseSegmenter,
    voice_pcm: Option<polar_voice::pcm::PcmTranscriptionSession>,
    // Disposable Voice-owned audio jobs; Segment/store remains the scheduler
    // and recovery authority. Keep at most two prepared ranges.
    prepared: std::collections::BTreeMap<String, polar_voice::pcm::PcmTranscriptionJob>,
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
                || (matches!(stage, crate::api::WaitStage::Capture) && record.capture != CaptureState::Recording)
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
            maintenance_dirty: std::sync::atomic::AtomicBool::new(true),
            events: broadcast::channel(64).0,
        }
    }
    /// Freeze an explicit user choice, never promote a preview to final ASR.
    pub async fn use_available(&self, id: &str) -> Result<crate::HistoryItem> {
        if matches!(self.store.item(id).await?, crate::HistoryItem::Text(_)) {
            return self.change_item(id, crate::ItemChange::UseAvailable).await;
        }
        // The GUI has already stopped input. Let the existing capture writer
        // acknowledge its tail before cancelling recognition; no timer/replay.
        self.wait(id, crate::api::WaitStage::Capture).await?;
        let record = self.preview_snapshot(id).await?;
        let text = record.text().filter(|s| !s.trim().is_empty()).or_else(|| {
            let preview = record.receipt().transcript_preview;
            (!preview.trim().is_empty()).then_some(preview)
        });
        Ok(crate::HistoryItem::Recording(Box::new(
            self.change(id, Change::UseAvailable { text }).await?
        )))
    }
    /// Reuse the injected tool-free text editor without creating an audio record.
    pub async fn refine_text(
        &self,
        request: polar_transcreation::EditRequest,
    ) -> Result<polar_transcreation::EditResult> {
        ensure!(
            !request.original.trim().is_empty() && request.original.len() <= crate::MAX_TEXT_BYTES,
            "text.edit_invalid_input"
        );
        let operation_id = request.operation_id.clone();
        let base_revision = request.base_revision;
        let mode = if matches!(
            request.edit,
            polar_transcreation::EditKind::CleanupTranslateEnglish { .. }
        ) {
            polar_transcreation::RefineMode::English
        } else {
            polar_transcreation::RefineMode::Cleanup
        };
        let saved = self
            .store
            .text_create(crate::TextEditRecord::new(
                operation_id.clone(),
                request.original.clone(),
                mode,
                now_ms(),
            )?)
            .await?;
        if !saved.running {
            if let Some(text) = saved.result {
                return Ok(polar_transcreation::EditResult {
                    operation_id,
                    base_revision,
                    text,
                });
            }
            anyhow::bail!(
                "{}",
                saved
                    .error
                    .unwrap_or_else(|| "text.edit_interrupted".into())
            );
        }
        let mut events = self.subscribe();
        let claimed = self
            .processing
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.worker_unavailable"))?
            .insert(operation_id.clone());
        if !claimed {
            loop {
                let crate::HistoryItem::Text(current) = self.store.item(&operation_id).await?
                else {
                    anyhow::bail!("dictate.text_id_conflict")
                };
                if !current.running {
                    let current = *current;
                    let error = current.error.unwrap_or_else(|| "text.edit_failed".into());
                    return current
                        .result
                        .map(|text| polar_transcreation::EditResult {
                            operation_id,
                            base_revision,
                            text,
                        })
                        .ok_or_else(|| anyhow::anyhow!(error));
                }
                events.recv().await?;
            }
        }
        let _guard = ProcessingGuard {
            ids: &self.processing,
            id: operation_id.clone(),
        };
        let cleanup = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.cleanup_text(request),
        );
        tokio::pin!(cleanup);
        let response = loop {
            tokio::select! {
                result = &mut cleanup => break result,
                _ = events.recv() => {
                    if !self.store.item(&operation_id).await?.active() {
                        anyhow::bail!("dictate.processing_cancelled");
                    }
                }
            }
        };
        let result = match response {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("text.edit_timeout; original retained".into()),
        };
        let saved = self.store.text_finish(&operation_id, result).await?;
        let _ = self.events.send(crate::DictateEvent::Item(Box::new(
            crate::HistoryItem::Text(Box::new(saved.clone())),
        )));
        let text = saved.result.ok_or_else(|| {
            anyhow::anyhow!(saved.error.unwrap_or_else(|| "text.edit_failed".into()))
        })?;
        Ok(polar_transcreation::EditResult {
            operation_id,
            base_revision,
            text,
        })
    }
    pub fn retention_policy(&self) -> Result<crate::RetentionPolicy> {
        Ok(match &self.settings {
            Some(settings) => settings()?.retention,
            None => Default::default(),
        })
    }
    pub async fn cleanup_history(
        &self,
        plan: &crate::CleanupPlan,
        apply: bool,
    ) -> Result<crate::CleanupReport> {
        if apply {
            ensure!(
                plan.policy == self.retention_policy()? && plan.as_of_ms <= now_ms(),
                "dictate.cleanup_rule_changed; preview again"
            );
        }
        let mut policy = plan.policy;
        policy.enabled = true;
        let report = self.store.cleanup(plan.as_of_ms, policy, apply).await?;
        if report.deleted > 0 {
            let _ = self.events.send(crate::DictateEvent::HistoryInvalidated);
        }
        self.wake.notify_one();
        Ok(report)
    }
    pub async fn change_item(
        &self,
        id: &str,
        change: crate::ItemChange,
    ) -> Result<crate::HistoryItem> {
        let item = self.store.item_change(id, change).await?;
        if let crate::HistoryItem::Recording(record) = &item {
            self.publish(record);
        } else {
            let _ = self
                .events
                .send(crate::DictateEvent::Item(Box::new(item.clone())));
        }
        self.maintenance_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        self.wake.notify_one();
        Ok(item)
    }

    pub fn with_voice_settings(
        mut self,
        settings: Arc<dyn Fn() -> Result<crate::VoiceSettings> + Send + Sync>,
    ) -> Self {
        self.settings = Some(settings);
        self
    }
    async fn cleanup_text(&self, mut request: polar_transcreation::EditRequest) -> Result<String> {
        if let (
            Some(settings),
            polar_transcreation::EditKind::CleanupTranscript { context }
            | polar_transcreation::EditKind::CleanupTranslateEnglish { context },
        ) = (&self.settings, &mut request.edit)
        {
            *context = settings()?.cleanup_context(context.take())?;
        }
        self.cleanup
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("dictate.cleanup_not_configured"))?
            .clean(request)
            .await
    }
    pub fn with_cleanup(mut self, cleanup: Arc<dyn TranscriptCleanup>) -> Self {
        self.cleanup = Some(cleanup);
        self
    }
    pub(crate) fn cleanup_provider(&self) -> Result<Arc<dyn TranscriptCleanup>> {
        self.cleanup
            .clone()
            .ok_or_else(|| anyhow::anyhow!("dictate.cleanup_not_configured"))
    }
    pub(crate) fn voice(&self) -> &Arc<VoiceRuntime> {
        &self.voice
    }
    pub(crate) fn recording_context(&self, record: &Recording) -> Result<Option<String>> {
        let Some(digest) = &record.context_digest else {
            return Ok(None);
        };
        Ok(self
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
            .get(&(record.id.clone(), digest.clone()))
            .cloned())
    }
    pub fn reference_policy(&self, settings: &crate::VoiceSettings) -> crate::ReferencePolicy {
        let mode = match settings.transcription_mode {
            crate::TranscriptionMode::Realtime => polar_voice::pcm::PcmTranscriptionMode::Incremental,
            crate::TranscriptionMode::Segmented => polar_voice::pcm::PcmTranscriptionMode::Buffered,
        };
        settings.reference_policy(self.voice.transcription_reference_support(mode) != polar_voice::TranscriptionReferenceSupport::Unsupported)
    }

    pub(crate) fn transcription_reference(&self, record: &Recording, mode: polar_voice::pcm::PcmTranscriptionMode) -> Result<polar_voice::TranscriptionReference> {
        if self.voice.transcription_reference_support(mode) == polar_voice::TranscriptionReferenceSupport::Unsupported {
            return Ok(polar_voice::TranscriptionReference::default());
        }
        crate::VoiceSettings {
            glossary: record.glossary.clone(), context: record.context_enabled, ..Default::default()
        }.transcription_reference(self.recording_context(record)?, record.reference.clone())
    }

    fn forget_context(&self, id: &str) {
        if let Ok(mut contexts) = self.contexts.lock() {
            contexts.retain(|(recording, _), _| recording != id);
        }
    }
    pub fn settings_changed(&self) {
        self.maintenance_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        self.wake.notify_one();
        let _ = self.events.send(crate::DictateEvent::SettingsChanged);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<crate::DictateEvent> {
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
            let mut settings = settings()?;
            record.transcription_mode = settings.transcription_mode;
            let mode = if record.cleanup_enabled {
                settings.mode()
            } else {
                crate::DictateOutputMode::NoRefine
            };
            record.output_mode = Some(mode);
            record.cleanup_enabled = mode.refines();
            settings.set_output_mode(mode);
            let reference_policy = self.reference_policy(&settings);
            record.cleanup_deferred = record.cleanup_enabled && reference_policy.surrounding
                && record.context.is_none()
                && record.reference.is_empty()
                && !record.is_test;
            if !reference_policy.surrounding {
                record.context = None;
                record.context_enabled = false;
                record.context_digest = record.context.as_ref().map(|value| format!("{:x}", Sha256::digest(value.as_bytes())));
                record.cleanup_deferred = false;
            }
            record.glossary = if reference_policy.terminology { settings.glossary } else { String::new() };
        }
        let context = record.context.clone();
        let record = self.store.create(record).await?;
        if record.capture == CaptureState::Recording {
            if let Some(context) = context {
                self.contexts
                    .lock()
                    .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
                    .insert(
                        (
                            record.id.clone(),
                            record
                                .context_digest
                                .clone()
                                .expect("configured context digest"),
                        ),
                        context,
                    );
            }
        }
        if record.capture == CaptureState::Recording && record.cleanup_enabled {
            if let Some(cleanup) = self.cleanup.clone() {
                let input = self.capture_input(&record.id)?;
                let preparing = record.clone();
                let task = tokio::spawn(async move {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(8),
                        cleanup.prepare_recording(&preparing),
                    )
                    .await;
                });
                if let Ok(mut slot) = input.preparation.lock() {
                    *slot = Some(task);
                };
            }
        }
        self.publish(&record);
        self.wake.notify_one();
        Ok(record)
    }
    pub async fn delete_recording(&self, id: &str) -> Result<()> {
        self.store.delete(id).await?;
        self.inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.capture_unavailable"))?
            .remove(id);
        self.forget_context(id);
        let _ = self.events.send(crate::DictateEvent::Removed(id.into()));
        Ok(())
    }
    pub async fn bind_context(&self, id: &str, context: String) -> Result<Recording> {
        // Serialize bindings with existing per-record capture ownership. Readers
        // resolve only the digest committed by the store, never an uncommitted value.
        let input = self.capture_input(id)?;
        let _binding = input.audio.lock().await;
        let mut candidate = self.store.get(id).await?;
        candidate.apply(Change::BindContext {
            context: context.clone(),
        })?;
        let key = (
            id.to_owned(),
            candidate.context_digest.clone().expect("bound digest"),
        );
        let inserted = {
            let mut contexts = self
                .contexts
                .lock()
                .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?;
            if let std::collections::hash_map::Entry::Vacant(entry) = contexts.entry(key.clone()) {
                entry.insert(context.clone());
                true
            } else {
                false
            }
        };
        let record = match self.store.change(id, Change::BindContext { context }).await {
            Ok(record) => record,
            Err(error) => {
                if inserted {
                    self.contexts
                        .lock()
                        .map_err(|_| anyhow::anyhow!("dictate.context_unavailable"))?
                        .remove(&key);
                }
                return Err(error);
            }
        };
        self.publish(&record);
        self.wake.notify_one();
        Ok(record)
    }
    fn capture_input(&self, id: &str) -> Result<Arc<CaptureInput>> {
        Ok(self
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.capture_unavailable"))?
            .entry(id.into())
            .or_default()
            .clone())
    }
    /// Native PCM feeds do not define the durability or transcription boundary.
    pub async fn feed(&self, id: &str, pcm: Vec<u8>) -> Result<SaveReceipt> {
        ensure!(
            !pcm.is_empty() && pcm.len().is_multiple_of(2) && pcm.len() <= 32000,
            "dictate.invalid_audio_feed"
        );
        self.store.get(id).await?; // Validate before allocating a capture resource.
        let input = self.capture_input(id)?;
        let mut input = input.audio.lock().await;
        let record = self.store.get(id).await?;
        ensure!(
            record.capture == CaptureState::Recording,
            "dictate.capture_stopped"
        );
        ensure!(
            record.saved_samples + (input.pending.len() + pcm.len()) as u64 / 2
                <= crate::MAX_RECORDING_SAMPLES,
            "dictate.recording_limit"
        );
        input.pending.extend_from_slice(&pcm);
        while input.pending.len() >= 32000 {
            let record = self.store.get(id).await?;
            let block = AudioBlock {
                recording_id: id.into(),
                sequence: record.next_sequence,
                start_sample: record.saved_samples,
                pcm: input.pending[..32000].to_vec(),
            };
            self.save_block(block, &mut input).await?;
            input.pending.drain(..32000);
        }
        let record = self.store.get(id).await?;
        self.publish(&record); // Wakes the live reader even without a durable block.
        Ok(record.receipt())
    }
    pub async fn append(&self, block: AudioBlock) -> Result<SaveReceipt> {
        self.store.get(&block.recording_id).await?;
        let input = self.capture_input(&block.recording_id)?;
        let mut input = input.audio.lock().await;
        ensure!(input.pending.is_empty(), "dictate.mixed_audio_ingress");
        self.save_block(block, &mut input).await
    }
    async fn save_block(&self, block: AudioBlock, input: &mut CaptureAudio) -> Result<SaveReceipt> {
        let id = block.recording_id.clone();
        let mut receipt = self.store.append(block.clone()).await?;
        let mut record = self.store.get(&id).await?;
        if record.transcription_mode == crate::TranscriptionMode::Segmented {
            // Persistence precedes opportunistic encoding. A codec-cache failure
            // must never turn acknowledged audio into a duplicated append.
            let encoded = async {
                if input.voice_pcm.is_none() {
                    let start = record
                        .segments
                        .last()
                        .map_or(0, |s| s.start_sample + s.sample_count);
                    // A lost codec cache cannot restart halfway through a
                    // segment and submit only its tail under the full range ID.
                    // Leave that range to the existing durable recovery path.
                    if block.start_sample != start {
                        return Ok(());
                    }
                    input.voice_pcm = Some(
                        self.voice
                            .open_pcm_with_reference(&id, polar_voice::pcm::PcmTranscriptionMode::Buffered, start,
                                self.transcription_reference(&record, polar_voice::pcm::PcmTranscriptionMode::Buffered)
                                    .map_err(|_| polar_voice::VoiceError::InvalidOperation)?)
                            .await?,
                    );
                }
                input
                    .voice_pcm
                    .as_mut()
                    .expect("PCM session initialized")
                    .append_pcm(&block.pcm)
                    .await
            }
            .await;
            if encoded.is_err() {
                input.voice_pcm = None;
            }
            for through_sample in input.segmenter.append(block.start_sample, &block.pcm) {
                record = self
                    .store
                    .change(&id, Change::SealSegment { through_sample })
                    .await?;
                if let (Some(voice), Some(segment)) =
                    (input.voice_pcm.as_mut(), record.segments.last())
                {
                    let request_id = segment_request_id(&id, segment.index, segment.attempt + 1);
                    if let Ok(job) = voice.seal_prefix(request_id.clone(), through_sample) {
                        input.prepared.insert(request_id, job);
                        while input.prepared.len() > 2 {
                            input.prepared.pop_first();
                        }
                    } else {
                        input.voice_pcm = None;
                    }
                }
            }
        }
        receipt.revision = record.revision;
        receipt.transcript_preview = record.receipt().transcript_preview;
        self.publish(&record);
        self.wake.notify_one();
        Ok(receipt)
    }
    pub(crate) async fn capture_audio(
        &self,
        id: &str,
        cursor: u64,
    ) -> Result<(Recording, Vec<u8>, u64)> {
        let input = self.capture_input(id)?;
        let input = input.audio.lock().await;
        let record = self.store.get(id).await?;
        let available = record.saved_samples + input.pending.len() as u64 / 2;
        ensure!(cursor <= available, "dictate.audio_cursor_invalid");
        let pcm = if cursor < record.saved_samples {
            self.store
                .audio(
                    id,
                    cursor,
                    (record.saved_samples - cursor).min(crate::MAX_BLOCK_SAMPLES),
                )
                .await?
        } else {
            input.pending[((cursor - record.saved_samples) * 2) as usize..].to_vec()
        };
        Ok((record, pcm, available))
    }
    pub(crate) fn set_preview_partial(&self, id: &str, partial: &str) -> Result<()> {
        let input = self.capture_input(id)?;
        *input
            .partial
            .lock()
            .map_err(|_| anyhow::anyhow!("dictate.preview_unavailable"))? = partial.into();
        Ok(())
    }
    pub(crate) fn publish_preview(&self, record: Recording, partial: &str) -> Result<()> {
        self.set_preview_partial(&record.id, partial)?;
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
                let block = AudioBlock {
                    recording_id: id.into(),
                    sequence: record.next_sequence,
                    start_sample: record.saved_samples,
                    pcm: input.pending.clone(),
                };
                self.save_block(block, &mut input).await?;
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
            Change::CancelProcessing
                | Change::ProcessingFailed { .. }
                | Change::UseAvailable { .. }
                | Change::MarkCopied { .. }
                | Change::UseOriginal
                | Change::CleanupFinished { .. }
        );
        let cancels_preparation = matches!(
            change,
            Change::CancelProcessing | Change::ProcessingFailed { .. } | Change::UseAvailable { .. }
                | Change::UseOriginal | Change::MarkCopied { .. }
        );
        let stopping = matches!(change, Change::Stop { .. });
        let record = self.store.change(id, change).await?;
        if cancels_preparation {
            self.cancel_preparation(id);
        }
        if stopping && record.transcription_mode == crate::TranscriptionMode::Segmented {
            let input = self.capture_input(id)?;
            let mut input = input.audio.lock().await;
            if let (Some(mut voice), Some(segment)) =
                (input.voice_pcm.take(), record.segments.last())
            {
                let request_id = segment_request_id(id, segment.index, segment.attempt + 1);
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    input.prepared.entry(request_id.clone())
                {
                    if let Ok(job) = voice.finish_input(request_id).await {
                        entry.insert(job);
                    }
                }
                while input.prepared.len() > 2 {
                    input.prepared.pop_first();
                }
            }
        }
        if forgets_context {
            self.forget_context(id);
        }
        if record.capture != CaptureState::Recording && !record.processing_enabled {
            self.inputs
                .lock()
                .map_err(|_| anyhow::anyhow!("dictate.capture_unavailable"))?
                .remove(id);
        }
        self.publish(&record);
        if wakes {
            if record.completed_at_ms.is_some() || record.pinned {
                self.maintenance_dirty
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            self.wake.notify_one();
        }
        Ok(record)
    }
    fn preview_projection(&self, record: Recording) -> Recording {
        let mut projection = record;
        let input = self
            .inputs
            .lock()
            .ok()
            .and_then(|inputs| inputs.get(&projection.id).cloned());
        if let Some(input) = input {
            if let Ok(partial) = input.partial.lock() {
                let base = projection.receipt().transcript_preview;
                projection.live_preview = Some(if partial.is_empty() {
                    base
                } else if base.is_empty() {
                    partial.clone()
                } else {
                    format!("{base} {partial}")
                });
            };
        }
        projection
    }
    pub async fn preview_snapshot(&self, id: &str) -> Result<Recording> {
        Ok(self.preview_projection(self.store.get(id).await?))
    }
    fn publish(&self, record: &Recording) {
        let _ = self.events.send(crate::DictateEvent::Recording(Box::new(
            self.preview_projection(record.clone()),
        )));
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
            let realtime = self
                .settings
                .as_ref()
                .and_then(|settings| settings().ok())
                .is_some_and(|settings| {
                    settings.transcription_mode == crate::TranscriptionMode::Realtime
                });
            // Optional warmup must yield to new capture/stop/settings work.
            // Cancelling preparation is safe: providers own their partial resources.
            if self.store.pending().await?.is_empty() {
              tokio::select! {
                biased;
                _ = self.wake.notified() => {}
                result = self.voice.prepare_streaming(realtime) => {
                    if let Err(error) = result {
                        tracing::warn!(target:"polar_dictate::latency",event="dictate.prepare_failed",?error,"Realtime transport preparation unavailable; capture remains independent");
                    }
                }
              }
            }
            let now = now_ms();
            let policy = self.retention_policy()?;
            if self
                .maintenance_dirty
                .swap(false, std::sync::atomic::Ordering::AcqRel)
                || deadline.is_some_and(|at| at <= tokio::time::Instant::now())
            {
                match self.store.cleanup(now, policy, true).await {
                    Ok(report) => {
                        if report.deleted > 0 {
                            let _ = self.events.send(crate::DictateEvent::HistoryInvalidated);
                        }
                        deadline = report.next_at_ms.and_then(|at| {
                            tokio::time::Instant::now().checked_add(
                                std::time::Duration::from_millis(at.saturating_sub(now)),
                            )
                        });
                    }
                    Err(error) => {
                        deadline = None;
                        tracing::warn!(
                            ?error,
                            "Dictate cleanup failed; recording remains available"
                        );
                    }
                }
            }
            let pending = self.store.pending().await?;
            if pending.len() >= 200 {
                self.wake.notify_one();
            }
            for record in pending {
                if let Err(error) = self.process(&record.id).await {
                    if self.store.get(&record.id).await.is_ok_and(|r| r.processing_enabled) {
                        let _ = self.change(&record.id, Change::ProcessingFailed {
                            error: format!("Dictate processing failed: {error}; saved audio retained"),
                        }).await;
                    }
                }
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
        let mut events = self.subscribe();
        let result = {
            let processing = self.process_inner(id);
            tokio::pin!(processing);
            loop {
                tokio::select! {
                    result = &mut processing => break result,
                    _ = events.recv() => {
                        if !self.store.get(id).await?.processing_enabled { break Ok(()); }
                    }
                }
            }
        };
        if self
            .store
            .get(id)
            .await
            .is_ok_and(|r| r.capture != CaptureState::Recording)
        {
            if let Ok(mut inputs) = self.inputs.lock() {
                inputs.remove(id);
            }
        }
        if let Some(cleanup) = &self.cleanup {
            if self
                .store
                .get(id)
                .await
                .is_ok_and(|r| r.capture != CaptureState::Recording || !r.processing_enabled)
            {
                cleanup.release_recording(id).await;
            }
        }
        result
    }
    async fn process_inner(&self, id: &str) -> Result<()> {
        let initial = self.store.get(id).await?;
        if initial.transcription_mode == crate::TranscriptionMode::Realtime
            && initial.processing_enabled
            && (initial.capture == CaptureState::Recording
                || initial
                    .segments
                    .iter()
                    .any(|s| s.state == SegmentState::Pending))
        {
            return self.process_realtime(id).await;
        }
        let mut cleanup: Option<crate::cleanup::PrefixCleanup> = None;
        let mut cleanup_events = self.subscribe();
        loop {
            let record = self.store.get(id).await?;
            if !record.processing_enabled {
                return Ok(());
            }
            if !record.cleanup_enabled {
                cleanup = None;
            }
            if cleanup.is_none()
                && (record.capture == CaptureState::Recording
                    || !record.cleaned_prefix.is_empty()
                    || record
                        .segments
                        .iter()
                        .any(|s| s.state == SegmentState::Pending))
            {
                cleanup = self.prefix_cleanup(&record)?;
            }
            let Some(segment) = record
                .segments
                .iter()
                .find(|s| s.state == SegmentState::Pending)
            else {
                if let Some(work) = cleanup.as_mut() {
                    tokio::select! {
                        result=work => { cleanup=None;self.apply_prefix_cleanup(id,result).await?; }
                        _=cleanup_events.recv() => {}
                    }
                    continue;
                }
                if record.capture != CaptureState::Recording
                    && record
                        .segments
                        .iter()
                        .any(|s| s.state == SegmentState::Failed)
                {
                    self.change(id, Change::CancelProcessing).await?;
                    return Ok(());
                }
                let context = self.recording_context(&record)?;
                if record.original().is_some() && !record.cleanup_enabled {
                    self.forget_context(id);
                }
                return self.clean(&record, context).await;
            };
            let index = segment.index;
            let running = self.change(id, Change::SegmentStarted { index }).await?;
            let segment = &running.segments[index as usize];
            let attempt = segment.attempt;
            let mut events = self.subscribe();
            let response = async {
                let request_id = segment_request_id(id, index, attempt);
                let input = self.capture_input(id)?;
                let prepared = input.audio.lock().await.prepared.remove(&request_id);
                let job = if let Some(job) = prepared {
                    job
                } else {
                    // Restart/retry reconstructs disposable packets from the
                    // same durable range through the same Voice PCM facade.
                    let pcm = self
                        .store
                        .audio(id, segment.start_sample, segment.sample_count)
                        .await?;
                    let mut voice = self
                        .voice
                        .open_pcm_with_reference(
                            &request_id,
                            polar_voice::pcm::PcmTranscriptionMode::Buffered,
                            segment.start_sample,
                            self.transcription_reference(&running, polar_voice::pcm::PcmTranscriptionMode::Buffered)?,
                        )
                        .await?;
                    voice.append_pcm(&pcm).await?;
                    voice.finish_input(request_id.clone()).await?
                };
                Ok::<_,anyhow::Error>(job.wait()
                    .instrument(tracing::info_span!(target:"polar_dictate::latency","dictate.segment.request",operation_id=id,ordinal=index,attempt))
                    .await?.text)
            };
            tokio::pin!(response);
            let response = loop {
                tokio::select! {
                    result=&mut response => break result,
                    result=async {match cleanup.as_mut() {Some(work)=>work.await,None=>std::future::pending().await}} => {
                        cleanup=None;
                        self.apply_prefix_cleanup(id,result).await?;
                    }
                    event=events.recv() => {
                        let current=match event {Ok(crate::DictateEvent::Recording(record)) if record.id==id=>*record,Ok(_)=>continue,Err(_)=>self.store.get(id).await?};
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
            let pause_processing = response.as_ref().err().is_some_and(|error| {
                error
                    .downcast_ref::<polar_voice::VoiceError>()
                    .is_none_or(|error| error.blocks_following_requests())
            });
            let (text, error) = match response {
                Ok(value) => (Some(value), None),
                Err(error) => (
                    None,
                    Some(
                        error
                            .downcast_ref::<polar_voice::VoiceError>()
                            .map_or_else(|| error.to_string(), |error| error.recording_message()),
                    ),
                ),
            };
            let record = self
                .change(
                    id,
                    Change::SegmentFinished {
                        pause_processing,
                        index,
                        attempt,
                        text,
                        error,
                    },
                )
                .await?;
            if record.processing_enabled {
                let parts = record
                    .segments
                    .iter()
                    .take_while(|s| s.state == SegmentState::Complete)
                    .filter_map(|s| s.original.clone())
                    .collect();
                self.change(id, Change::ConfirmTranscript { parts }).await?;
            }
        }
    }
    pub(crate) fn cancel_preparation(&self, id: &str) {
        if let Ok(inputs) = self.inputs.lock() {
            if let Some(input) = inputs.get(id) {
                if let Ok(mut task) = input.preparation.lock() {
                    if let Some(task) = task.take() {
                        task.abort();
                    }
                }
            }
        }
    }
    pub(crate) async fn finish_preparation(&self, id: &str) {
        let task = self
            .inputs
            .lock()
            .ok()
            .and_then(|inputs| inputs.get(id).cloned())
            .and_then(|input| {
                input
                    .preparation
                    .lock()
                    .ok()
                    .and_then(|mut task| task.take())
            });
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
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
        let separator = match record.transcription_mode {
            crate::TranscriptionMode::Segmented => "\n",
            crate::TranscriptionMode::Realtime => " ",
        };
        if record.cleaned_prefix_parts == record.confirmed_transcript.len()
            && !record.cleaned_prefix.is_empty()
            && record.confirmed_transcript.join(separator).trim() == original
        {
            let running = self.change(&record.id, Change::CleanupStarted).await?;
            self.change(
                &record.id,
                Change::CleanupFinished {
                    attempt: running.cleanup_attempt,
                    text: Some(record.cleaned_prefix.clone()),
                    error: None,
                },
            )
            .await?;
            return Ok(());
        }
        let mut events = self.subscribe();
        let running = match self.change(&record.id, Change::CleanupStarted).await {
            Ok(running) => running,
            Err(error) => {
                // Use Original/cancellation may win after the transcript snapshot.
                // Do not turn that successful user action into a worker failure.
                let current = self.store.get(&record.id).await?;
                if !current.cleanup_enabled
                    || !current.processing_enabled
                    || current.delivery != crate::DeliveryState::Pending
                {
                    return Ok(());
                }
                return Err(error);
            }
        };
        let operation_id = format!(
            "{:x}",
            Sha256::digest(format!("{}:cleanup:{}", record.id, running.cleanup_attempt))
        );
        let request = polar_transcreation::EditRequest {
            project: record.project.clone(),
            scope: None,
            operation_id,
            base_revision: running.revision,
            original,
            edit: record
                .output_mode()
                .edit(if record.context_enabled {
                    context.filter(|text| {
                        record.context_digest.as_deref()
                            == Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
                    })
                } else {
                    None
                })
                .ok_or_else(|| anyhow::anyhow!("dictate.refinement_disabled"))?,
        };
        let mut request = request;
        let (polar_transcreation::EditKind::CleanupTranscript { context }
        | polar_transcreation::EditKind::CleanupTranslateEnglish { context }) = &mut request.edit
        else {
            anyhow::bail!("dictate.invalid_cleanup_kind");
        };
        *context = record.cleanup_reference_context(context.take())?;
        self.finish_preparation(&record.id).await;
        let response = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            self.cleanup_provider()?
                .clean_recording(&record.id, request)
                .await
        });
        tokio::pin!(response);
        let response = loop {
            tokio::select! {
                value = &mut response => break value,
                event = events.recv() => {
                    let current = match event {
                        Ok(crate::DictateEvent::Recording(current)) if current.id == record.id => *current,
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

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn segment_request_id(id: &str, index: u32, attempt: u32) -> String {
    format!("{:x}", Sha256::digest(format!("{id}:{index}:{attempt}")))
}
