use serde::{Deserialize, Serialize};

use crate::{HashBytes, NodeId, StreamId, WireError, WireResult};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PacketSeqRange {
    pub from_seq: u64,
    pub to_seq: u64,
}

impl PacketSeqRange {
    pub fn new(from_seq: u64, to_seq: u64) -> Self {
        Self { from_seq, to_seq }
    }

    pub fn contains(&self, seq: u64) -> bool {
        self.from_seq <= seq && seq <= self.to_seq
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketStreamWindowReport {
    pub lane: DeliveryLaneId,
    pub covered: PacketSeqRange,
    pub resend_ranges: Vec<PacketSeqRange>,
    pub source: WindowReportSource,
    pub created_at_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowReportSource {
    pub stream_id: StreamId,
    pub packet_hash: HashBytes,
    pub reason: WindowReportReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowReportReason {
    PacketReceived,
    PeerOnlineObserved,
    Probe,
    ProbeResponse,
}

impl WindowReportReason {
    pub fn reason_code(self) -> u8 {
        match self {
            Self::PacketReceived => 1,
            Self::PeerOnlineObserved => 2,
            Self::Probe => 3,
            Self::ProbeResponse => 4,
        }
    }

    pub fn from_reason_code(reason_code: u8) -> WireResult<Self> {
        match reason_code {
            1 => Ok(Self::PacketReceived),
            2 => Ok(Self::PeerOnlineObserved),
            3 => Ok(Self::Probe),
            4 => Ok(Self::ProbeResponse),
            _ => Err(WireError::Codec(format!(
                "unknown packet window report reason_code {reason_code}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeliveryLaneId {
    pub sender_node_id: NodeId,
    pub receiver_node_id: NodeId,
    pub access_plane_id: String,
}

#[derive(Debug, Serialize)]
struct PacketSeqRangeWire(u64, u64);

#[derive(Debug, Deserialize)]
struct PacketSeqRangeWireOwned(u64, u64);

#[derive(Debug, Serialize)]
struct DeliveryLaneWire<'a>(&'a str, &'a str, &'a str);

#[derive(Debug, Deserialize)]
struct DeliveryLaneWireOwned(String, String, String);

#[derive(Debug, Serialize)]
struct WindowReportSourceWire<'a>(&'a str, HashBytes, u8);

#[derive(Debug, Deserialize)]
struct WindowReportSourceWireOwned(String, HashBytes, u8);

#[derive(Debug, Serialize)]
struct PacketStreamWindowReportWire<'a>(
    u8,
    DeliveryLaneWire<'a>,
    PacketSeqRangeWire,
    Vec<PacketSeqRangeWire>,
    WindowReportSourceWire<'a>,
    u64,
);

#[derive(Debug, Deserialize)]
struct PacketStreamWindowReportWireOwned(
    u8,
    DeliveryLaneWireOwned,
    PacketSeqRangeWireOwned,
    Vec<PacketSeqRangeWireOwned>,
    WindowReportSourceWireOwned,
    u64,
);

pub fn encode_packet_stream_window_report_body(
    report: &PacketStreamWindowReport,
) -> WireResult<Vec<u8>> {
    serde_cbor::to_vec(&PacketStreamWindowReportWire(
        1,
        DeliveryLaneWire(
            &report.lane.sender_node_id,
            &report.lane.receiver_node_id,
            &report.lane.access_plane_id,
        ),
        PacketSeqRangeWire(report.covered.from_seq, report.covered.to_seq),
        report
            .resend_ranges
            .iter()
            .map(|range| PacketSeqRangeWire(range.from_seq, range.to_seq))
            .collect(),
        WindowReportSourceWire(
            &report.source.stream_id,
            report.source.packet_hash,
            report.source.reason.reason_code(),
        ),
        report.created_at_secs,
    ))
    .map_err(Into::into)
}

pub fn decode_packet_stream_window_report_body(
    bytes: &[u8],
) -> WireResult<PacketStreamWindowReport> {
    let PacketStreamWindowReportWireOwned(
        version,
        DeliveryLaneWireOwned(sender_node_id, receiver_node_id, access_plane_id),
        PacketSeqRangeWireOwned(covered_from, covered_to),
        resend_ranges,
        WindowReportSourceWireOwned(stream_id, packet_hash, reason_code),
        created_at_secs,
    ) = serde_cbor::from_slice(bytes)?;
    if version != 1 {
        return Err(WireError::Codec(format!(
            "unsupported packet window report body version {version}"
        )));
    }
    Ok(PacketStreamWindowReport {
        lane: DeliveryLaneId {
            sender_node_id,
            receiver_node_id,
            access_plane_id,
        },
        covered: PacketSeqRange::new(covered_from, covered_to),
        resend_ranges: resend_ranges
            .into_iter()
            .map(|PacketSeqRangeWireOwned(from_seq, to_seq)| PacketSeqRange::new(from_seq, to_seq))
            .collect(),
        source: WindowReportSource {
            stream_id,
            packet_hash,
            reason: WindowReportReason::from_reason_code(reason_code)?,
        },
        created_at_secs,
    })
}
