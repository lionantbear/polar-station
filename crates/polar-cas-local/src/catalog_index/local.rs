//! Local admission facts share the same partition records and publication head
//! as selected Store facts. No exported inventory or second aggregate index.
use super::{persistent, CatalogSnapshot, UnifiedCatalogIndex};
use crate::shared_index::partitions::ValueChange;
use polar_cas::{
    shared::{IndexMaintenanceReceipt, LocatedContent, PhysicalCandidate, SharedIndexGeneration},
    Error, ObjectId, Result,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub(super) fn coverage_packs(
    coverage: &SharedIndexGeneration,
) -> Result<BTreeMap<String, polar_cas::ContentRef>> {
    if !matches!(coverage.version, 0 | 4)
        || !coverage.runs.is_empty()
        || !coverage.loose_runs.is_empty()
        || coverage.covered.len() > 100_000
        || coverage.covered_loose.len() > 100_000
    {
        return Err(Error::Integrity);
    }
    let mut packs = BTreeMap::new();
    for reference in &coverage.covered {
        reference.index.require_sha256()?;
        polar_cas::native_fs::ErofsPackBinding::native(reference.pack.clone())?;
        if packs
            .insert(reference.pack.id.to_string(), reference.pack.clone())
            .is_some()
        {
            return Err(Error::Integrity);
        }
    }
    let mut loose = std::collections::BTreeSet::new();
    for id in &coverage.covered_loose {
        id.require_sha256()?;
        if !loose.insert(id.to_string()) {
            return Err(Error::Integrity);
        }
    }
    Ok(packs)
}
pub(super) fn validate(
    row: &LocatedContent,
    packs: &BTreeMap<String, polar_cas::ContentRef>,
    has_loose: bool,
) -> Result<()> {
    if let PhysicalCandidate::PackMember { pack } = &row.location {
        polar_cas::native_fs::ErofsPackBinding::native(pack.clone())?;
        if packs.get(&pack.id.to_string()) != Some(pack) || row.content.id == pack.id {
            return Err(Error::Integrity);
        }
    } else if !has_loose {
        return Err(Error::Integrity);
    }
    Ok(())
}
fn cancelled(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Relaxed) {
        Err(Error::Io(std::io::ErrorKind::Interrupted.into()))
    } else {
        Ok(())
    }
}
impl UnifiedCatalogIndex {
    pub(crate) fn publish_local(
        &self,
        expected: &CatalogSnapshot,
        coverage: SharedIndexGeneration,
        rows: Vec<LocatedContent>,
        reset: bool,
        stop: &AtomicBool,
    ) -> Result<IndexMaintenanceReceipt> {
        let _publication = self.publication_guard()?;
        let old = &expected.0;
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        cancelled(stop)?;
        let packs = coverage_packs(&coverage)?;
        let mut touched = BTreeMap::new();
        if reset {
            if let Some(index) = &old.index {
                index.visit_records(|row| {
                    cancelled(stop)?;
                    // The shared snapshot also contains writer reverse buckets.
                    // Physical rebuild owns only the local part of position rows.
                    if row.key.starts_with(super::writers::PREFIX) {
                        return Ok(());
                    }
                    let mut positions = persistent::decode(&row.value)?;
                    if !positions.local.is_empty() {
                        positions.local.clear();
                        touched.insert(row.locator.to_string(), (row.locator, positions));
                    }
                    Ok(())
                })?;
            }
        }
        for row in rows {
            cancelled(stop)?;
            validate(&row, &packs, !coverage.covered_loose.is_empty())?;
            let key = row.content.id.to_string();
            if !touched.contains_key(&key) {
                let mut positions = persistent::positions(old, &row.content.id)?;
                if reset {
                    positions.local.clear();
                }
                touched.insert(key.clone(), (row.content.id.clone(), positions));
            }
            let positions = &mut touched.get_mut(&key).ok_or(Error::Integrity)?.1;
            if positions.local.iter().any(|old| old.content != row.content) {
                return Err(Error::ClaimMismatch);
            }
            if !positions.local.contains(&row) {
                positions.local.push(row);
            }
        }
        let mut changes = Vec::<ValueChange>::with_capacity(touched.len());
        for (_, (id, mut positions)) in touched {
            // Stable preferred local location; keep every alternative for fallback.
            positions
                .local
                .sort_unstable_by(|a, b| match (&a.location, &b.location) {
                    (PhysicalCandidate::Loose, PhysicalCandidate::Loose) => {
                        std::cmp::Ordering::Equal
                    }
                    (PhysicalCandidate::Loose, _) => std::cmp::Ordering::Less,
                    (_, PhysicalCandidate::Loose) => std::cmp::Ordering::Greater,
                    (
                        PhysicalCandidate::PackMember { pack: a },
                        PhysicalCandidate::PackMember { pack: b },
                    ) => a.id.digest_bytes().cmp(b.id.digest_bytes()),
                });
            changes.push(persistent::change(id, positions)?);
        }
        let mut next = (**old).clone();
        next.local = coverage;
        let index = persistent::publish_values(
            storage,
            old.index.as_deref(),
            &changes,
            persistent::metadata(&next)?,
        )?;
        let mut receipt = IndexMaintenanceReceipt {
            covered_packs: next.local.covered.len() as u64,
            segments: index.partition_refs().len() as u64,
            ..Default::default()
        };
        let mut written_files = BTreeMap::new();
        for (prefix, partition) in index.partition_refs() {
            if old
                .index
                .as_ref()
                .and_then(|i| i.partition_refs().get(prefix))
                != Some(partition)
            {
                receipt.rows_written += partition.records as u64;
                let file = partition.portable_file();
                written_files.insert(file.id.to_string(), file.size_bytes);
                if partition.storage.is_none() && partition.native != partition.facts {
                    written_files
                        .insert(partition.native.id.to_string(), partition.native.size_bytes);
                }
            }
        }
        receipt.bytes_written = written_files.values().sum::<u64>() + index.reference().size_bytes;
        next.index = Some(Arc::new(index));
        cancelled(stop)?;
        ;
        self.commit(old, next)?;
        ;
        Ok(receipt)
    }
}
impl CatalogSnapshot {
    pub(crate) fn visit_local(
        &self,
        mut visit: impl FnMut(LocatedContent) -> Result<()>,
    ) -> Result<()> {
        if let Some(index) = &self.0.index {
            index.visit_records(|row| {
                if !row.key.starts_with(super::writers::PREFIX) {
                    for position in persistent::decode(&row.value)?.local {
                        visit(position)?;
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(crate) fn local_coverage(&self) -> &SharedIndexGeneration {
        &self.0.local
    }
    pub fn root(&self) -> Option<&polar_cas::ContentRef> {
        self.0.index.as_ref().map(|i| i.reference())
    }
    pub fn partition_refs(&self) -> BTreeMap<u8, crate::shared_index::partitions::PartitionRef> {
        self.0
            .index
            .as_ref()
            .map(|i| i.partition_refs().clone())
            .unwrap_or_default()
    }
    pub fn partition_count(&self) -> usize {
        self.0
            .index
            .as_ref()
            .map_or(0, |i| i.partition_refs().len())
    }
    pub fn local_candidates(&self, id: &ObjectId) -> Result<Vec<LocatedContent>> {
        match self
            .0
            .index
            .as_ref()
            .map(|index| persistent::record(index, id))
            .transpose()?
            .flatten()
        {
            Some(bytes) => persistent::local(&bytes),
            None => Ok(Vec::new()),
        }
    }
    #[inline(always)]
    pub fn locate_local(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        let Some(index) = &self.1 else {
            return Ok(None);
        };
        index.locate_local(id, persistent::local_record)
    }

    /// Batched metadata lookup against the same pinned partition set.
    pub fn locate_local_batch(&self, ids: &[ObjectId]) -> Result<Vec<Option<LocatedContent>>> {
        let mut out = vec![None; ids.len().min(1024)];
        self.locate_local_batch_into(ids, &mut out)?;
        Ok(out)
    }
    pub fn locate_local_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if ids.len() > 1024 || out.len() != ids.len() {
            return Err(Error::TooLarge);
        }
        let Some(index) = &self.1 else {
            out.fill(None);
            return Ok(());
        };
        if ids.is_empty() {
            return Ok(());
        }
        index.locate_local_batch_into(ids, out, persistent::local_record)?;
        Ok(())
    }
}
