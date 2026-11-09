//! Opaque scoped values in the SAME physical content row. Domain semantics and
//! UUID assignment remain with the projection owner.
use super::*;
use crate::shared_index::partitions::ValueChange;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedExtension {
    pub context: [u8; 32],
    pub value: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ExtensionUpdate {
    pub content: ObjectId,
    pub context: [u8; 32],
    /// None removes only this context, never content or another context.
    pub value: Option<Vec<u8>>,
}

pub(super) fn changes(state: &State, updates: &[ExtensionUpdate]) -> Result<Vec<ValueChange>> {
    if updates.len() > 1024 {
        return Err(Error::TooLarge);
    }
    let mut bytes = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = BTreeMap::new();
    for update in updates {
        if !seen.insert((persistent::key(&update.content), update.context)) {
            return Err(Error::Conflict);
        }
        bytes = bytes
            .checked_add(65 + update.value.as_ref().map_or(0, Vec::len))
            .ok_or(Error::TooLarge)?;
        if bytes > 8 << 20 || update.value.as_ref().is_some_and(Vec::is_empty) {
            return Err(Error::TooLarge);
        }
        let row_key = persistent::key(&update.content);
        if let std::collections::btree_map::Entry::Vacant(entry) = rows.entry(row_key) {
            entry.insert((
                update.content.clone(),
                persistent::positions(state, &update.content)?,
            ));
        }
        let (_, row) = rows.get_mut(&row_key).ok_or(Error::Integrity)?;
        row.extensions.retain(|e| e.context != update.context);
        if let Some(value) = &update.value {
            row.extensions.push(ScopedExtension {
                context: update.context,
                value: value.clone(),
            });
        }
        row.extensions.sort_by_key(|e| e.context);
        if row.extensions.len() > 1024 {
            return Err(Error::TooLarge);
        }
    }
    let mut out = vec![];
    let mut output_bytes = 0usize;
    for (_, (id, row)) in rows {
        let change = persistent::change(id, row)?;
        if let ValueChange::Put(value) = &change {
            output_bytes = output_bytes
                .checked_add(value.key.len() + value.value.len())
                .ok_or(Error::TooLarge)?;
        }
        if output_bytes > 8 << 20 {
            return Err(Error::TooLarge);
        }
        out.push(change);
    }
    Ok(out)
}

impl CatalogSnapshot {
    /// Exact scoped association bytes in this pinned root. No location decode,
    /// source open, download, or independent content navigator.
    pub fn content_extension(
        &self,
        id: &ObjectId,
        context: &[u8; 32],
    ) -> Result<Option<std::borrow::Cow<'_, [u8]>>> {
        let Some(index) = &self.0.index else {
            return Ok(None);
        };
        index.scoped_extension(id, context, persistent::extension)
    }
}



impl CatalogSnapshot {
    /// Prepare immutable extensions for a caller-owned atomic root. Reuses the
    /// pinned values directly; no reopening, record enumeration or head change.
    pub fn prepare_extensions(
        &self,
        storage: &crate::LocalCas,
        updates: &[ExtensionUpdate],
    ) -> Result<Self> {
        let changes = changes(&self.0, updates)?;
        let mut next = self.0.as_ref().clone();
        next.index = Some(Arc::new(super::persistent::publish_values(
            storage,
            self.0.index.as_deref(),
            &changes,
            super::persistent::metadata(&next)?,
        )?));
        Ok(Self::new(Arc::new(next)))
    }
}
