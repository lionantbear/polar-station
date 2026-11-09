//! Identity presentation is independent of message transport and thread.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

/// Hydrated display facts, independent of transport and storage records.
#[derive(Default)]
pub struct AuthorFacts<'a> {
    pub kind: &'a str,
    pub human_label: Option<&'a str>,
    pub inhouse_name: Option<&'a str>,
    pub sender_inhouse_name: Option<&'a str>,
    pub inhouse_id: Option<&'a str>,
    pub sender_inhouse_id: Option<&'a str>,
    pub node_name: Option<&'a str>,
    pub node_id: &'a str,
}

pub fn author_label<'a>(facts: &AuthorFacts<'a>) -> &'a str {
    let label = match facts.kind {
        "inhouse_polar" => facts.inhouse_name.or(facts.sender_inhouse_name)
            .or(facts.inhouse_id).or(facts.sender_inhouse_id),
        "local_human" | "telegram_user" | "external_connector_user" => facts.human_label,
        _ => facts.human_label.or(facts.sender_inhouse_name),
    };
    label.or(facts.node_name).unwrap_or(facts.node_id)
}

/// Older message views use the same identity rules as newly projected views.
pub fn message_view_author(view: &serde_json::Value) -> (String, AuthorPresentation) {
    let kind = view["author_kind"].as_str().unwrap_or("unknown_legacy");
    let node = view["sender_node_id_hex"].as_str().unwrap_or("");
    let label = view["presentation"]["author_label"].as_str().unwrap_or_else(|| {
        author_label(&AuthorFacts {
            kind,
            human_label: view["by_human_label"].as_str(),
            inhouse_name: view["by_inhouse_polar_name"].as_str(),
            sender_inhouse_name: view["sender_inhouse_polar_name"].as_str(),
            inhouse_id: view["by_inhouse_polar_id"].as_str(),
            sender_inhouse_id: view["sender_inhouse_polar_id"].as_str(),
            node_name: view["sender_node_name"].as_str(),
            node_id: if node.is_empty() { "Unknown" } else { node },
        })
    });
    let presentation = serde_json::from_value(view["presentation"]["author"].clone())
        .unwrap_or_else(|_| author_presentation(kind, node, view["author_ref"].as_str(), label));
    (label.to_owned(), presentation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorPresentation {
    pub badge: String,
    pub role: String,
    /// ANSI theme palette entry; native themes own its actual RGB value.
    pub color: Option<u8>,
}

pub fn author_presentation(kind: &str, node: &str, reference: Option<&str>, label: &str) -> AuthorPresentation {
    let role = match kind {
        "local_human" | "telegram_user" | "external_connector_user" => "User",
        "inhouse_polar" => "Polar",
        "system_runtime" => "System",
        "remote_node" => "Node",
        _ => "",
    };
    let badge = label.trim().graphemes(true).filter(|s| !s.chars().all(char::is_whitespace))
        .take(2).collect::<String>().to_uppercase();
    let reference = reference.filter(|s| !s.is_empty());
    let color = reference.filter(|_| !node.is_empty() && !role.is_empty()).map(|reference| {
        let mut hash = Sha256::new();
        for part in [kind, node, reference] { hash.update((part.len() as u64).to_le_bytes()); hash.update(part.as_bytes()); }
        1 + hash.finalize()[0] % 6
    });
    AuthorPresentation { badge: if badge.is_empty() { "?".into() } else { badge }, role: role.into(), color }
}


