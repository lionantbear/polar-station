//! Recording orchestration over the existing VoiceRuntime streaming path.
use crate::{CaptureState, Change, SegmentState};
use anyhow::Result;
use polar_voice::{pcm::PcmTranscriptionMode, VoiceStreamAbortRequest};

impl crate::runtime::DictateRuntime {
    pub(crate) async fn process_realtime(&self, id: &str) -> Result<()> {
        let mut events = self.subscribe();
        let initial = self.store().get(id).await?;
        let operation = format!("dictate-stream:{id}:{}", initial.revision);
        let result = self.transcribe_and_clean(id, &operation, &mut events).await;
        // A timeout/cancelled future must not leave media running in the provider.
        let _ = self
            .voice()
            .stream_abort(VoiceStreamAbortRequest::new(&operation))
            .await;
        let current = self.store().get(id).await?;
        if !current.processing_enabled
            || current.transcription_mode != crate::TranscriptionMode::Realtime
        {
            return Ok(());
        }
        if current.capture == CaptureState::Recording {
            // Preserve saved audio and expose the failure through the existing
            // stop/error path; capture cannot silently keep a dead stream alive.
            self.change(
                id,
                Change::Stop {
                    reason: Some("Realtime transcription interrupted; saved audio retained".into()),
                },
            )
            .await?;
        }
        let current = self.store().get(id).await?;
        let record = if let Some(segment) = current
            .segments
            .iter()
            .find(|s| s.state == SegmentState::Pending)
        {
            let index = segment.index;
            let running = self.change(id, Change::SegmentStarted { index }).await?;
            let (text, error) = match result {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(error.to_string())),
            };
            self.change(
                id,
                Change::SegmentFinished {
                    pause_processing: true,
                    index,
                    attempt: running.segments[index as usize].attempt,
                    text,
                    error,
                },
            )
            .await?
        } else {
            current
        };
        if record.cleanup_enabled
            && record.cleanup_error.is_none()
            && !record.cleaned_prefix.is_empty()
            && record.cleaned_prefix_parts == record.confirmed_transcript.len()
            && record.original().as_deref() == Some(record.confirmed_transcript.join(" ").trim())
        {
            let running = self.change(id, Change::CleanupStarted).await?;
            self.change(
                id,
                Change::CleanupFinished {
                    attempt: running.cleanup_attempt,
                    text: Some(record.cleaned_prefix),
                    error: None,
                },
            )
            .await?;
            Ok(())
        } else {
            self.clean(&record, self.recording_context(&record)?).await
        }
    }

    async fn transcribe_and_clean(
        &self,
        id: &str,
        operation: &str,
        events: &mut tokio::sync::broadcast::Receiver<crate::DictateEvent>,
    ) -> Result<String> {
        let record = self.store().get(id).await?;
        let reference = self.transcription_reference(&record, PcmTranscriptionMode::Incremental)?;
        let mut voice = self
            .voice()
            .open_pcm_with_reference(operation, PcmTranscriptionMode::Incremental, 0, reference)
            .await?;
        let mut updates = voice
            .subscribe()
            .await?
            .ok_or_else(|| anyhow::anyhow!("voice.transcript_events_unavailable"))?;
        let transfer = self.transcribe_recording(id, operation, &mut voice, events);
        tokio::pin!(transfer);
        let mut cleanup: Option<crate::cleanup::PrefixCleanup> = None;
        let mut completed: Option<String> = None;
        let mut observing = true;
        let mut cancellation = self.subscribe();
        loop {
            let record = self.store().get(id).await?;
            anyhow::ensure!(
                record.processing_enabled
                    && record.transcription_mode == crate::TranscriptionMode::Realtime,
                "dictate.processing_cancelled"
            );
            if !record.cleanup_enabled {
                cleanup = None;
            }
            if cleanup.is_none() {
                cleanup = self.prefix_cleanup(&record)?;
            }
            if cleanup.is_none() {
                if let Some(text) = completed.take() {
                    return Ok(text);
                }
            }
            let cleanup_wait = async {
                match cleanup.as_mut() {
                    Some(work) => work.await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                result = &mut transfer, if completed.is_none() => {
                    let text = result?;
                    let snapshot = updates.borrow().clone();
                    self.set_preview_partial(id,&snapshot.partial)?;
                    let parts = if snapshot.confirmed_text().trim() == text.trim() {
                        snapshot.confirmed
                    } else { vec![text.clone()] };
                    self.change(id, Change::ConfirmTranscript { parts }).await?;
                    // Publish complete original before awaiting cleanup, keeping
                    // the existing Use Original action available during rewriting.
                    let current = self.store().get(id).await?;
                    if let Some(segment) = current.segments.iter().find(|s| s.state == SegmentState::Pending) {
                        let index = segment.index;
                        let running = self.change(id, Change::SegmentStarted { index }).await?;
                        self.change(id, Change::SegmentFinished { pause_processing: true, index, attempt: running.segments[index as usize].attempt,
                            text: Some(text.clone()), error: None }).await?;
                    }
                    completed = Some(text);
                    observing = false;
                }
                changed = updates.changed(), if observing => {
                    if changed.is_err() { observing = false; continue; }
                    let snapshot = updates.borrow_and_update().clone();
                    self.set_preview_partial(id,&snapshot.partial)?;
                    let parts = snapshot.confirmed;
                    if parts != record.confirmed_transcript {
                        self.change(id, Change::ConfirmTranscript { parts }).await?;
                    }
                    self.publish_preview(self.store().get(id).await?, &snapshot.partial)?;
                }
                result = cleanup_wait => {
                    cleanup = None;
                    self.apply_prefix_cleanup(id,result).await?;
                }
                _ = cancellation.recv() => {}
            }
        }
    }

    async fn transcribe_recording(
        &self,
        id: &str,
        operation: &str,
        voice: &mut polar_voice::pcm::PcmTranscriptionSession,
        events: &mut tokio::sync::broadcast::Receiver<crate::DictateEvent>,
    ) -> Result<String> {
        let mut cursor = 0;
        loop {
            let (record, pcm, available) = self.capture_audio(id, cursor).await?;
            anyhow::ensure!(
                record.processing_enabled
                    && record.transcription_mode == crate::TranscriptionMode::Realtime,
                "dictate.processing_cancelled"
            );
            let count = pcm.len() as u64 / 2;
            let finished = record.capture != CaptureState::Recording && cursor + count == available;
            anyhow::ensure!(!finished || available != 0, "dictate.empty_recording");
            if count > 0 || finished {
                if !pcm.is_empty() {
                    voice.append_pcm(&pcm).await?;
                }
                cursor += count;
            }
            if finished {
                let job = voice.finish_input(operation).await?;
                let response = job.wait();
                tokio::pin!(response);
                loop {
                    tokio::select! {
                        result = &mut response => return Ok(result?.text),
                        event = events.recv() => {
                            if let Ok(crate::DictateEvent::Recording(record)) = event {
                                if record.id == id && !record.processing_enabled { anyhow::bail!("dictate.processing_cancelled"); }
                            } else if !self.store().get(id).await?.processing_enabled { anyhow::bail!("dictate.processing_cancelled"); }
                        }
                    }
                }
            }
            if cursor < available {
                continue;
            }
            // Subscription predates the snapshot, so append/stop cannot be missed.
            let _ = events.recv().await;
        }
    }
}
