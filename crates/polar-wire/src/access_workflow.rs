use std::collections::BTreeMap;

use polar_transport::AccessPlaneEndpointHints;
use serde::{Deserialize, Serialize};
use serde_bytes::{ByteBuf, Bytes};

use crate::{AccessPlaneRef, WireError, WireResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessWorkflowKind {
    P2pTrustApplication,
    RealmCaApplication,
    RealmEnrollment,
    RealmMemberTrust,
    CapabilityGrant,
    ServiceAccess,
}

impl AccessWorkflowKind {
    pub fn code(self) -> u16 {
        match self {
            Self::P2pTrustApplication => 1,
            Self::RealmCaApplication => 2,
            Self::RealmEnrollment => 3,
            Self::RealmMemberTrust => 4,
            Self::CapabilityGrant => 5,
            Self::ServiceAccess => 6,
        }
    }

    pub fn from_code(code: u16) -> WireResult<Self> {
        match code {
            1 => Ok(Self::P2pTrustApplication),
            2 => Ok(Self::RealmCaApplication),
            3 => Ok(Self::RealmEnrollment),
            4 => Ok(Self::RealmMemberTrust),
            5 => Ok(Self::CapabilityGrant),
            6 => Ok(Self::ServiceAccess),
            _ => Err(WireError::Codec(format!(
                "unknown access workflow kind code {code}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::P2pTrustApplication => "p2p_trust_application",
            Self::RealmCaApplication => "realm_ca_application",
            Self::RealmEnrollment => "realm_enrollment",
            Self::RealmMemberTrust => "realm_member_trust",
            Self::CapabilityGrant => "capability_grant",
            Self::ServiceAccess => "service_access",
        }
    }
}

pub type PlaneId = String;
pub type DecisionKindText = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessWorkflowRequestBody {
    pub workflow_kind_code: u16,
    pub request_id: String,
    pub sender_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub receiver_node_id: String,
    pub local_plane: PlaneId,
    pub remote_plane: PlaneId,
    pub local_plane_ref: AccessPlaneRef,
    pub remote_plane_ref: AccessPlaneRef,
    pub timestamp_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ca_der_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub csr_der: Vec<u8>,
}

impl AccessWorkflowRequestBody {
    pub fn workflow_kind(&self) -> WireResult<AccessWorkflowKind> {
        AccessWorkflowKind::from_code(self.workflow_kind_code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessWorkflowDecisionBody {
    pub workflow_kind_code: u16,
    pub from_node_id: String,
    pub to_node_id: String,
    pub sender_node_name: String,
    pub sender_binding_pubkey: String,
    pub local_plane: PlaneId,
    pub remote_plane: PlaneId,
    pub local_plane_ref: AccessPlaneRef,
    pub remote_plane_ref: AccessPlaneRef,
    pub kind: DecisionKindText,
    pub kind_code: u16,
    pub timestamp_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ca_der_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_pubkey_hex: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plane_endpoints: AccessPlaneEndpointHints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_root_ca_pem: Option<String>,
}

impl AccessWorkflowDecisionBody {
    pub fn workflow_kind(&self) -> WireResult<AccessWorkflowKind> {
        AccessWorkflowKind::from_code(self.workflow_kind_code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessWorkflowReceiptBody {
    pub workflow_kind_code: u16,
    pub request_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub access_plane: PlaneId,
    pub access_plane_ref: AccessPlaneRef,
    pub timestamp_secs: u64,
}

impl AccessWorkflowReceiptBody {
    pub fn workflow_kind(&self) -> WireResult<AccessWorkflowKind> {
        AccessWorkflowKind::from_code(self.workflow_kind_code)
    }
}

#[derive(Debug, Serialize)]
struct AccessWorkflowRequestWire<'a>(
    u8,
    u16,
    &'a str,
    &'a str,
    &'a str,
    ByteBuf,
    &'a str,
    AccessPlaneRef,
    AccessPlaneRef,
    &'a str,
    &'a str,
    u64,
    Option<ByteBuf>,
    Option<ByteBuf>,
    &'a AccessPlaneEndpointHints,
    Option<&'a str>,
    &'a Bytes,
);

#[derive(Debug, Serialize, Deserialize)]
struct AccessWorkflowRequestWireOwned(
    u8,
    u16,
    String,
    String,
    String,
    ByteBuf,
    String,
    AccessPlaneRef,
    AccessPlaneRef,
    PlaneId,
    PlaneId,
    u64,
    Option<ByteBuf>,
    Option<ByteBuf>,
    AccessPlaneEndpointHints,
    Option<String>,
    ByteBuf,
);

#[derive(Debug, Serialize)]
struct AccessWorkflowDecisionWire<'a>(
    u8,
    u16,
    &'a str,
    &'a str,
    &'a str,
    ByteBuf,
    AccessPlaneRef,
    AccessPlaneRef,
    &'a str,
    &'a str,
    u16,
    &'a str,
    u64,
    Option<ByteBuf>,
    Option<ByteBuf>,
    &'a AccessPlaneEndpointHints,
    Option<ByteBuf>,
    Option<Vec<ByteBuf>>,
);

#[derive(Debug, Serialize, Deserialize)]
struct AccessWorkflowDecisionWireOwned(
    u8,
    u16,
    String,
    String,
    String,
    ByteBuf,
    AccessPlaneRef,
    AccessPlaneRef,
    PlaneId,
    PlaneId,
    u16,
    DecisionKindText,
    u64,
    Option<ByteBuf>,
    Option<ByteBuf>,
    AccessPlaneEndpointHints,
    Option<ByteBuf>,
    Option<Vec<ByteBuf>>,
);

#[derive(Debug, Serialize)]
struct AccessWorkflowReceiptWire<'a>(
    u8,
    u16,
    &'a str,
    &'a str,
    &'a str,
    AccessPlaneRef,
    &'a str,
    u64,
);

#[derive(Debug, Serialize, Deserialize)]
struct AccessWorkflowReceiptWireOwned(
    u8,
    u16,
    String,
    String,
    String,
    AccessPlaneRef,
    PlaneId,
    u64,
);

pub fn encode_access_workflow_request_body(
    body: &AccessWorkflowRequestBody,
) -> WireResult<Vec<u8>> {
    AccessWorkflowKind::from_code(body.workflow_kind_code)?;
    check_size(body.csr_der.len(), MAX_CONTROL_DER_BYTES, "CSR")?;
    serde_cbor::to_vec(&AccessWorkflowRequestWire(
        3,
        body.workflow_kind_code,
        &body.request_id,
        &body.sender_node_id,
        &body.sender_node_name,
        hex_bytes(&body.sender_binding_pubkey, 32, Some(32), "binding key")?,
        &body.receiver_node_id,
        body.local_plane_ref,
        body.remote_plane_ref,
        &body.local_plane,
        &body.remote_plane,
        body.timestamp_secs,
        optional_hex(
            body.peer_ca_der_hex.as_deref(),
            MAX_CONTROL_DER_BYTES,
            None,
            "peer CA",
        )?,
        optional_hex(
            body.peer_session_pubkey_hex.as_deref(),
            32,
            Some(32),
            "session key",
        )?,
        &body.plane_endpoints,
        body.admission_token.as_deref(),
        Bytes::new(&body.csr_der),
    ))
    .map_err(Into::into)
}

pub fn decode_access_workflow_request_body(bytes: &[u8]) -> WireResult<AccessWorkflowRequestBody> {
    let AccessWorkflowRequestWireOwned(
        v,
        workflow_kind_code,
        request_id,
        sender_node_id,
        sender_node_name,
        sender_binding_pubkey,
        receiver_node_id,
        local_plane_ref,
        remote_plane_ref,
        local_plane,
        remote_plane,
        timestamp_secs,
        peer_ca_der_hex,
        peer_session_pubkey_hex,
        plane_endpoints,
        admission_token,
        csr_der,
    ) = decode_canonical(bytes)?;
    if v != 3 {
        return Err(WireError::Codec(format!(
            "unsupported access workflow request body version {}",
            v
        )));
    }
    AccessWorkflowKind::from_code(workflow_kind_code)?;
    Ok(AccessWorkflowRequestBody {
        workflow_kind_code,
        request_id,
        sender_node_id,
        sender_node_name,
        sender_binding_pubkey: checked_hex(sender_binding_pubkey, 32, Some(32), "binding key")?,
        receiver_node_id,
        local_plane,
        remote_plane,
        local_plane_ref,
        remote_plane_ref,
        timestamp_secs,
        peer_ca_der_hex: peer_ca_der_hex
            .map(|b| checked_hex(b, MAX_CONTROL_DER_BYTES, None, "peer CA"))
            .transpose()?,
        peer_session_pubkey_hex: peer_session_pubkey_hex
            .map(|b| checked_hex(b, 32, Some(32), "session key"))
            .transpose()?,
        plane_endpoints,
        admission_token,
        csr_der: checked_der(csr_der, "CSR")?,
    })
}

pub fn encode_access_workflow_decision_body(
    body: &AccessWorkflowDecisionBody,
) -> WireResult<Vec<u8>> {
    AccessWorkflowKind::from_code(body.workflow_kind_code)?;
    serde_cbor::to_vec(&AccessWorkflowDecisionWire(
        3,
        body.workflow_kind_code,
        &body.from_node_id,
        &body.to_node_id,
        &body.sender_node_name,
        hex_bytes(&body.sender_binding_pubkey, 32, Some(32), "binding key")?,
        body.local_plane_ref,
        body.remote_plane_ref,
        &body.local_plane,
        &body.remote_plane,
        body.kind_code,
        &body.kind,
        body.timestamp_secs,
        optional_hex(
            body.peer_ca_der_hex.as_deref(),
            MAX_CONTROL_DER_BYTES,
            None,
            "peer CA",
        )?,
        optional_hex(
            body.peer_session_pubkey_hex.as_deref(),
            32,
            Some(32),
            "session key",
        )?,
        &body.plane_endpoints,
        certificate_der(body.realm_cert_pem.as_deref())?,
        certificate_chain_der(body.realm_root_ca_pem.as_deref())?,
    ))
    .map_err(Into::into)
}

pub fn decode_access_workflow_decision_body(
    bytes: &[u8],
) -> WireResult<AccessWorkflowDecisionBody> {
    let AccessWorkflowDecisionWireOwned(
        v,
        workflow_kind_code,
        from_node_id,
        to_node_id,
        sender_node_name,
        sender_binding_pubkey,
        local_plane_ref,
        remote_plane_ref,
        local_plane,
        remote_plane,
        kind_code,
        kind,
        timestamp_secs,
        peer_ca_der_hex,
        peer_session_pubkey_hex,
        plane_endpoints,
        realm_cert_pem,
        realm_root_ca_pem,
    ) = decode_canonical(bytes)?;
    if v != 3 {
        return Err(WireError::Codec(format!(
            "unsupported access workflow decision body version {}",
            v
        )));
    }
    AccessWorkflowKind::from_code(workflow_kind_code)?;
    Ok(AccessWorkflowDecisionBody {
        workflow_kind_code,
        from_node_id,
        to_node_id,
        sender_node_name,
        sender_binding_pubkey: checked_hex(sender_binding_pubkey, 32, Some(32), "binding key")?,
        local_plane,
        remote_plane,
        local_plane_ref,
        remote_plane_ref,
        kind,
        kind_code,
        timestamp_secs,
        peer_ca_der_hex: peer_ca_der_hex
            .map(|b| checked_hex(b, MAX_CONTROL_DER_BYTES, None, "peer CA"))
            .transpose()?,
        peer_session_pubkey_hex: peer_session_pubkey_hex
            .map(|b| checked_hex(b, 32, Some(32), "session key"))
            .transpose()?,
        plane_endpoints,
        realm_cert_pem: realm_cert_pem
            .map(|b| certificate_pem(&[b], 1))
            .transpose()?,
        realm_root_ca_pem: realm_root_ca_pem
            .map(|chain| certificate_pem(&chain, MAX_CONTROL_CA_CHAIN))
            .transpose()?,
    })
}

pub fn encode_access_workflow_receipt_body(
    body: &AccessWorkflowReceiptBody,
) -> WireResult<Vec<u8>> {
    AccessWorkflowKind::from_code(body.workflow_kind_code)?;
    serde_cbor::to_vec(&AccessWorkflowReceiptWire(
        1,
        body.workflow_kind_code,
        &body.request_id,
        &body.from_node_id,
        &body.to_node_id,
        body.access_plane_ref,
        &body.access_plane,
        body.timestamp_secs,
    ))
    .map_err(Into::into)
}

pub fn decode_access_workflow_receipt_body(bytes: &[u8]) -> WireResult<AccessWorkflowReceiptBody> {
    let AccessWorkflowReceiptWireOwned(
        v,
        workflow_kind_code,
        request_id,
        from_node_id,
        to_node_id,
        access_plane_ref,
        access_plane,
        timestamp_secs,
    ) = decode_canonical(bytes)?;
    if v != 1 {
        return Err(WireError::Codec(format!(
            "unsupported access workflow receipt body version {}",
            v
        )));
    }
    AccessWorkflowKind::from_code(workflow_kind_code)?;
    Ok(AccessWorkflowReceiptBody {
        workflow_kind_code,
        request_id,
        from_node_id,
        to_node_id,
        access_plane,
        access_plane_ref,
        timestamp_secs,
    })
}

/// Bounds of the reviewed certificate/CSR material profile, not a whole-body limit.
pub const MAX_CONTROL_DER_BYTES: usize = 2048;
pub const MAX_CONTROL_CA_CHAIN: usize = 4;

fn check_size(actual: usize, maximum: usize, label: &str) -> WireResult<()> {
    if actual > maximum {
        return Err(WireError::Codec(format!("{label} exceeds {maximum} bytes")));
    }
    Ok(())
}
fn hex_bytes(
    value: &str,
    maximum: usize,
    exact: Option<usize>,
    label: &str,
) -> WireResult<ByteBuf> {
    check_size(value.len(), maximum * 2, label)?;
    let bytes = hex::decode(value).map_err(|e| WireError::Codec(format!("{label}: {e}")))?;
    if exact.is_some_and(|n| bytes.len() != n) {
        return Err(WireError::Codec(format!("{label} has invalid length")));
    }
    Ok(ByteBuf::from(bytes))
}
fn optional_hex(
    value: Option<&str>,
    maximum: usize,
    exact: Option<usize>,
    label: &str,
) -> WireResult<Option<ByteBuf>> {
    value
        .map(|v| hex_bytes(v, maximum, exact, label))
        .transpose()
}
fn checked_hex(
    bytes: ByteBuf,
    maximum: usize,
    exact: Option<usize>,
    label: &str,
) -> WireResult<String> {
    check_size(bytes.len(), maximum, label)?;
    if exact.is_some_and(|n| bytes.len() != n) {
        return Err(WireError::Codec(format!("{label} has invalid length")));
    }
    Ok(hex::encode(bytes))
}
fn checked_der(bytes: ByteBuf, label: &str) -> WireResult<Vec<u8>> {
    check_size(bytes.len(), MAX_CONTROL_DER_BYTES, label)?;
    Ok(bytes.into_vec())
}
fn parse_certificates(value: &str, max_count: usize) -> WireResult<Vec<ByteBuf>> {
    // Bounded text adaptation before parsing. 4 KiB accommodates one 2 KiB DER
    // certificate in the PEM emitted by the existing PKI implementation.
    check_size(value.len(), 4096 * max_count, "certificate PEM")?;
    let pems =
        pem::parse_many(value).map_err(|e| WireError::Codec(format!("certificate PEM: {e}")))?;
    if pems.is_empty() || pems.len() > max_count {
        return Err(WireError::Codec("invalid certificate count".into()));
    }
    pems.into_iter()
        .map(|p| {
            if p.tag() != "CERTIFICATE" || p.contents().is_empty() {
                return Err(WireError::Codec("expected nonempty CERTIFICATE PEM".into()));
            }
            check_size(p.contents().len(), MAX_CONTROL_DER_BYTES, "certificate DER")?;
            Ok(ByteBuf::from(p.into_contents()))
        })
        .collect()
}
fn certificate_der(value: Option<&str>) -> WireResult<Option<ByteBuf>> {
    value
        .map(|v| Ok(parse_certificates(v, 1)?.remove(0)))
        .transpose()
}
fn certificate_chain_der(value: Option<&str>) -> WireResult<Option<Vec<ByteBuf>>> {
    value
        .map(|v| parse_certificates(v, MAX_CONTROL_CA_CHAIN))
        .transpose()
}
fn certificate_pem(chain: &[ByteBuf], max_count: usize) -> WireResult<String> {
    if chain.is_empty() || chain.len() > max_count {
        return Err(WireError::Codec("invalid certificate count".into()));
    }
    let mut value = String::new();
    for der in chain {
        check_size(der.len(), MAX_CONTROL_DER_BYTES, "certificate DER")?;
        if der.is_empty() {
            return Err(WireError::Codec("empty certificate DER".into()));
        }
        value.push_str(&pem::encode(&pem::Pem::new("CERTIFICATE", der.as_ref())));
    }
    Ok(value)
}
fn decode_canonical<T: serde::de::DeserializeOwned + Serialize>(bytes: &[u8]) -> WireResult<T> {
    let value = serde_cbor::from_slice(bytes)?;
    if serde_cbor::to_vec(&value)? != bytes {
        return Err(WireError::Codec(
            "noncanonical access control encoding".into(),
        ));
    }
    Ok(value)
}


