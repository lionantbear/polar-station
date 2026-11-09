//! File discovery over existing admitted content and directory metadata.
//! These are recorded names, not paths to scan or permission grants.
use crate::{ContentRef, Error, ObjectId, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FileMatch {
    /// Only valid together with an explicit writer filter.
    All,
    Name(String),
    Key(String),
    Digest(ObjectId),
}

impl FileMatch {
    pub fn parse(query: &str) -> Result<Self> {
        let query = query.trim();
        if query.is_empty() || query.len() > 1024 || query.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "expected a filename or content digest".into(),
            ));
        }
        if query.starts_with("manifest:") {
            return Ok(Self::Key(query.to_owned()));
        }
        if query.starts_with("sha256:") || query.starts_with("blake3:") {
            return Ok(Self::Digest(ObjectId::try_from(query.to_owned())?));
        }
        if query.len() == 64 && query.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Self::Digest(ObjectId::try_from(format!(
                "sha256:{}",
                query.to_ascii_lowercase()
            ))?));
        }
        Ok(Self::Name(query.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileQuery {
    /// Independent native catalog epoch. Not a writer filter or an access grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_snapshot: Option<CatalogSnapshotToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::WriterId>,
    /// Preserve this root across writer-filtered pages. None starts a new query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContentRef>,
    pub matching: FileMatch,
    pub after: Option<String>,
    pub limit: u32,
}
impl FileQuery {
    pub fn validate(&self) -> Result<()> {
        if let Some(token) = &self.catalog_snapshot { token.validate()?; }
        if matches!(self.matching, FileMatch::All) && self.writer.is_none()
            || self.writer.is_none() && self.snapshot.is_some()
            || self.writer.is_some() && self.after.is_some() && self.snapshot.is_none()
        {
            return Err(Error::Invalid(
                "writer pages require a filter and fixed snapshot".into(),
            ));
        }
        if let Some(snapshot) = &self.snapshot {
            snapshot.id.require_sha256()?;
        }
        if !(1..=200).contains(&self.limit)
            || self
                .after
                .as_ref()
                .is_some_and(|v| v.len() > 8192 || v.contains('\0'))
        {
            return Err(Error::Invalid("invalid file query page".into()));
        }
        if let FileMatch::Name(name) | FileMatch::Key(name) = &self.matching {
            if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
                return Err(Error::Invalid("invalid filename query".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<crate::catalog::writers::WriterRecord>,
    /// Stable result ordering key; never a storage locator.
    pub key: String,
    pub name: Option<String>,
    pub content: ContentRef,
    /// Directory in which this name was recorded. None for a digest-only hit.
    pub directory: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContentRef>,
    pub files: Vec<CatalogFile>,
    pub next: Option<String>,
}

/// A SQL metadata view marker, never an authorization grant or storage locator.
/// The owner incarnation changes on independent reopen; clones share it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMetadataToken {
    pub catalog_id: String,
    pub incarnation: [u8; 32],
    pub revision: u64,
}
impl FileMetadataToken {
    pub fn validate(&self) -> Result<()> {
        crate::source::LocationId::new(self.catalog_id.clone())?;
        if self.revision > i64::MAX as u64 {
            return Err(Error::Invalid("invalid file metadata revision".into()));
        }
        Ok(())
    }
}

/// Opt-in fenced current-view page. Preserve `metadata` on every continuation.
/// Legacy FilePage remains current-view; this does not fence merged domains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FencedFilePage {
    pub catalog_snapshot: CatalogSnapshotToken,
    pub page: FilePage,
    pub metadata: FileMetadataToken,
}

/// Complete native head identity, distinct from SQL revision and retained writer root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSnapshotToken {
    #[serde(deserialize_with = "required_catalog_root")]
    pub root: Option<ContentRef>,
}
fn required_catalog_root<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<ContentRef>, D::Error> {
    Option::<ContentRef>::deserialize(deserializer)
}
impl CatalogSnapshotToken {
    pub fn validate(&self) -> Result<()> {
        if let Some(root) = &self.root {
            root.id.require_sha256()?;
            if root.size_bytes == 0 { return Err(Error::Invalid("invalid native catalog root".into())); }
        }
        Ok(())
    }
}


