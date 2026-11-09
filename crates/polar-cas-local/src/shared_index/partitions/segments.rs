//! Physical packing of existing binary partitions. Prepare the final layout
//! before publishing segments; maintenance never rebuilds unchanged navigators.
use super::*;
const TARGET: usize = 64 << 20;

enum PreparedBytes {
    Mapped(Arc<memmap2::Mmap>),
    Owned(Vec<u8>),
}
impl std::ops::Deref for PreparedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(m) => m,
            Self::Owned(v) => v,
        }
    }
}
struct Prepared {
    reference: PartitionRef,
    bytes: PreparedBytes,
    // Keep preparation paths visible to peak-space accounting until consumed.
    _temporary: Option<tempfile::NamedTempFile>,
    start: usize,
    len: usize,
    native_delta: u64,
    rows: Option<Vec<ValueRecord>>,
}
impl Prepared {
    fn existing(p: &Partition) -> Result<Self> {
        let storage = p.reference.storage.as_ref().ok_or(Error::Integrity)?;
        Ok(Self {
            reference: p.reference.clone(),
            bytes: PreparedBytes::Mapped(p.facts.clone()),
            _temporary: None,
            start: usize::try_from(storage.facts_offset).map_err(|_| Error::TooLarge)?,
            len: stored_len(&p.reference)?,
            native_delta: storage
                .native_offset
                .checked_sub(storage.facts_offset)
                .ok_or(Error::Integrity)?,
            rows: match &p.index {
                PartitionIndex::Sorted(rows) => Some(rows.clone()),
                PartitionIndex::Perfect(_) => None,
            },
        })
    }
    fn perfect(&self) -> bool {
        self.reference.native != self.reference.facts
    }
}
fn stored_len(reference: &PartitionRef) -> Result<usize> {
    let storage = reference.storage.as_ref().ok_or(Error::Integrity)?;
    let length = if reference.native != reference.facts {
        storage
            .native_offset
            .checked_sub(storage.facts_offset)
            .and_then(|n| n.checked_add(reference.native.size_bytes))
            .ok_or(Error::Integrity)?
    } else {
        reference.facts.size_bytes
    };
    usize::try_from(length).map_err(|_| Error::TooLarge)
}
fn prepare(
    store: &LocalCas,
    mut rows: Vec<ValueRecord>,
    local: Option<LocalPosition>,
) -> Result<Prepared> {
    
    let mut prepared = None;
    let mut temporary = None;
    let native = if rows.len() > SMALL_PARTITION_RECORDS {
        let mut file = store.index_temporary()?;
        let native = ValueIndex::try_build_staged(file.as_file_mut(), &rows, local)?;
        
        if native.is_some() {
            // Private immutable preparation mapping; no root can reference it.
            prepared = Some(PreparedBytes::Mapped(Arc::new(unsafe {
                memmap2::Mmap::map(file.as_file())?
            })));
            temporary = Some(file);
        }
        native
    } else {
        None
    };
    let bytes = match prepared {
        Some(bytes) => bytes,
        None => PreparedBytes::Owned(encode_records(&rows)?),
    };
    let facts_bytes: &[u8] = if native.is_some() {
        super::super::values::portable_prefix(&bytes)?
    } else {
        &bytes
    };
    let facts = ContentRef {
        id: ObjectId::for_bytes(facts_bytes),
        size_bytes: facts_bytes.len() as u64,
    };
    let records = rows.len();
    
    let sorted = if native.is_none() {
        rows.sort_unstable_by_key(|r| identity(&r.locator));
        Some(rows)
    } else {
        None
    };
    Ok(Prepared {
        reference: PartitionRef {
            platform: if native.is_some() {
                platform()
            } else {
                SORTED_PLATFORM.into()
            },
            native: native.unwrap_or_else(|| facts.clone()),
            facts,
            records,
            storage: None,
        },
        start: 0,
        len: bytes.len(),
        bytes,
        _temporary: temporary,
        native_delta: 0,
        rows: sorted,
    })
}
// Identical next-fit/alignment rules for layout prediction and actual emission.
fn groups(items: impl Iterator<Item = (u8, usize, bool)>) -> Vec<(u64, Vec<u8>)> {
    let mut groups = vec![];
    let mut size = 8usize;
    let mut prefixes = vec![];
    for (prefix, len, perfect) in items {
        let aligned = if perfect { (size + 127) & !127 } else { size };
        if !prefixes.is_empty() && aligned + len > TARGET {
            groups.push((size as u64, std::mem::take(&mut prefixes)));
            size = 8;
        }
        if perfect {
            size = (size + 127) & !127;
        }
        size += len;
        prefixes.push(prefix);
    }
    if !prefixes.is_empty() {
        groups.push((size as u64, prefixes));
    }
    groups
}

/// Changed partitions are prepared once. Include old ranges selected for packing
/// before any physical segment is published, so no intermediate segment is
/// published and then immediately copied by compaction in this batch.
pub(super) fn update(
    store: &LocalCas,
    publication: &mut crate::index_publication::IndexPublication<'_>,
    partitions: &mut BTreeMap<u8, Arc<Partition>>,
    rebuilt: BTreeMap<u8, Vec<ValueRecord>>,
    local: Option<LocalPosition>,
    reclaim: bool,
) -> Result<()> {
    for prefix in rebuilt.keys() {
        partitions.remove(prefix);
    }
    if partitions.is_empty() {
        // Initial/full replacement builds have no old ranges to compact. Keep
        // their existing streaming memory/disk bound instead of staging all leaves.
        partitions.extend(emit(
            publication,
            rebuilt
                .into_iter()
                .map(|(prefix, rows)| prepare(store, rows, local).map(|p| (prefix, p))),
        )?);
        return Ok(());
    }
    let mut prepared = BTreeMap::new();
    for (prefix, rows) in rebuilt {
        partitions.remove(&prefix);
        prepared.insert(prefix, prepare(store, rows, local)?);
    }
    
    let mut old = BTreeMap::<String, (u64, u64, Vec<u8>)>::new();
    for (prefix, p) in partitions.iter() {
        let Some(s) = &p.reference.storage else {
            continue;
        };
        let group =
            old.entry(s.container.id.to_string())
                .or_insert((s.container.size_bytes, 8, vec![]));
        group.1 += stored_len(&p.reference)? as u64;
        group.2.push(*prefix);
    }
    let new_groups = groups(prepared.iter().map(|(p, v)| (*p, v.len, v.perfect())));
    let mut small = BTreeSet::new();
    let mut small_files = 0;
    for (size, prefixes) in old
        .values()
        .map(|(s, _, ps)| (*s, ps))
        .chain(new_groups.iter().map(|(s, ps)| (*s, ps)))
    {
        if size < TARGET as u64 {
            small_files += 1;
            small.extend(prefixes.iter().copied());
        }
    }
    let mut sizes = BTreeMap::new();
    for prefix in &small {
        if let Some(p) = prepared.get(prefix) {
            sizes.insert(*prefix, (p.len, p.perfect()));
        } else {
            let p = &partitions[prefix];
            sizes.insert(
                *prefix,
                (
                    stored_len(&p.reference)?,
                    p.reference.native != p.reference.facts,
                ),
            );
        }
    }
    let merged_count = groups(sizes.into_iter().map(|(p, (n, perfect))| (p, n, perfect))).len();
    let merge_small = small_files > 4 && merged_count < small_files;
    for (size, live, prefixes) in old.into_values() {
        let dead_divisor = if size < (TARGET / 2) as u64 { 5 } else { 4 };
        let reclaimable = size.saturating_sub(live);
        if (merge_small && size < TARGET as u64)
            || (reclaim && reclaimable >= 4096 && live <= size - size / dead_divisor)
        {
            for prefix in prefixes {
                
                prepared.insert(prefix, Prepared::existing(&partitions[&prefix])?);
            }
        }
    }
    partitions.extend(emit(publication, prepared.into_iter().map(Ok))?);
    
    Ok(())
}
fn emit(
    publication: &mut crate::index_publication::IndexPublication<'_>,
    prepared: impl Iterator<Item = Result<(u8, Prepared)>>,
) -> Result<BTreeMap<u8, Arc<Partition>>> {
    let mut bytes = b"PCASSEG1".to_vec();
    let mut pending = Vec::new();
    let mut out = BTreeMap::new();
    for entry in prepared {
        let (prefix, p) = entry?;
        let aligned = if p.perfect() {
            (bytes.len() + 127) & !127
        } else {
            bytes.len()
        };
        if !pending.is_empty() && aligned + p.len > TARGET {
            flush(publication, &mut bytes, &mut pending, &mut out)?;
        }
        if p.perfect() {
            bytes.resize((bytes.len() + 127) & !127, 0);
        }
        let facts_offset = bytes.len() as u64;
        let stored = p
            .bytes
            .get(p.start..p.start.checked_add(p.len).ok_or(Error::Integrity)?)
            .ok_or(Error::Integrity)?;
        bytes.extend_from_slice(stored);
        pending.push(Pending {
            prefix,
            reference: p.reference,
            rows: p.rows,
            facts_offset,
            native_offset: facts_offset + p.native_delta,
        });
    }
    if !pending.is_empty() {
        flush(publication, &mut bytes, &mut pending, &mut out)?;
    }
    Ok(out)
}

struct Pending {
    prefix: u8,
    reference: PartitionRef,
    rows: Option<Vec<ValueRecord>>,
    facts_offset: u64,
    native_offset: u64,
}
pub(super) fn build(
    store: &LocalCas,
    publication: &mut crate::index_publication::IndexPublication<'_>,
    rebuilt: BTreeMap<u8, Vec<ValueRecord>>,
    local: Option<LocalPosition>,
) -> Result<BTreeMap<u8, Arc<Partition>>> {
    let mut partitions = BTreeMap::new();
    update(store, publication, &mut partitions, rebuilt, local, false)?;
    Ok(partitions)
}
fn flush(
    publication: &mut crate::index_publication::IndexPublication<'_>,
    bytes: &mut Vec<u8>,
    pending: &mut Vec<Pending>,
    out: &mut BTreeMap<u8, Arc<Partition>>,
) -> Result<()> {
    let container = publication.publish(bytes)?;
    let file = publication.open_staged(&container)?;
    // SAFETY: the segment was atomically published and is immutable.
    let facts = Arc::new(unsafe { memmap2::Mmap::map(&file)? });
    for mut p in pending.drain(..) {
        p.reference.storage = Some(PartitionStorage {
            container: container.clone(),
            facts_offset: p.facts_offset,
            native_offset: p.native_offset,
        });
        let index = match p.rows {
            Some(rows) => PartitionIndex::Sorted(rows),
            None => PartitionIndex::Perfect(Arc::new(ValueIndex::open_mapped(
                facts.clone(),
                p.native_offset,
                p.reference.native.size_bytes,
            )?)),
        };
        out.insert(
            p.prefix,
            Arc::new(Partition {
                reference: p.reference,
                index,
                facts: facts.clone(),
            }),
        );
    }
    bytes.clear();
    bytes.extend_from_slice(b"PCASSEG1");
    Ok(())
}
