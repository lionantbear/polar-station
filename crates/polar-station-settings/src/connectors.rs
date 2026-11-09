//! Station routing configuration; provider identities reuse the shared contract.
use crate::ValidationError;
pub use polar_connector_contract::ConnectorProviderKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageConnectorSettingsV1 {
    pub revision: u64,
    pub connectors: Vec<MessageConnectorV1>,
    pub routes: Vec<CodexMessageRouteV1>,
    pub disabled_monitors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageConnectorV1 {
    pub id: String,
    pub host_id: String,
    #[serde(with = "provider_name")]
    pub provider: ConnectorProviderKind,
    /// None is local. Remote references use existing scoped Host connections.
    pub connection_ref: Option<String>,
    pub enabled: bool,
    /// Serving Host only; remote callers cannot choose an arbitrary recipient.
    pub allowed_source_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexMessageRouteV1 {
    pub endpoint_id: String,
    pub thread_id: Option<String>,
    pub connector_id: String,
    pub destination_ref: String,
    pub enabled: bool,
}

mod provider_name {
    use super::ConnectorProviderKind;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &ConnectorProviderKind,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ConnectorProviderKind, D::Error> {
        let value = String::deserialize(d)?;
        ConnectorProviderKind::parse(&value)
            .ok_or_else(|| serde::de::Error::custom("unknown connector provider"))
    }
}

impl MessageConnectorSettingsV1 {
    pub fn validate(&self) -> Result<(), ValidationError> {
        fn id(v: &str) -> Result<(), ValidationError> {
            if v.is_empty() || v.len() > 256 || v.chars().any(char::is_control) {
                Err(ValidationError::InvalidText("connector identifier"))
            } else {
                Ok(())
            }
        }
        if self.connectors.len() > 64
            || self.routes.len() > 256
            || self.disabled_monitors.len() > 64
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 192 * 1024)
        {
            return Err(ValidationError::InvalidText("connector settings limit"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for c in &self.connectors {
            id(&c.id)?;
            id(&c.host_id)?;
            if !ids.insert(c.id.as_str())
                || !matches!(
                    c.provider,
                    ConnectorProviderKind::Telegram | ConnectorProviderKind::Imessage
                )
            {
                return Err(ValidationError::InvalidText(
                    "unsupported or duplicate connector",
                ));
            }
            if let Some(r) = &c.connection_ref {
                id(r)?;
            }
            if c.allowed_source_hosts.len() > 64 {
                return Err(ValidationError::InvalidText("connector sources limit"));
            }
            for source in &c.allowed_source_hosts {
                id(source)?;
            }
        }
        let mut routes = std::collections::BTreeSet::new();
        for r in &self.routes {
            id(&r.endpoint_id)?;
            id(&r.connector_id)?;
            id(&r.destination_ref)?;
            if let Some(t) = &r.thread_id {
                id(t)?;
            }
            if !ids.contains(r.connector_id.as_str())
                || !routes.insert((
                    &r.endpoint_id,
                    &r.thread_id,
                    &r.connector_id,
                    &r.destination_ref,
                ))
            {
                return Err(ValidationError::InvalidText(
                    "unknown connector or duplicate route",
                ));
            }
        }
        for endpoint in &self.disabled_monitors {
            id(endpoint)?;
        }
        Ok(())
    }

    pub fn routes_for<'a>(&'a self, endpoint: &str, thread: &str) -> Vec<&'a CodexMessageRouteV1> {
        self.routes
            .iter()
            .filter(|r| {
                r.endpoint_id == endpoint
                    && r.enabled
                    && (r.thread_id.as_deref() == Some(thread)
                        || (r.thread_id.is_none()
                            && !self.routes.iter().any(|specific| {
                                specific.endpoint_id == endpoint
                                    && specific.thread_id.as_deref() == Some(thread)
                                    && specific.connector_id == r.connector_id
                                    && specific.destination_ref == r.destination_ref
                            })))
            })
            .collect()
    }
}
