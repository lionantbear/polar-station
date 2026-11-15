//! Bounded Opus playout; no authentication, UI, or transcript policy.
use polar_voice::{VoiceError, VoiceOpusPacket};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const JITTER_TARGET_PACKETS: usize = 3;
const JITTER_HARD_LIMIT_PACKETS: usize = 5;
const JITTER_RESERVE_PACKETS: u32 = 2;
const MAXIMUM_CONSECUTIVE_MISSING_PACKETS: u32 = 5;
const MAXIMUM_CONSECUTIVE_STALL_PACKETS: u32 = 50;
// Matched live boundary tests truncate the final word at 25 packets;
// 50 packets retain it before the protocol session-close request.
const PROVIDER_TAIL_SILENCE_PACKETS: u32 = 50;

/// A canonical 20 ms Opus silence frame at 48 kHz. It is forwarded without
/// decode or re-encode for both loss concealment and the provider-only tail.
pub const OPUS_SILENCE_PACKET: &[u8] = &[0xf8, 0xff, 0xfe];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JitterPlayout {
    Wait,
    Packet(Vec<u8>),
    ReserveSilence,
    StallSilence,
    Silence,
    TailSilence,
    Complete,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderJitterStats {
    pub maximum_buffered_packets: usize,
    pub late_packets: u32,
    pub duplicate_packets: u32,
    pub jitter_reserve_packets: u32,
    pub stall_silence_packets: u32,
    pub concealment_packets: u32,
    pub maximum_consecutive_stalled: u32,
    pub maximum_consecutive_missing: u32,
}

#[derive(Debug, Default)]
pub(super) struct ProviderIngressTiming {
    previous_packet_at: Option<Instant>,
    maximum_gap: Duration,
}

impl ProviderIngressTiming {
    pub(super) fn observe(&mut self, packet_at: Instant) -> Option<Duration> {
        let gap = self
            .previous_packet_at
            .map(|previous_packet_at| packet_at.saturating_duration_since(previous_packet_at));
        self.previous_packet_at = Some(packet_at);
        if let Some(gap) = gap {
            self.maximum_gap = self.maximum_gap.max(gap);
        }
        gap
    }

    pub(super) fn maximum_gap(&self) -> Duration {
        self.maximum_gap
    }
}

/// Recording-scoped, packet-preserving jitter buffer. Timing and WebRTC
/// playout are provider concerns; the Voice domain owns sequence validity and
/// the declared recording manifest.
#[derive(Debug)]
pub struct ProviderJitterBuffer {
    packets: BTreeMap<u32, Vec<u8>>,
    pub(super) next_sequence: u32,
    started: bool,
    pub(super) committed_packet_count: Option<u32>,
    jitter_reserve_remaining: u32,
    consecutive_stalled: u32,
    consecutive_missing: u32,
    tail_silence_remaining: u32,
    complete: bool,
    stats: ProviderJitterStats,
}

impl Default for ProviderJitterBuffer {
    fn default() -> Self {
        Self {
            packets: BTreeMap::new(),
            next_sequence: 0,
            started: false,
            committed_packet_count: None,
            jitter_reserve_remaining: JITTER_RESERVE_PACKETS,
            consecutive_stalled: 0,
            consecutive_missing: 0,
            tail_silence_remaining: 0,
            complete: false,
            stats: ProviderJitterStats::default(),
        }
    }
}

impl ProviderJitterBuffer {
    pub fn can_accept_packet(&self) -> bool {
        self.packets.len() < JITTER_HARD_LIMIT_PACKETS
    }

    pub fn push(&mut self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        if packet.sequence < self.next_sequence {
            self.stats.late_packets = self.stats.late_packets.saturating_add(1);
            return Ok(());
        }
        if self.packets.contains_key(&packet.sequence) {
            self.stats.duplicate_packets = self.stats.duplicate_packets.saturating_add(1);
            return Ok(());
        }
        if self.packets.len() >= JITTER_HARD_LIMIT_PACKETS {
            return Err(VoiceError::MediaJitterExceeded);
        }
        self.packets.insert(packet.sequence, packet.payload);
        self.stats.maximum_buffered_packets =
            self.stats.maximum_buffered_packets.max(self.packets.len());
        Ok(())
    }

    pub fn commit(&mut self, packet_count: u32) -> Result<(), VoiceError> {
        if self
            .committed_packet_count
            .is_some_and(|existing| existing != packet_count)
            || self
                .packets
                .keys()
                .any(|sequence| *sequence >= packet_count)
            || self.next_sequence > packet_count
        {
            return Err(VoiceError::InvalidStream);
        }
        self.committed_packet_count = Some(packet_count);
        self.tail_silence_remaining = PROVIDER_TAIL_SILENCE_PACKETS;
        Ok(())
    }

    pub fn tick(&mut self) -> Result<JitterPlayout, VoiceError> {
        if self.complete {
            return Ok(JitterPlayout::Complete);
        }
        if !self.started {
            if self.packets.len() < JITTER_TARGET_PACKETS && self.committed_packet_count.is_none() {
                return Ok(JitterPlayout::Wait);
            }
            self.started = true;
        }
        if self
            .committed_packet_count
            .is_some_and(|packet_count| self.next_sequence >= packet_count)
        {
            if self.tail_silence_remaining > 0 {
                self.tail_silence_remaining -= 1;
                return Ok(JitterPlayout::TailSilence);
            }
            self.complete = true;
            return Ok(JitterPlayout::Complete);
        }
        if let Some(payload) = self.packets.remove(&self.next_sequence) {
            self.next_sequence += 1;
            self.consecutive_stalled = 0;
            self.consecutive_missing = 0;
            self.jitter_reserve_remaining = JITTER_RESERVE_PACKETS;
            return Ok(JitterPlayout::Packet(payload));
        }

        if self.jitter_reserve_remaining > 0 {
            self.jitter_reserve_remaining -= 1;
            self.stats.jitter_reserve_packets = self.stats.jitter_reserve_packets.saturating_add(1);
            return Ok(JitterPlayout::ReserveSilence);
        }

        let confirmed_gap = self
            .packets
            .first_key_value()
            .is_some_and(|(sequence, _)| *sequence > self.next_sequence)
            || self.committed_packet_count.is_some();
        if confirmed_gap {
            self.next_sequence += 1;
            self.consecutive_stalled = 0;
            self.consecutive_missing = self.consecutive_missing.saturating_add(1);
            self.stats.concealment_packets = self.stats.concealment_packets.saturating_add(1);
            self.stats.maximum_consecutive_missing = self
                .stats
                .maximum_consecutive_missing
                .max(self.consecutive_missing);
            if self.consecutive_missing > MAXIMUM_CONSECUTIVE_MISSING_PACKETS {
                return Err(VoiceError::MediaJitterExceeded);
            }
            return Ok(JitterPlayout::Silence);
        }

        self.consecutive_stalled = self.consecutive_stalled.saturating_add(1);
        self.stats.stall_silence_packets = self.stats.stall_silence_packets.saturating_add(1);
        self.stats.maximum_consecutive_stalled = self
            .stats
            .maximum_consecutive_stalled
            .max(self.consecutive_stalled);
        if self.consecutive_stalled >= MAXIMUM_CONSECUTIVE_STALL_PACKETS {
            return Err(VoiceError::MediaJitterExceeded);
        }
        Ok(JitterPlayout::StallSilence)
    }

    pub fn stats(&self) -> ProviderJitterStats {
        self.stats
    }
}
