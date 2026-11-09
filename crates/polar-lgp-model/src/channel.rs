use polar_identity::{NodeId, UnixTimestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    DirectMessage,
    BroadcastList,
    PolarChannel,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TransportPlane {
    #[default]
    Member,
    ControlBus,
    Custom(String),
}

impl TransportPlane {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Member => "member",
            Self::ControlBus => "control_bus",
            Self::Custom(value) => value.as_str(),
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let value = raw.trim();
        match value {
            "" => None,
            "member" => Some(Self::Member),
            "control_bus" => Some(Self::ControlBus),
            _ => Some(Self::Custom(value.to_string())),
        }
    }

    pub fn is_member(&self) -> bool {
        matches!(self, Self::Member)
    }

    pub fn is_control_bus(&self) -> bool {
        matches!(self, Self::ControlBus)
    }

    pub fn is_configured_plane_id(&self) -> bool {
        match self {
            Self::Member | Self::ControlBus => true,
            Self::Custom(value) => value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
        }
    }

    pub fn display_label(&self) -> String {
        self.as_str().to_string()
    }

    pub fn member() -> Self {
        Self::Member
    }

    pub fn control_bus() -> Self {
        Self::ControlBus
    }
}

impl From<&str> for TransportPlane {
    fn from(value: &str) -> Self {
        Self::parse(value).unwrap_or(Self::Member)
    }
}

impl From<String> for TransportPlane {
    fn from(value: String) -> Self {
        Self::parse(&value).unwrap_or(Self::Member)
    }
}

impl std::fmt::Display for TransportPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Access-plane identifier used by LGP channel/thread policy surfaces.
///
/// The underlying enum is kept as `TransportPlane` for compatibility while
/// public channel/thread fields are named `access_plane` to reflect that this
/// scope carries policy and authorization semantics, not only transport.
pub type AccessPlaneId = TransportPlane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRecord {
    pub owner_id: NodeId,
    pub channel_id: String,
    pub partner_id: Option<NodeId>,
    pub partner_inhouse_polar_id: Option<String>,
    pub channel_kind: ChannelKind,
    pub access_plane: TransportPlane,
    pub title: String,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMemberRecord {
    pub owner_id: NodeId,
    pub channel_id: String,
    pub member_id: NodeId,
    pub added_at: UnixTimestamp,
}


