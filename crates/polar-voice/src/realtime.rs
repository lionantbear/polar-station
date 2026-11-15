//! Provider-neutral realtime audio. Conversation ownership and tool execution
//! remain with callers; this interface never creates a message or thread.

use crate::{VoiceError, VoiceOpusPacket};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const REALTIME_PENDING_AUDIO_PACKETS: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceRealtimeStart {
    pub operation_id: String,
    /// Caller-authorized instructions/context, never an arbitrary device tool grant.
    pub instructions: String,
}

impl VoiceRealtimeStart {
    pub fn validate(&self) -> Result<(), VoiceError> {
        crate::validate_operation_id(&self.operation_id)?;
        if self.instructions.is_empty() || self.instructions.len() > 32 * 1024 {
            return Err(VoiceError::InvalidOperation);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceRealtimeEvent {
    Transcript {
        role: String,
        text: String,
        complete: bool,
    },
    Handoff {
        handoff_id: String,
        text: String,
    },
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceRealtimeStats {
    pub input_dropped: u64,
    pub output_dropped: u64,
    pub input_pending: usize,
    pub output_pending: usize,
}

#[async_trait]
pub trait VoiceRealtimeProvider: Send + Sync {
    async fn start(
        &self,
        request: VoiceRealtimeStart,
    ) -> Result<Arc<dyn VoiceRealtimeSession>, VoiceError>;
}

#[async_trait]
pub trait VoiceRealtimeSession: Send + Sync {
    /// Monotonic sequence per call, one canonical 20 ms Opus packet.
    async fn append(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError>;
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceError>;
    /// Has an independent reader from output audio: control cannot wait behind audio.
    async fn next_event(&self) -> Result<VoiceRealtimeEvent, VoiceError>;
    async fn next_audio(&self) -> Result<VoiceOpusPacket, VoiceError>;
    async fn handoff_output(&self, handoff_id: &str, text: &str) -> Result<(), VoiceError>;
    async fn stats(&self) -> VoiceRealtimeStats;
    async fn close(&self) -> Result<(), VoiceError>;
}


