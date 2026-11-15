use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{from_str as json_from_str, to_string as json_to_string};

pub mod endpoint_hint_policy;
pub use endpoint_hint_policy::*;
pub mod listener;
pub use listener::ListenerBindPolicy;

pub const ENDPOINT_PORT_POOL_NODE_SETTING_KEY: &str = "transport.endpoint_port_pool.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EndpointPortPool {
    #[serde(default)]
    pub slots: Vec<EndpointPortSlot>,
}

impl EndpointPortPool {
    pub fn validate(&self) -> Result<(), EndpointPortPoolValidationError> {
        let mut slot_ids = BTreeSet::new();
        let mut local_endpoints = BTreeSet::new();
        let mut advertised_endpoints = BTreeSet::new();
        for slot in &self.slots {
            slot.validate()?;
            if !slot_ids.insert(slot.slot_id.clone()) {
                return Err(EndpointPortPoolValidationError::DuplicateSlotId(
                    slot.slot_id.clone(),
                ));
            }
            let local = slot.local_endpoint_key();
            if !local_endpoints.insert(local.clone()) {
                return Err(EndpointPortPoolValidationError::DuplicateLocalEndpoint(
                    local,
                ));
            }
            let advertised = slot.advertised_endpoint_key();
            if !advertised_endpoints.insert(advertised.clone()) {
                return Err(
                    EndpointPortPoolValidationError::DuplicateAdvertisedEndpoint(advertised),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointPortSlot {
    pub slot_id: String,
    pub local_host: String,
    pub local_port: u16,
    pub advertised_host: String,
    pub advertised_port: u16,
}

impl EndpointPortSlot {
    pub fn validate(&self) -> Result<(), EndpointPortPoolValidationError> {
        if self.slot_id.trim().is_empty() {
            return Err(EndpointPortPoolValidationError::EmptySlotId);
        }
        if self.local_host.trim().is_empty() {
            return Err(EndpointPortPoolValidationError::EmptyLocalHost(
                self.slot_id.clone(),
            ));
        }
        if self.advertised_host.trim().is_empty() {
            return Err(EndpointPortPoolValidationError::EmptyAdvertisedHost(
                self.slot_id.clone(),
            ));
        }
        if self.local_port == 0 {
            return Err(EndpointPortPoolValidationError::ZeroLocalPort(
                self.slot_id.clone(),
            ));
        }
        if self.advertised_port == 0 {
            return Err(EndpointPortPoolValidationError::ZeroAdvertisedPort(
                self.slot_id.clone(),
            ));
        }
        Ok(())
    }

    pub fn local_endpoint_key(&self) -> String {
        format!("{}:{}", self.local_host.trim(), self.local_port)
    }

    pub fn advertised_endpoint_key(&self) -> String {
        format!("{}:{}", self.advertised_host.trim(), self.advertised_port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointPortPoolValidationError {
    EmptySlotId,
    EmptyLocalHost(String),
    EmptyAdvertisedHost(String),
    ZeroLocalPort(String),
    ZeroAdvertisedPort(String),
    DuplicateSlotId(String),
    DuplicateLocalEndpoint(String),
    DuplicateAdvertisedEndpoint(String),
}

impl std::fmt::Display for EndpointPortPoolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySlotId => write!(f, "endpoint port pool slot_id must not be empty"),
            Self::EmptyLocalHost(slot_id) => {
                write!(
                    f,
                    "endpoint port pool slot {slot_id} local_host must not be empty"
                )
            }
            Self::EmptyAdvertisedHost(slot_id) => write!(
                f,
                "endpoint port pool slot {slot_id} advertised_host must not be empty"
            ),
            Self::ZeroLocalPort(slot_id) => {
                write!(
                    f,
                    "endpoint port pool slot {slot_id} local_port must not be zero"
                )
            }
            Self::ZeroAdvertisedPort(slot_id) => write!(
                f,
                "endpoint port pool slot {slot_id} advertised_port must not be zero"
            ),
            Self::DuplicateSlotId(slot_id) => {
                write!(f, "endpoint port pool slot_id {slot_id} is duplicated")
            }
            Self::DuplicateLocalEndpoint(endpoint) => write!(
                f,
                "endpoint port pool local endpoint {endpoint} is duplicated"
            ),
            Self::DuplicateAdvertisedEndpoint(endpoint) => write!(
                f,
                "endpoint port pool advertised endpoint {endpoint} is duplicated"
            ),
        }
    }
}

impl std::error::Error for EndpointPortPoolValidationError {}

// ── hex_bytes serde helper ────────────────────────────────────────────────────

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        hex.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let hex = String::deserialize(d)?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportBackendKind {
    #[default]
    Zenoh,
    Libp2pMock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaneSemanticKind {
    Bootstrap,
    #[default]
    Member,
    Realm,
    ControlBus,
    #[serde(rename = "custom", alias = "user_defined")]
    UserDefined,
}

pub type PlaneKind = PlaneSemanticKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaneRuntimeState {
    Disabled,
    #[default]
    Starting,
    Running,
    Reconciling,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransportMaterialState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconcile_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconcile_outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneRuntimeStatus {
    pub plane_id: String,
    pub plane_kind: PlaneKind,
    pub backend: TransportBackendKind,
    pub enabled: bool,
    pub runtime_state: PlaneRuntimeState,
    pub material: TransportMaterialState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransportMaterial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMaterialReconcileOutcome {
    NoChange,
    ListenerRebind,
    MockApplied,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportMaterialReconcileRequest {
    pub plane_id: String,
    pub backend: TransportBackendKind,
    #[serde(default)]
    pub listener_addrs: Vec<String>,
    pub reason: String,
    pub material: TransportMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportMaterialReconcileReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_fingerprint: Option<String>,
    pub reason: String,
    pub outcome: TransportMaterialReconcileOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryFrame {
    pub node_id: String,
    pub node_name: String,
    pub binding_pubkey: String,
    pub timestamp_secs: u64,
    pub intent: String,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransportEndpointHintSet {
    pub backend: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
}

pub type AccessPlaneEndpointHints = BTreeMap<String, Vec<TransportEndpointHintSet>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTrustRequestTransportBody {
    pub request_id: String,
    pub sender_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub receiver_node_id: String,
    pub sender_plane: String,
    pub receiver_plane: String,
    pub timestamp_secs: u64,
    #[serde(default)]
    pub peer_ca_der_hex: Option<String>,
    #[serde(default)]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    #[serde(default)]
    pub admission_token: Option<String>,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalApproveFrame {
    pub peer_node_id: String,
    pub approved_at_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTrustDecisionTransportBody {
    pub from_node_id: String,
    pub to_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub sender_plane: String,
    pub receiver_plane: String,
    pub kind: String,
    pub timestamp_secs: u64,
    pub peer_ca_der_hex: Option<String>,
    #[serde(default)]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmEnrollmentRequestTransportBody {
    pub request_id: String,
    pub sender_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub access_plane: String,
    #[serde(default)]
    pub receiver_plane: String,
    pub timestamp_secs: u64,
    #[serde(default)]
    pub admission_token: Option<String>,
    #[serde(with = "hex_bytes")]
    pub csr_der: Vec<u8>,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmEnrollmentDecisionTransportBody {
    pub from_node_id: String,
    pub to_node_id: String,
    pub access_plane: String,
    #[serde(default)]
    pub receiver_plane: String,
    pub kind: String,
    pub timestamp_secs: u64,
    #[serde(default)]
    pub realm_cert_pem: Option<String>,
    #[serde(default)]
    pub realm_root_ca_pem: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmMemberAccessTrustRequestTransportBody {
    pub request_id: String,
    pub sender_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub access_plane: String,
    #[serde(default)]
    pub receiver_plane: String,
    pub timestamp_secs: u64,
    #[serde(default)]
    pub admission_token: Option<String>,
    #[serde(default)]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmMemberAccessTrustDecisionTransportBody {
    pub from_node_id: String,
    pub to_node_id: String,
    pub access_plane: String,
    #[serde(default)]
    pub receiver_plane: String,
    pub kind: String,
    pub timestamp_secs: u64,
    #[serde(default)]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessWorkflowReceiptTransportBody {
    pub workflow_kind: String,
    pub request_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub access_plane: String,
    pub timestamp_secs: u64,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmReplayAckSegment {
    pub previous_acked_message_id: String,
    pub acked_message_id: String,
    pub received_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmReplayTransportRequest {
    pub sender_node_id: String,
    pub target_node_id: String,
    pub access_plane_id: String,
    pub ack_segment: DmReplayAckSegment,
    pub timestamp_secs: u64,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatFrame {
    pub node_id: String,
    pub node_name: String,
    pub binding_pubkey: String,
    pub timestamp_secs: u64,
    pub status: String,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionKeyUpdateFrame {
    pub node_id: String,
    pub peer_node_id: String,
    pub epoch: u64,
    pub ephemeral_pubkey: String,
    pub expires_at_secs: u64,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedPayloadEnvelope {
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LgpTransportEnvelope {
    pub route_kind: String,
    pub sender_node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_plane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_plane: Option<String>,
    pub owner_node_id: String,
    pub channel_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub message_kind: String,
    pub intent: String,
    pub payload_family: String,
    pub payload_encoding: String,
    pub payload_compression: String,
    pub sealed_payload: SealedPayloadEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_owner_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partner_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sender_node_id: Option<String>,
    pub channel_title: Option<String>,
    pub thread_tag: Option<String>,
    pub created_at_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_ts_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_peer_receipt: Option<DmReplayAckSegment>,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDistributionFrame {
    pub sender_node_id: String,
    pub target_node_id: String,
    pub capability_name: String,
    pub version: Option<String>,
    pub timestamp_secs: u64,
    pub force_install: bool,
    pub force_auth_overwrite: bool,
    pub auth_sha256: String,
    pub sealed_auth_ephemeral_pubkey_hex: String,
    pub sealed_auth_nonce_hex: String,
    pub sealed_auth_ciphertext_hex: String,
    pub sig_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body")]
pub enum TransportFrame {
    Discovery(DiscoveryFrame),
    LocalApprove(LocalApproveFrame),
    Heartbeat(HeartbeatFrame),
}

impl TransportFrame {
    pub fn legacy_business_kind(&self) -> Option<&'static str> {
        None
    }

    pub fn as_lgp_wire_envelope(&self) -> Option<&LgpTransportEnvelope> {
        None
    }

    pub fn access_trust_request(_body: AccessTrustRequestTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn trust_decision(_body: AccessTrustDecisionTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn realm_enrollment_request(_body: RealmEnrollmentRequestTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn realm_enrollment_decision(_body: RealmEnrollmentDecisionTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn realm_member_trust_request(_body: RealmMemberAccessTrustRequestTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn realm_member_trust_decision(_body: RealmMemberAccessTrustDecisionTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn access_workflow_receipt(_body: AccessWorkflowReceiptTransportBody) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn session_key_update(_body: SessionKeyUpdateFrame) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn direct_dm_replay_request(_body: DmReplayTransportRequest) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn capability_distribution(_body: CapabilityDistributionFrame) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn lgp_transport_envelope(_body: LgpTransportEnvelope) -> Self {
        panic!(
            "business TransportFrame variants are removed; publish a raw PolarWirePacket payload"
        )
    }

    pub fn as_trust_request(&self) -> Option<&AccessTrustRequestTransportBody> {
        None
    }

    pub fn as_trust_decision(&self) -> Option<&AccessTrustDecisionTransportBody> {
        None
    }

    pub fn as_realm_enrollment_request(&self) -> Option<&RealmEnrollmentRequestTransportBody> {
        None
    }

    pub fn as_realm_enrollment_decision(&self) -> Option<&RealmEnrollmentDecisionTransportBody> {
        None
    }

    pub fn as_realm_member_trust_request(
        &self,
    ) -> Option<&RealmMemberAccessTrustRequestTransportBody> {
        None
    }

    pub fn as_realm_member_trust_decision(
        &self,
    ) -> Option<&RealmMemberAccessTrustDecisionTransportBody> {
        None
    }

    pub fn as_access_workflow_receipt(&self) -> Option<&AccessWorkflowReceiptTransportBody> {
        None
    }

    pub fn as_session_key_update(&self) -> Option<&SessionKeyUpdateFrame> {
        None
    }

    pub fn as_dm_replay_request(&self) -> Option<&DmReplayTransportRequest> {
        None
    }

    pub fn as_capability_distribution(&self) -> Option<&CapabilityDistributionFrame> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body")]
pub enum TransportPayload {
    RawPolarWirePacket(#[serde(with = "hex_bytes")] Vec<u8>),
    Discovery(DiscoveryFrame),
    LocalApprove(LocalApproveFrame),
    Heartbeat(HeartbeatFrame),
    Status(#[serde(with = "hex_bytes")] Vec<u8>),
}

impl TransportPayload {
    pub fn raw_polar_wire_packet(raw_packet_bytes: impl Into<Vec<u8>>) -> Self {
        Self::RawPolarWirePacket(raw_packet_bytes.into())
    }

    pub fn raw_polar_wire_packet_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::RawPolarWirePacket(bytes) => Some(bytes),
            _ => None,
        }
    }

    pub fn transport_frame(&self) -> Option<TransportFrame> {
        match self {
            Self::Discovery(body) => Some(TransportFrame::Discovery(body.clone())),
            Self::LocalApprove(body) => Some(TransportFrame::LocalApprove(body.clone())),
            Self::Heartbeat(body) => Some(TransportFrame::Heartbeat(body.clone())),
            _ => None,
        }
    }
}

impl From<TransportFrame> for TransportPayload {
    fn from(frame: TransportFrame) -> Self {
        match frame {
            TransportFrame::Discovery(body) => Self::Discovery(body),
            TransportFrame::LocalApprove(body) => Self::LocalApprove(body),
            TransportFrame::Heartbeat(body) => Self::Heartbeat(body),
        }
    }
}

pub fn encode_frame(frame: &TransportFrame) -> serde_json::Result<String> {
    json_to_string(frame)
}

pub fn decode_frame(line: &str) -> serde_json::Result<TransportFrame> {
    json_from_str(line)
}

pub trait LineTransport {
    fn send_line(&mut self, line: &str) -> io::Result<()>;
    fn recv_line(&mut self) -> io::Result<Option<String>>;
}

#[derive(Debug, Default)]
pub struct MemoryEndpoint {
    inbound: VecDeque<String>,
    outbound: VecDeque<String>,
}

impl MemoryEndpoint {
    pub fn pair() -> (Self, Self) {
        (
            Self::default(),
            Self {
                inbound: VecDeque::new(),
                outbound: VecDeque::new(),
            },
        )
    }

    pub fn flush_to(&mut self, other: &mut Self) {
        while let Some(line) = self.outbound.pop_front() {
            other.inbound.push_back(line);
        }
    }
}

impl LineTransport for MemoryEndpoint {
    fn send_line(&mut self, line: &str) -> io::Result<()> {
        self.outbound.push_back(line.to_string());
        Ok(())
    }

    fn recv_line(&mut self) -> io::Result<Option<String>> {
        Ok(self.inbound.pop_front())
    }
}

pub struct LineIo<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> LineIo<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

impl<R, W> LineTransport for LineIo<R, W>
where
    R: BufRead,
    W: Write,
{
    fn send_line(&mut self, line: &str) -> io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    fn recv_line(&mut self) -> io::Result<Option<String>> {
        let mut buf = String::new();
        let read = self.reader.read_line(&mut buf)?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(buf.trim_end_matches(['\r', '\n']).to_string()))
    }
}


