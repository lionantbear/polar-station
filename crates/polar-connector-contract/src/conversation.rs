//! Original Connector history/event contract shared by HTTP and product adapters.
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventCursor(pub String);
impl EventCursor {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl From<&str> for EventCursor {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}
impl From<String> for EventCursor {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorThreadEventProjection {
    pub cursor: EventCursor,
    pub event: String,
    pub thread_handle: String,
    pub occurred_at_secs: u64,
    pub body: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorThreadHistoryView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<EventCursor>,
    pub items: Vec<ConnectorThreadEventProjection>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorThreadHistoryPage {
    #[default]
    Latest,
    Oldest,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ConnectorThreadMessagesQuery {
    pub after: Option<String>,
    pub before: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    #[serde(default)]
    pub page: ConnectorThreadHistoryPage,
}

impl ConnectorThreadMessagesQuery {
    pub fn new(after: Option<String>, cursor: Option<String>, limit: Option<u32>) -> Self {
        Self {
            after,
            before: None,
            cursor,
            limit,
            page: ConnectorThreadHistoryPage::Latest,
        }
    }
}
