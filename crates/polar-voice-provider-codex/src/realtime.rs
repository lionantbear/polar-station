//! Real duplex calls; derives upstream session/handoff handling from dictate-lab.
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};

use codex_api::{RealtimeContextAppendChannel, RealtimeSessionConfig};
use codex_protocol::protocol::RealtimeEvent;
use polar_voice::{
    realtime::{
        VoiceRealtimeEvent, VoiceRealtimeProvider, VoiceRealtimeSession, VoiceRealtimeStart,
        VoiceRealtimeStats, REALTIME_PENDING_AUDIO_PACKETS,
    },
    VoiceError, VoiceOpusPacket, MAXIMUM_OPUS_PACKET_BYTES,
};
use ropus::OpusRepacketizer;
use tokio::sync::{watch, Mutex, Notify};

use crate::{CodexVoiceProvider, ProviderResources, OPUS_FRAME_DURATION, OPUS_SILENCE_PACKET};

#[derive(Default)]
pub(super) struct AudioQueue {
    packets: StdMutex<VecDeque<VoiceOpusPacket>>,
    available: Notify,
    closed: AtomicBool,
    dropped: AtomicU64,
}

impl AudioQueue {
    fn push(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        let mut queue = self.packets.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return Err(VoiceError::StreamNotFound);
        }
        if queue.len() == REALTIME_PENDING_AUDIO_PACKETS {
            queue.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queue.push_back(packet);
        self.available.notify_one();
        Ok(())
    }
    fn pop(&self) -> Option<VoiceOpusPacket> {
        self.packets.lock().unwrap().pop_front()
    }
    fn clear(&self) {
        self.packets.lock().unwrap().clear();
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.clear();
        self.available.notify_one();
    }
    async fn next(&self) -> Result<VoiceOpusPacket, VoiceError> {
        loop {
            let available = self.available.notified();
            if self.closed.load(Ordering::Acquire) {
                return Err(VoiceError::StreamNotFound);
            }
            if let Some(packet) = self.pop() {
                return Ok(packet);
            }
            available.await;
        }
    }
}

pub(super) fn session_config(
    request: &VoiceRealtimeStart,
    correlation: &str,
) -> RealtimeSessionConfig {
    let mut config = crate::transcription_session(correlation);
    config.model = Some("gpt-live-1-codex".into());
    config.instructions.clone_from(&request.instructions);
    config
}

pub(super) fn attach_output(
    peer: &Arc<webrtc::peer_connection::RTCPeerConnection>,
    output: Arc<AudioQueue>,
) {
    peer.on_track(Box::new(move |track, _, _| {
        let output = Arc::clone(&output);
        Box::pin(async move {
            tokio::spawn(async move {
                let result = async {
                    if !track
                        .codec()
                        .capability
                        .mime_type
                        .eq_ignore_ascii_case("audio/opus")
                    {
                        return Err(VoiceError::InvalidAudio);
                    }
                    let mut sequence = 0u32;
                    while let Ok((packet, _)) = track.read_rtp().await {
                        forward_output(&packet.payload, &output, &mut sequence)?;
                    }
                    Ok::<_, VoiceError>(())
                }
                .await;
                let _ = result;
                output.close();
            });
        })
    }));
}

// RTP's 48 kHz clock does not require a 48 kHz PCM conversion. Keep the
// encoded frames intact and let the device decoder select its output rate.
fn forward_output(
    payload: &[u8],
    output: &AudioQueue,
    sequence: &mut u32,
) -> Result<(), VoiceError> {
    let mut repacketizer = OpusRepacketizer::new();
    if payload.len() > MAXIMUM_OPUS_PACKET_BYTES * 6
        || repacketizer.cat(payload, payload.len() as i32) != 0
    {
        return Err(VoiceError::InvalidAudio);
    }
    let samples = ropus::opus::decoder::opus_packet_get_nb_samples(payload, 48_000)
        .map_err(|_| VoiceError::InvalidAudio)? as usize;
    let frames = repacketizer.get_nb_frames() as usize;
    let frame_samples = samples / frames;
    if samples < 960 || !samples.is_multiple_of(960) || !960usize.is_multiple_of(frame_samples) {
        return Err(VoiceError::InvalidAudio);
    }
    let per_packet = 960 / frame_samples;
    for begin in (0..frames).step_by(per_packet) {
        let next = sequence.checked_add(1).ok_or(VoiceError::InvalidStream)?;
        let packet = if samples == 960 && payload.len() <= MAXIMUM_OPUS_PACKET_BYTES {
            payload.to_vec()
        } else {
            let mut bytes = vec![0; MAXIMUM_OPUS_PACKET_BYTES];
            let length = repacketizer.out_range(
                begin,
                begin + per_packet,
                &mut bytes,
                MAXIMUM_OPUS_PACKET_BYTES as i32,
            );
            if length <= 0 {
                return Err(VoiceError::InvalidAudio);
            }
            bytes.truncate(length as usize);
            bytes
        };
        output.push(VoiceOpusPacket::new(*sequence, packet))?;
        *sequence = next;
    }
    Ok(())
}

struct CodexRealtimeSession {
    resources: Arc<ProviderResources>,
    input: Arc<AudioQueue>,
    output: Arc<AudioQueue>,
    muted: Arc<AtomicBool>,
    stop: watch::Sender<bool>,
    sender_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    next_sequence: Mutex<u32>,
    pending_handoffs: Mutex<HashSet<String>>,
    closed: AtomicBool,
}

#[async_trait::async_trait]
impl VoiceRealtimeProvider for CodexVoiceProvider {
    async fn start(
        &self,
        request: VoiceRealtimeStart,
    ) -> Result<Arc<dyn VoiceRealtimeSession>, VoiceError> {
        request.validate()?;
        let output = Arc::new(AudioQueue::default());
        let resources = Arc::new(
            tokio::time::timeout(
                Duration::from_secs(30),
                self.connect_media_session_for(Some(&request), Some(Arc::clone(&output))),
            )
            .await
            .map_err(|_| VoiceError::Timeout)??,
        );
        let input = Arc::new(AudioQueue::default());
        let muted = Arc::new(AtomicBool::new(false));
        let (stop, mut stopped) = watch::channel(false);
        let media = Arc::clone(&resources);
        let upstream = Arc::clone(&input);
        let downstream = Arc::clone(&output);
        let is_muted = Arc::clone(&muted);
        let task = tokio::spawn(async move {
            let mut clock = tokio::time::interval(OPUS_FRAME_DURATION);
            clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    _ = clock.tick() => {
                        let packet = upstream.pop();
                        let payload = if is_muted.load(Ordering::Acquire) { OPUS_SILENCE_PACKET }
                            else { packet.as_ref().map_or(OPUS_SILENCE_PACKET, |packet| packet.payload.as_slice()) };
                        let write = media.peer().write_opus_packet(payload);
                        tokio::select! {
                            biased;
                            _ = stopped.changed() => break,
                            result = tokio::time::timeout(Duration::from_millis(100), write) => {
                                if !matches!(result, Ok(Ok(()))) { break; }
                            }
                        }
                    }
                }
            }
            upstream.close();
            downstream.close();
            let _ = tokio::time::timeout(Duration::from_secs(2), media.sideband().close()).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), media.peer().close()).await;
        });
        Ok(Arc::new(CodexRealtimeSession {
            resources,
            input,
            output,
            muted,
            stop,
            sender_task: Mutex::new(Some(task)),
            next_sequence: Mutex::new(0),
            pending_handoffs: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
        }))
    }
}

#[async_trait::async_trait]
impl VoiceRealtimeSession for CodexRealtimeSession {
    async fn append(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(VoiceError::StreamNotFound);
        }
        if packet.payload.is_empty()
            || packet.payload.len() > MAXIMUM_OPUS_PACKET_BYTES
            || ropus::opus::decoder::opus_packet_get_nb_samples(&packet.payload, 48_000) != Ok(960)
        {
            return Err(VoiceError::InvalidAudio);
        }
        let mut next = self.next_sequence.lock().await;
        if packet.sequence < *next {
            return Err(VoiceError::InvalidStream);
        }
        *next = packet
            .sequence
            .checked_add(1)
            .ok_or(VoiceError::InvalidStream)?;
        if self.muted.load(Ordering::Acquire) {
            return Ok(());
        }
        self.input.push(packet)
    }
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(VoiceError::StreamNotFound);
        }
        self.muted.store(muted, Ordering::Release);
        self.input.clear();
        Ok(())
    }
    async fn next_audio(&self) -> Result<VoiceOpusPacket, VoiceError> {
        self.output.next().await
    }
    async fn next_event(&self) -> Result<VoiceRealtimeEvent, VoiceError> {
        loop {
            let event = self
                .resources
                .sideband()
                .next_event()
                .await
                .map_err(|_| VoiceError::ProviderUnavailable)?;
            let (role, text, complete) = match event {
                Some(RealtimeEvent::InputTranscriptDelta(value)) => ("user", value.delta, false),
                Some(RealtimeEvent::InputTranscriptDone(value)) => ("user", value.text, true),
                Some(RealtimeEvent::OutputTranscriptDelta(value)) => {
                    ("assistant", value.delta, false)
                }
                Some(RealtimeEvent::OutputTranscriptDone(value)) => ("assistant", value.text, true),
                Some(RealtimeEvent::HandoffRequested(value)) => {
                    let mut pending = self.pending_handoffs.lock().await;
                    if pending.len() >= 64 {
                        return Err(VoiceError::OperationConflict);
                    }
                    if !pending.insert(value.handoff_id.clone()) {
                        continue;
                    }
                    return Ok(VoiceRealtimeEvent::Handoff {
                        handoff_id: value.handoff_id,
                        text: value.input_transcript,
                    });
                }
                Some(RealtimeEvent::Error(_)) => return Err(VoiceError::ProviderUnavailable),
                None => return Ok(VoiceRealtimeEvent::Closed),
                _ => continue,
            };
            if text.len() > 32 * 1024 {
                return Err(VoiceError::TranscriptionInvalid);
            }
            return Ok(VoiceRealtimeEvent::Transcript {
                role: role.into(),
                text,
                complete,
            });
        }
    }
    async fn handoff_output(&self, handoff_id: &str, text: &str) -> Result<(), VoiceError> {
        if text.is_empty() || text.len() > 32 * 1024 {
            return Err(VoiceError::InvalidOperation);
        }
        if !self.pending_handoffs.lock().await.remove(handoff_id) {
            return Err(VoiceError::InvalidOperation);
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            self.resources
                .sideband()
                .writer()
                .with_context_append_channel(RealtimeContextAppendChannel::Speakable)
                .send_conversation_function_call_output(handoff_id.to_owned(), text.to_owned()),
        )
        .await
        .map_err(|_| VoiceError::Timeout)?
        .map_err(|_| VoiceError::ProviderUnavailable)
    }
    async fn stats(&self) -> VoiceRealtimeStats {
        VoiceRealtimeStats {
            input_dropped: self.input.dropped.load(Ordering::Relaxed),
            output_dropped: self.output.dropped.load(Ordering::Relaxed),
            input_pending: self.input.packets.lock().unwrap().len(),
            output_pending: self.output.packets.lock().unwrap().len(),
        }
    }
    async fn close(&self) -> Result<(), VoiceError> {
        self.closed.store(true, Ordering::Release);
        self.input.close();
        self.output.close();
        self.stop.send_replace(true);
        if let Some(task) = self.sender_task.lock().await.take() {
            task.await.map_err(|_| VoiceError::ProviderUnavailable)?;
        }
        Ok(())
    }
}

impl Drop for CodexRealtimeSession {
    fn drop(&mut self) {
        self.input.close();
        self.output.close();
        self.stop.send_replace(true);
    }
}


