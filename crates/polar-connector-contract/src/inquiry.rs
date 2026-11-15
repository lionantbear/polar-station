//! Original Connector Inquiry response shared by transport and product adapters.
use polar_inquiry_graph_model::*;
use serde::Serialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectorInquirySessionView {
    pub session_handle: String,
    pub package_ref: InquiryGraphPackageRef,
    #[serde(default)]
    pub package_metadata: JsonValue,
    /// The connector may return the persisted session so browser clients can
    /// resume an in-progress activation without inventing a new session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<InquiryGraphSessionState>,
    pub revision: u64,
    pub status: String,
    pub projection: InquiryGraphSessionProjection,
    #[serde(default)]
    pub questions: Vec<InquiryGraphNode>,
    #[serde(default)]
    pub batch: Vec<InquiryGraphNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<InquiryGraphResultArtifact>,
}
