use crate::{invalid, IdeaTree, Result};
use polar_cas::ContentRef;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeFormat {
    Itree,
    Etree,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Reference {
    Local {
        node: String,
    },
    Object {
        #[serde(deserialize_with = "canonical_uuid")]
        uuid: Uuid,
    },
    Tree {
        format: TreeFormat,
        content: ContentRef,
        node: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_uuid"
        )]
        uuid: Option<Uuid>,
    },
    Content {
        content: ContentRef,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_optional"
        )]
        media_type: Option<String>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRef {
    #[schemars(schema_with = "crate::documents::content_ref_schema")]
    pub document: ContentRef,
    pub node: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_uuid"
    )]
    #[schemars(with = "String")]
    pub expected_uuid: Option<Uuid>,
}
impl NodeRef {
    /// Caller supplies an already verified document identity. Check EVERY
    /// selector/UUID assertion even when the document is cached by hash.
    pub fn check_target(&self, actual: &ContentRef, tree: &IdeaTree) -> Result<()> {
        if &self.document != actual {
            return Err(crate::Error::IdentityMismatch);
        }
        let node = tree
            .node(&self.node)
            .ok_or_else(|| invalid(&self.node, "missing node"))?;
        if self.expected_uuid.is_some() && self.expected_uuid != node.uuid() {
            return Err(crate::Error::IdentityMismatch);
        }
        Ok(())
    }
}
pub(crate) fn valid_key(key: &str) -> bool {
    let b = key.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}
pub(crate) fn valid_type(kind: &str) -> bool {
    if kind.len() > 160 || !kind.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
        return false;
    }
    let Some((prefix, version)) = kind.rsplit_once(".v") else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
        && version
            .as_bytes()
            .first()
            .is_some_and(|c| matches!(c, b'1'..=b'9'))
        && version.bytes().all(|c| c.is_ascii_digit())
}

// Missing optional fields and explicit null are not interchangeable in v1.
pub(crate) fn present_optional<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}
pub(crate) fn canonical_uuid<'de, D>(d: D) -> std::result::Result<Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(d)?;
    let id = Uuid::parse_str(&text).map_err(serde::de::Error::custom)?;
    if id.hyphenated().to_string() != text {
        return Err(serde::de::Error::custom(
            "UUID must be lowercase hyphenated form",
        ));
    }
    Ok(id)
}
pub(crate) fn optional_uuid<'de, D>(d: D) -> std::result::Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    canonical_uuid(d).map(Some)
}
