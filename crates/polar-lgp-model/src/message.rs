use polar_identity::{NodeId, UnixTimestamp};
use polar_payload_codec::{
    decode_message_payload, MessagePayloadCodecError, MessagePayloadCompression,
    MessagePayloadEncoding, MessagePayloadFamily,
};
pub use polar_protocol::MessagePayload;
use serde::{Deserialize, Serialize};

/// Stable identity of one LGP message. `message_id` alone is never a key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LgpMessageKey {
    #[serde(with = "node_id_hex")]
    #[schemars(with = "String")]
    pub sender_id: NodeId,
    pub message_id: String,
}

/// Total-order cursor: timestamps and message ids can both collide across senders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LgpMessageCursor {
    pub created_at: u64,
    pub key: LgpMessageKey,
}

impl From<&MessageRecord> for LgpMessageCursor {
    fn from(row: &MessageRecord) -> Self {
        Self {
            created_at: row.created_at.as_secs(),
            key: LgpMessageKey::new(row.sender_id, row.message_id.clone()),
        }
    }
}

mod node_id_hex {
    use polar_identity::NodeId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(node_id: &NodeId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&node_id.to_hex())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<NodeId, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        NodeId::from_hex(&raw).map_err(serde::de::Error::custom)
    }
}

impl LgpMessageKey {
    pub fn new(sender_id: NodeId, message_id: impl Into<String>) -> Self {
        Self {
            sender_id,
            message_id: message_id.into(),
        }
    }
}

/// A full-key message reference plus its thread location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LgpMessageReference {
    pub key: LgpMessageKey,
    pub thread_id: String,
}

/// Typed projection of LGP's three distinct causal reference groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LgpCausalEnvelope {
    pub current: LgpMessageReference,
    pub causal_kind: Option<ThreadMessageCausalKind>,
    pub origin: Option<LgpMessageReference>,
    pub previous: Option<LgpMessageReference>,
    pub source: Option<LgpMessageReference>,
}

impl LgpCausalEnvelope {
    pub fn fresh(current: LgpMessageReference) -> Self {
        Self {
            current,
            causal_kind: None,
            origin: None,
            previous: None,
            source: None,
        }
    }

    pub fn reply(
        current: LgpMessageReference,
        parent: &Self,
        causal_kind: ThreadMessageCausalKind,
    ) -> Self {
        Self {
            current,
            causal_kind: Some(causal_kind),
            origin: Some(
                parent
                    .origin
                    .clone()
                    .unwrap_or_else(|| parent.current.clone()),
            ),
            previous: Some(parent.current.clone()),
            source: None,
        }
    }

    pub fn projection(
        current: LgpMessageReference,
        parent: &Self,
        causal_kind: ThreadMessageCausalKind,
    ) -> Self {
        Self {
            current,
            causal_kind: Some(causal_kind),
            origin: Some(
                parent
                    .origin
                    .clone()
                    .unwrap_or_else(|| parent.current.clone()),
            ),
            previous: Some(parent.current.clone()),
            source: Some(parent.current.clone()),
        }
    }
}
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Task,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageAppendStatus {
    Appended,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadMessageAuthorKind {
    LocalHuman,
    RemoteNode,
    TelegramUser,
    InhousePolar,
    SystemRuntime,
    ExternalConnectorUser,
    UnknownLegacy,
}

impl ThreadMessageAuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalHuman => "local_human",
            Self::RemoteNode => "remote_node",
            Self::TelegramUser => "telegram_user",
            Self::InhousePolar => "inhouse_polar",
            Self::SystemRuntime => "system_runtime",
            Self::ExternalConnectorUser => "external_connector_user",
            Self::UnknownLegacy => "unknown_legacy",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "local_human" => Self::LocalHuman,
            "remote_node" => Self::RemoteNode,
            "telegram_user" => Self::TelegramUser,
            "inhouse_polar" => Self::InhousePolar,
            "system_runtime" => Self::SystemRuntime,
            "external_connector_user" => Self::ExternalConnectorUser,
            _ => Self::UnknownLegacy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadMessageIngressKind {
    LocalComposer,
    ExplicitInhouseTurn,
    InhouseReply,
    ChatBridgeDelivery,
    DirectMessageTransport,
    TelegramTransport,
    ControlBusTransport,
    SystemRuntime,
    WebPublicHttpConnector,
    ProductApi,
    PolarInkConnector,
    LegacyUnknown,
}

impl ThreadMessageIngressKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalComposer => "local_composer",
            Self::ExplicitInhouseTurn => "explicit_inhouse_turn",
            Self::InhouseReply => "inhouse_reply",
            Self::ChatBridgeDelivery => "chat_bridge_delivery",
            Self::DirectMessageTransport => "direct_message_transport",
            Self::TelegramTransport => "telegram_transport",
            Self::ControlBusTransport => "control_bus_transport",
            Self::SystemRuntime => "system_runtime",
            Self::WebPublicHttpConnector => "web_public_http_connector",
            Self::ProductApi => "product_api",
            Self::PolarInkConnector => "polar_ink_connector",
            Self::LegacyUnknown => "legacy_unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "local_composer" => Self::LocalComposer,
            "explicit_inhouse_turn" => Self::ExplicitInhouseTurn,
            "inhouse_reply" => Self::InhouseReply,
            "chat_bridge_delivery" => Self::ChatBridgeDelivery,
            "direct_message_transport" => Self::DirectMessageTransport,
            "telegram_transport" => Self::TelegramTransport,
            "control_bus_transport" => Self::ControlBusTransport,
            "system_runtime" => Self::SystemRuntime,
            "web_public_http_connector" => Self::WebPublicHttpConnector,
            "product_api" => Self::ProductApi,
            "polar_ink_connector" => Self::PolarInkConnector,
            _ => Self::LegacyUnknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadMessageCausalKind {
    ChatBridgeDelivery,
    InhouseReply,
    Reply,
    SystemDerived,
    LegacyBackfill,
}

impl ThreadMessageCausalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatBridgeDelivery => "chat_bridge_delivery",
            Self::InhouseReply => "inhouse_reply",
            Self::Reply => "reply",
            Self::SystemDerived => "system_derived",
            Self::LegacyBackfill => "legacy_backfill",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "chat_bridge_delivery" => Some(Self::ChatBridgeDelivery),
            "inhouse_reply" => Some(Self::InhouseReply),
            "reply" => Some(Self::Reply),
            "system_derived" => Some(Self::SystemDerived),
            "legacy_backfill" => Some(Self::LegacyBackfill),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactCausalReferenceError {
    PartialTuple,
    EmptyMessageId,
    SelfReference,
}

impl std::fmt::Display for ExactCausalReferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PartialTuple => formatter.write_str(
                "causal_kind, causal_sender_id, and causal_message_id must be all present or all absent",
            ),
            Self::EmptyMessageId => formatter.write_str("causal_message_id must not be empty"),
            Self::SelfReference => formatter.write_str("a Message cannot causally reference itself"),
        }
    }
}

impl std::error::Error for ExactCausalReferenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactCausalReference {
    pub kind: ThreadMessageCausalKind,
    pub sender_id: NodeId,
    pub message_id: String,
}

impl ExactCausalReference {
    pub fn from_parts(
        kind: Option<ThreadMessageCausalKind>,
        sender_id: Option<NodeId>,
        message_id: Option<String>,
    ) -> Result<Option<Self>, ExactCausalReferenceError> {
        match (kind, sender_id, message_id) {
            (None, None, None) => Ok(None),
            (Some(kind), Some(sender_id), Some(message_id)) => {
                let message_id = message_id.trim();
                if message_id.is_empty() {
                    return Err(ExactCausalReferenceError::EmptyMessageId);
                }
                Ok(Some(Self {
                    kind,
                    sender_id,
                    message_id: message_id.to_string(),
                }))
            }
            _ => Err(ExactCausalReferenceError::PartialTuple),
        }
    }

    pub fn validate_child(
        &self,
        child_sender_id: &NodeId,
        child_message_id: &str,
    ) -> Result<(), ExactCausalReferenceError> {
        if self.sender_id == *child_sender_id && self.message_id == child_message_id {
            return Err(ExactCausalReferenceError::SelfReference);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LgpAuthorSignatureVersion {
    V5 = 5,
    V6 = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LgpSenderEvidenceV1 {
    pub signature_version: LgpAuthorSignatureVersion,
    pub protocol_version: u16,
    pub original_sender_node_id: NodeId,
    pub owner_node_id: NodeId,
    pub channel_owner_node_id: Option<NodeId>,
    pub channel_id: String,
    pub thread_id: String,
    pub partner_node_id: Option<NodeId>,
    pub message_id: String,
    pub author_message_hash: [u8; 32],
    pub original_author_signature: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedCausalMetadataError {
    InvalidJson(String),
    NotObject,
    UnknownEntry(String),
    InvalidSenderEvidence,
}

impl std::fmt::Display for ManagedCausalMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid causal metadata JSON: {error}"),
            Self::NotObject => formatter.write_str("causal metadata must be a JSON object"),
            Self::UnknownEntry(entry) => {
                write!(formatter, "unregistered causal metadata entry: {entry}")
            }
            Self::InvalidSenderEvidence => {
                formatter.write_str("sender_evidence_v1 must be a non-empty base64url string")
            }
        }
    }
}

impl std::error::Error for ManagedCausalMetadataError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedCausalMetadata {
    sender_evidence_v1: Option<String>,
}

impl ManagedCausalMetadata {
    fn validate_sender_evidence_v1(encoded: &str) -> Result<&str, ManagedCausalMetadataError> {
        let encoded = encoded.trim();
        if encoded.is_empty()
            || !encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(ManagedCausalMetadataError::InvalidSenderEvidence);
        }
        Ok(encoded)
    }

    pub fn from_json_str(raw: &str) -> Result<Self, ManagedCausalMetadataError> {
        let value: JsonValue = serde_json::from_str(raw)
            .map_err(|error| ManagedCausalMetadataError::InvalidJson(error.to_string()))?;
        let object = value
            .as_object()
            .ok_or(ManagedCausalMetadataError::NotObject)?;
        for key in object.keys() {
            if key != "sender_evidence_v1" {
                return Err(ManagedCausalMetadataError::UnknownEntry(key.clone()));
            }
        }
        let sender_evidence_v1 = match object.get("sender_evidence_v1") {
            None => None,
            Some(value) => {
                let encoded = Self::validate_sender_evidence_v1(
                    value
                        .as_str()
                        .ok_or(ManagedCausalMetadataError::InvalidSenderEvidence)?,
                )?;
                Some(encoded.to_string())
            }
        };
        Ok(Self { sender_evidence_v1 })
    }

    pub fn from_sender_evidence_v1(
        encoded: impl Into<String>,
    ) -> Result<Self, ManagedCausalMetadataError> {
        let encoded = encoded.into();
        let encoded = Self::validate_sender_evidence_v1(&encoded)?;
        Ok(Self {
            sender_evidence_v1: Some(encoded.to_string()),
        })
    }

    pub fn sender_evidence_v1(&self) -> Option<&str> {
        self.sender_evidence_v1.as_deref()
    }

    pub fn to_canonical_json(&self) -> String {
        match self.sender_evidence_v1.as_deref() {
            Some(encoded) => serde_json::json!({"sender_evidence_v1": encoded}).to_string(),
            None => "{}".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRecord {
    pub sender_id: NodeId,
    pub sender_inhouse_polar_id: Option<String>,
    pub message_id: String,
    pub owner_id: NodeId,
    pub thread_id: String,
    pub channel_owner_id: NodeId,
    pub channel_id: String,
    pub partner_id: Option<NodeId>,
    pub by_inhouse_polar_id: Option<String>,
    pub by_human_label: Option<String>,
    pub rewritten_by_owner: bool,
    pub message_kind: MessageKind,
    pub author_kind: ThreadMessageAuthorKind,
    pub author_ref: Option<String>,
    pub ingress_kind: ThreadMessageIngressKind,
    pub ingress_ref: Option<String>,
    pub ingress_metadata_json: String,
    pub intent: String,
    pub payload_family: MessagePayloadFamily,
    pub payload_encoding: MessagePayloadEncoding,
    pub payload_compression: MessagePayloadCompression,
    pub payload_bytes: Vec<u8>,
    pub causal_kind: Option<ThreadMessageCausalKind>,
    pub causal_sender_id: Option<NodeId>,
    pub causal_message_id: Option<String>,
    pub trace_id: Option<String>,
    pub origin_thread_id: Option<String>,
    pub origin_message_id: Option<String>,
    pub previous_thread_id: Option<String>,
    pub previous_message_id: Option<String>,
    pub source_thread_id: Option<String>,
    pub source_message_id: Option<String>,
    pub chat_bridge_id: Option<String>,
    pub chat_route_id: Option<String>,
    pub hop_count: u32,
    pub causal_metadata_json: String,
    pub created_at: UnixTimestamp,
}

impl MessageRecord {
    pub fn primary_key(&self) -> LgpMessageKey {
        LgpMessageKey::new(self.sender_id, self.message_id.clone())
    }

    pub fn payload(&self) -> Result<MessagePayload, MessagePayloadCodecError> {
        decode_message_payload(
            self.payload_family,
            self.payload_encoding,
            self.payload_compression,
            &self.payload_bytes,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAttachmentStorage {
    Inline,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttachmentReplica {
    /// Location claim only; never an access grant or local replica proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<polar_cas::source_hint::SourceHint>,
    pub replica_id: Option<String>,
    pub storage_family: String,
    pub replica_role: String,
    pub acquisition_mode: String,
    pub locator: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttachment {
    pub id: Option<String>,
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub data_url: Option<String>,
    pub path: Option<String>,
    pub blob_storage_family: Option<String>,
    pub blob_locator: Option<String>,
    pub blob_content_digest: Option<String>,
    pub blob_size_bytes: Option<u64>,
    #[serde(default)]
    pub replicas: Vec<MessageAttachmentReplica>,
    pub status: Option<String>,
    pub storage: MessageAttachmentStorage,
}

impl MessageAttachment {
    pub fn has_bytes_source(&self) -> bool {
        self.data_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .path
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .blob_locator
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePayloadAttachmentProjectionError {
    NonObjectPayload,
}

impl std::fmt::Display for MessagePayloadAttachmentProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("message attachments can only be projected into a JSON object payload")
    }
}

impl std::error::Error for MessagePayloadAttachmentProjectionError {}

/// Projects already-governed attachment references into the canonical LGP
/// JSON payload shape used by system-message ingress.
pub fn message_payload_with_attachments(
    payload: MessagePayload,
    attachments: &[MessageAttachment],
) -> Result<MessagePayload, MessagePayloadAttachmentProjectionError> {
    let MessagePayload::Json { value } = payload else {
        return Err(MessagePayloadAttachmentProjectionError::NonObjectPayload);
    };
    let serde_json::Value::Object(mut object) = value else {
        return Err(MessagePayloadAttachmentProjectionError::NonObjectPayload);
    };
    let mut inline = Vec::new();
    let mut external = Vec::new();
    for attachment in attachments {
        let value = message_attachment_json_value(attachment);
        match attachment.storage {
            MessageAttachmentStorage::Inline => inline.push(value),
            MessageAttachmentStorage::External => external.push(value),
        }
    }
    object.insert(
        "inline_attachments".to_owned(),
        serde_json::Value::Array(inline),
    );
    object.insert(
        "external_attachments".to_owned(),
        serde_json::Value::Array(external),
    );
    Ok(MessagePayload::Json {
        value: serde_json::Value::Object(object),
    })
}

fn message_attachment_json_value(attachment: &MessageAttachment) -> serde_json::Value {
    serde_json::json!({
        "id": attachment.id,
        "kind": attachment.kind,
        "name": attachment.name,
        "mime_type": attachment.mime_type,
        "data_url": attachment.data_url,
        "path": attachment.path,
        "storage_family": attachment.blob_storage_family,
        "locator": attachment.blob_locator,
        "content_digest": attachment.blob_content_digest,
        "size_bytes": attachment.blob_size_bytes,
        "replicas": attachment.replicas,
        "status": attachment.status,
    })
}

pub fn payload_message_attachments(payload: &MessagePayload) -> Vec<MessageAttachment> {
    message_payload_json_value(payload)
        .map(|value| json_message_attachments(&value))
        .unwrap_or_default()
}

pub fn payload_message_attachment_by_id(
    payload: &MessagePayload,
    attachment_id: &str,
) -> Option<MessageAttachment> {
    let attachment_id = attachment_id.trim();
    if attachment_id.is_empty() {
        return None;
    }
    payload_message_attachments(payload)
        .into_iter()
        .find(|attachment| attachment.id.as_deref() == Some(attachment_id))
}

fn message_payload_json_value(payload: &MessagePayload) -> Option<JsonValue> {
    match payload {
        MessagePayload::Json { value } => Some(value.clone()),
        MessagePayload::Text { text } => Some(serde_json::json!({ "text": text })),
        MessagePayload::Binary { .. } => None,
    }
}

fn json_message_attachments(value: &JsonValue) -> Vec<MessageAttachment> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let has_split_shape =
        object.contains_key("inline_attachments") || object.contains_key("external_attachments");
    let mut attachments = Vec::new();
    if has_split_shape {
        attachments.extend(json_attachment_rows(
            object.get("inline_attachments"),
            MessageAttachmentStorage::Inline,
        ));
        attachments.extend(json_attachment_rows(
            object.get("external_attachments"),
            MessageAttachmentStorage::External,
        ));
    }
    attachments
}

fn json_attachment_rows(
    rows: Option<&JsonValue>,
    default_storage: MessageAttachmentStorage,
) -> Vec<MessageAttachment> {
    let Some(rows) = rows.and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| json_attachment_row(row, default_storage))
        .collect()
}

fn json_attachment_row(
    row: &JsonValue,
    default_storage: MessageAttachmentStorage,
) -> Option<MessageAttachment> {
    let object = row.as_object()?;
    let path = object
        .get("path")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let data_url = object
        .get("data_url")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    let blob_storage_family = object
        .get("storage_family")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let blob_locator = object
        .get("locator")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let blob_content_digest = object
        .get("content_digest")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let blob_size_bytes = object.get("size_bytes").and_then(JsonValue::as_u64);
    let replicas = object
        .get("replicas")
        .and_then(JsonValue::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let object = row.as_object()?;
                    let storage_family = object
                        .get("storage_family")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let replica_role = object
                        .get("replica_role")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let acquisition_mode = object
                        .get("acquisition_mode")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let locator = object
                        .get("locator")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let source_hint = match object.get("source_hint").filter(|v| !v.is_null()) {
                        Some(value) => {
                            let hint: polar_cas::source_hint::SourceHint =
                                serde_json::from_value(value.clone()).ok()?;
                            hint.validate().ok()?;
                            Some(hint)
                        }
                        None => None,
                    };
                    Some(MessageAttachmentReplica {
                        source_hint,
                        replica_id: object
                            .get("replica_id")
                            .and_then(JsonValue::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToString::to_string),
                        storage_family,
                        replica_role,
                        acquisition_mode,
                        locator,
                        status: object
                            .get("status")
                            .and_then(JsonValue::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToString::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let storage = match default_storage {
        MessageAttachmentStorage::Inline
            if (path.is_some() || blob_locator.is_some()) && data_url.is_none() =>
        {
            MessageAttachmentStorage::External
        }
        other => other,
    };
    let kind = object
        .get("kind")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(match storage {
            MessageAttachmentStorage::Inline => "image",
            MessageAttachmentStorage::External => "document",
        })
        .to_string();
    let name = object
        .get("name")
        .or_else(|| object.get("filename"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let mime_type = object
        .get("mime_type")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let status = object
        .get("status")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let id = object
        .get("id")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Some(MessageAttachment {
        id,
        kind,
        name,
        mime_type,
        data_url,
        path,
        blob_storage_family,
        blob_locator,
        blob_content_digest,
        blob_size_bytes,
        replicas,
        status,
        storage,
    })
}


