//! Portable Shared Catalog supplement to the existing CAS v2 backup plan.
//! A serialized manifest is not a storage capability or a native-index trust grant.
use crate::{source::LocationId, ContentRef, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedProjectionBackup {
    pub query: ContentRef,
    pub selection: ContentRef,
    pub objects: Vec<ContentRef>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedCatalogBackup {
    pub schema: String,
    pub root: ContentRef,
    /// Authorized managed root represented by this existing LocationId.
    pub source_location: LocationId,
    /// Portable facts, selected originals, domain dependencies, and physical
    /// loose/pack bytes. Foreign native accelerators are not required to reopen.
    pub files: Vec<ContentRef>,
    pub projections: BTreeMap<String, SharedProjectionBackup>,
}
impl SharedCatalogBackup {
    pub fn validate(&self, max_files: usize, max_bytes: u64) -> Result<()> {
        if self.schema != "polar.cas.shared-catalog-backup.v1" {
            return Err(Error::Invalid(
                "unsupported Shared Catalog backup schema".into(),
            ));
        }
        if self.files.len() > max_files || self.projections.len() > 256 {
            return Err(Error::TooLarge);
        }
        self.root.id.require_sha256()?;
        let mut seen = BTreeMap::new();
        let mut previous = None;
        let mut bytes = 0u64;
        for file in &self.files {
            let id = file.id.to_string();
            if file.size_bytes > i64::MAX as u64 || previous.as_ref().is_some_and(|p| p >= &id) {
                return Err(Error::Integrity);
            }
            bytes = bytes.checked_add(file.size_bytes).ok_or(Error::TooLarge)?;
            if bytes > max_bytes {
                return Err(Error::TooLarge);
            }
            previous = Some(id.clone());
            seen.insert(id, file);
        }
        if seen.get(&self.root.id.to_string()).copied() != Some(&self.root) {
            return Err(Error::Integrity);
        }
        for (name, projection) in &self.projections {
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(Error::Integrity);
            }
            projection.query.id.require_sha256()?;
            projection.selection.id.require_sha256()?;
            let mut selected = BTreeSet::new();
            for object in &projection.objects {
                if !selected.insert(object.id.to_string())
                    || seen.get(&object.id.to_string()).copied() != Some(object)
                {
                    return Err(Error::Integrity);
                }
            }
            if !projection.objects.contains(&projection.query)
                || !projection.objects.contains(&projection.selection)
            {
                return Err(Error::Integrity);
            }
        }
        Ok(())
    }
    pub fn contains(&self, content: &ContentRef) -> bool {
        self.files
            .binary_search_by_key(&content.id.to_string(), |c| c.id.to_string())
            .ok()
            .is_some_and(|i| self.files[i] == *content)
    }
}


