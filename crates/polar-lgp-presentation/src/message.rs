use polar_lgp_model::{LgpMessageKey, MessagePayload};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Projection of hydrated message facts; never opens storage or resolves access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagePresentation {
    pub key: LgpMessageKey,
    pub thread_id: String,
    pub author_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<crate::AuthorPresentation>,
    pub created_at_secs: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<polar_edge_presentation::ContentDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fallback: Option<String>,
    pub attachments: Vec<Value>,
    pub reply_to: Option<LgpMessageKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal: Option<CausalPresentation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalPresentation {
    pub key: LgpMessageKey,
    pub kind: String,
    pub label: String,
}

pub fn causal_presentation(kind: Option<&str>, key: Option<LgpMessageKey>) -> Option<CausalPresentation> {
    Some(CausalPresentation {
        key: key?,
        kind: kind?.to_owned(),
        label: causal_label(kind).to_owned(),
    })
}

/// Compatibility at the shared presentation boundary, not in each renderer.
/// A canonical presentation always wins, including an intentionally empty body.
pub fn message_view_text(view: &Value) -> String {
    if let Some(text) = view["presentation"]["text"].as_str() { return text.to_owned(); }
    let payload = &view["payload"];
    if let Ok(payload) = serde_json::from_value::<MessagePayload>(payload.clone()) {
        return message_text(&payload);
    }
    // Existing local-chat fixtures/older projections omit the family tag.
    if let Some(text) = payload["text"].as_str() { return text.to_owned(); }
    if let Some(value) = payload.get("value") {
        return message_text(&MessagePayload::Json { value: value.clone() });
    }
    String::new()
}

pub fn message_view_attachments(view: &Value) -> Vec<Value> {
    if let Some(rows) = view["presentation"]["attachments"].as_array() { return rows.clone(); }
    let payload = &view["payload"];
    if let Ok(payload) = serde_json::from_value::<MessagePayload>(payload.clone()) {
        return message_attachments(&payload);
    }
    payload.get("value").map(|value| message_attachments(&MessagePayload::Json { value: value.clone() })).unwrap_or_default()
}

pub fn content_fallback_label(reason: &str) -> &'static str {
    match reason {
        "source_budget" => "message exceeds rich-text size limit",
        "structure_budget" => "message exceeds rich-text structure limit",
        "empty_projection" => "no formatted content available",
        _ => "rich content unavailable",
    }
}

pub fn message_text(payload: &MessagePayload) -> String {
    match payload {
        MessagePayload::Text { text } => text.clone(),
        MessagePayload::Binary { .. } => "[Binary content — open attachment]".into(),
        MessagePayload::Json { value } => {
            for field in ["text", "summary_text", "message", "task", "content"] {
                if let Some(text) = value.get(field).and_then(Value::as_str) {
                    return text.into();
                }
            }
            value
                .pointer("/body/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
        }
    }
}

pub fn message_attachments(payload: &MessagePayload) -> Vec<Value> {
    let MessagePayload::Json { value } = payload else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for (field, storage) in [
        ("attachments", None),
        ("inline_attachments", Some("inline")),
        ("external_attachments", Some("external")),
    ] {
        if let Some(attachments) = value.get(field).and_then(Value::as_array) {
            for attachment in attachments {
                let Some(mut object) = attachment.as_object().cloned() else {
                    continue;
                };
                if let Some(storage) = storage {
                    object.insert("storage".into(), storage.into());
                }
                rows.push(Value::Object(object));
            }
        }
    }
    rows
}

/// Derive content after the caller has hydrated/authorized the message facts.
pub fn message_content(
    text: &str,
) -> (
    Option<polar_edge_presentation::ContentDocument>,
    Option<String>,
) {
    if text.trim().is_empty() { return (None, None); }
    match polar_edge_presentation::parse_content(
        text,
        polar_edge_presentation::SourceFormat::Markdown,
    ) {
        Ok(doc) => (Some(doc), None),
        Err(error) => (
            None,
            Some(
                match error {
                    polar_edge_presentation::ProjectionError::EmptySource => "empty_source",
                    polar_edge_presentation::ProjectionError::SourceTooLarge { .. } => {
                        "source_budget"
                    }
                    polar_edge_presentation::ProjectionError::ContentLimit => "structure_budget",
                    polar_edge_presentation::ProjectionError::EmptyProjection => "empty_projection",
                }
                .into(),
            ),
        ),
    }
}



/// Human-readable relation copy shared by message readers. Preserve the
/// canonical causal kind; derived/copied content is not relabelled as a reply.
pub fn causal_label(kind: Option<&str>) -> &'static str {
    use polar_lgp_model::ThreadMessageCausalKind as Kind;
    match kind.and_then(Kind::parse) {
        Some(Kind::Reply | Kind::InhouseReply) => "Reply to",
        Some(Kind::SystemDerived) => "Derived from",
        Some(Kind::ChatBridgeDelivery) => "Copied from",
        Some(Kind::LegacyBackfill) | None => "Linked to",
    }
}

