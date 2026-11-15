use polar_identity::{NodeId, UnixTimestamp};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum MessagePayload {
    Text { text: String },
    Json { value: serde_json::Value },
    Binary { bytes: Vec<u8> },
}

impl MessagePayload {
    pub fn family(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Json { .. } => "json",
            Self::Binary { .. } => "binary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LgpChannel {
    #[serde(with = "node_id_serde")]
    pub owner_node_id: NodeId,
    pub channel_id: String,
    #[serde(default)]
    pub attrs: ChannelAttrs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LgpThread {
    #[serde(with = "node_id_serde")]
    pub owner_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub attrs: ThreadAttrs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LgpMessage {
    #[serde(with = "node_id_serde")]
    pub owner_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    pub message_id: String,
    #[serde(with = "node_id_serde")]
    pub sender_node_id: NodeId,
    pub intent: String,
    pub payload: MessagePayload,
    #[serde(with = "unix_timestamp_serde")]
    pub created_at: UnixTimestamp,
}

mod node_id_serde {
    use polar_identity::NodeId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &NodeId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_hex())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<NodeId, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        NodeId::from_hex(&raw).map_err(serde::de::Error::custom)
    }
}

mod unix_timestamp_serde {
    use polar_identity::UnixTimestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &UnixTimestamp, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<UnixTimestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = u64::deserialize(deserializer)?;
        Ok(UnixTimestamp::from_secs(raw))
    }
}


