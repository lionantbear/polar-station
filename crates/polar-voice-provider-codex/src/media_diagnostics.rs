//! Numeric transport evidence only; never log SDP, addresses or audio/text.
use std::time::Instant;
use webrtc::{peer_connection::RTCPeerConnection, stats::StatsReportType};

#[derive(Default)]
pub(super) struct MediaCounters {
    pub outbound_packets: u64,
    pub outbound_bytes: u64,
    remote_stat_entries: usize,
    remote_rtt_measurements: u64,
    remote_packets: u64,
    remote_lost: i64,
    nominated_pairs: usize,
    ice_rtt_ms: Option<f64>,
}

impl MediaCounters {
    pub async fn read(peer: &RTCPeerConnection) -> Self {
        let mut result = Self::default();
        for stats in peer.get_stats().await.reports.values() {
            match stats {
                StatsReportType::OutboundRTP(s) if s.kind == "audio" => {
                    result.outbound_packets += s.packets_sent;
                    result.outbound_bytes += s.bytes_sent;
                }
                StatsReportType::RemoteInboundRTP(s) if s.kind == "audio" => {
                    // A stats entry exists even before any receiver report.
                    result.remote_stat_entries += 1;
                    result.remote_rtt_measurements += s.round_trip_time_measurements;
                    result.remote_packets += s.packets_received;
                    result.remote_lost += s.packets_lost;
                }
                StatsReportType::CandidatePair(s) if s.nominated => {
                    result.nominated_pairs += 1;
                    if s.current_round_trip_time > 0.0 {
                        result.ice_rtt_ms = Some(s.current_round_trip_time * 1000.0);
                    }
                }
                _ => {}
            }
        }
        result
    }
}

pub(super) fn enabled() -> bool {
    std::env::var_os("POLAR_VOICE_STAGE_DIAGNOSTICS").is_some()
}

pub(super) fn connection_stage(stage: &'static str, started: &mut Instant) {
    if enabled() {
        let now = Instant::now();
        tracing::info!(target: "polar_dictate::latency", event="voice.connection_stage",
            stage, elapsed_ms=now.duration_since(*started).as_millis());
        *started = now;
    }
}

pub(super) async fn trace(peer: &RTCPeerConnection, stage: &'static str) {
    if !enabled() { return; }
    // Diagnostics cannot hold finalization indefinitely or create a polling loop.
    let Ok(s) = tokio::time::timeout(std::time::Duration::from_secs(1), MediaCounters::read(peer)).await else {
        tracing::info!(target: "polar_dictate::latency", event="voice.media_stats_unavailable", stage);
        return;
    };
    tracing::info!(target: "polar_dictate::latency", event="voice.media_stats", stage,
        peer_state=?peer.connection_state(), outbound_packets=s.outbound_packets,
        outbound_bytes=s.outbound_bytes, remote_stat_entries=s.remote_stat_entries,
        remote_rtt_measurements=s.remote_rtt_measurements,
        remote_packets=s.remote_packets, remote_lost=s.remote_lost,
        nominated_pairs=s.nominated_pairs, ice_rtt_ms=s.ice_rtt_ms);
}
