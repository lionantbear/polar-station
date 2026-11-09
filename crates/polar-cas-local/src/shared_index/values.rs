//! Immutable exact-key -> opaque value records. The caller owns value semantics.
//! Uses the SAME PtrHash navigator as the physical CAS index, with a different
//! record codec. Native accelerators must only be opened from trusted local
//! publication; portable records can be verified and rebuilt on another host.
use super::{
    navigation::{identity, Phf},
    position::{self, ALGORITHM, PRESENT},
};
use epserde::prelude::*;
use memmap2::{Mmap, MmapMut};
use polar_cas::shared::{LocatedContent, PhysicalCandidate};
use polar_cas::{ContentRef, DigestAlgorithm, Error, ObjectId, Result};
use ptr_hash::PtrHashParams;
use std::{
    borrow::Cow,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::Arc,
};

mod portable;

pub(super) fn portable_prefix(bytes: &[u8]) -> Result<&[u8]> {
    portable::prefix(bytes)
}
const GENERIC: u64 = 1 << 61;
const KEY_IN_VALUE: u64 = 1 << 60;
const LOCAL_ONLY: u64 = 1 << 59;
const INLINE_EXTENSION: u64 = 1 << 58;
const PACK_MASK: u64 = u32::MAX as u64;
const KEY_OFFSET_MASK: u64 = (1 << 28) - 1;
pub(crate) type LocalPosition = fn(&ValueRecord) -> Result<Option<LocatedContent>>;
const HEADER: usize = 128;
const WIDTH: usize = 64;
const MAX_BYTES: usize = 512 << 20;
const MAX_RECORDS: usize = 1_000_000;

pub type RecordView<'a> = (Cow<'a, [u8]>, Cow<'a, [u8]>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueRecord {
    /// Physical content ID, or a digest used ONLY to locate a logical key.
    pub locator: ObjectId,
    /// Complete semantic key, checked after the PHF lookup.
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

struct MappedRange {
    map: Arc<Mmap>,
    start: usize,
    end: usize,
}
impl std::ops::Deref for MappedRange {
    type Target = [u8];
    #[inline(always)]
    fn deref(&self) -> &[u8] {
        &self.map[self.start..self.end]
    }
}
pub struct ValueIndex {
    bytes: MappedRange,
    ph: Phf,
    slots: usize,
    payload: usize,
    modern: bool,
    dictionary: usize,
    packs: usize,
}
#[inline(always)]
fn word(bytes: &[u8], at: usize) -> Result<u64> {
    let raw = bytes
        .get(at..at.checked_add(8).ok_or(Error::TooLarge)?)
        .ok_or(Error::Integrity)?;
    Ok(u64::from_le_bytes(
        raw.try_into().map_err(|_| Error::Integrity)?,
    ))
}
#[inline(always)]
fn num(bytes: &[u8], at: usize) -> Result<usize> {
    usize::try_from(word(bytes, at)?).map_err(|_| Error::TooLarge)
}

fn put(bytes: &mut [u8], at: usize, value: usize) {
    bytes[at..at + 8].copy_from_slice(&(value as u64).to_le_bytes());
}
fn compact_row(bytes: &[u8], at: usize, dictionary: usize, packs: usize) -> Result<ValueRecord> {
    let flags = word(bytes, at + 40)?;
    let extended = flags & INLINE_EXTENSION != 0;
    if !matches!(&bytes[..8], b"PCASVAL4" | b"PCASVAL5")
        || (extended && &bytes[..8] != b"PCASVAL5")
        || flags & (PRESENT | LOCAL_ONLY) != PRESENT | LOCAL_ONLY
        || flags & !(PRESENT | ALGORITHM | LOCAL_ONLY | PACK_MASK | INLINE_EXTENSION) != 0
        || (!extended && (word(bytes, at + 48)? != 0 || word(bytes, at + 56)? != 0))
    {
        return Err(Error::Integrity);
    }
    let locator = ObjectId::from_digest(
        if flags & ALGORITHM == 0 {
            DigestAlgorithm::Sha256
        } else {
            DigestAlgorithm::Blake3
        },
        bytes[at..at + 32]
            .try_into()
            .map_err(|_| Error::Integrity)?,
    );
    let end = dictionary
        .checked_add(packs.checked_mul(40).ok_or(Error::Integrity)?)
        .ok_or(Error::Integrity)?;
    let position = position::decode(
        locator.clone(),
        word(bytes, at + 32)?,
        flags & PACK_MASK,
        bytes.get(dictionary..end).ok_or(Error::Integrity)?,
    )?;
    Ok(ValueRecord {
        key: identity(&locator).to_vec(),
        locator,
        value: if extended {
            position::single_local_extended(
                &position,
                &(
                    bytes[88..120].try_into().map_err(|_| Error::Integrity)?,
                    bytes[at + 48..at + 64]
                        .try_into()
                        .map_err(|_| Error::Integrity)?,
                ),
            )?
        } else {
            position::single_local_value(&position)?
        },
    })
}

impl ValueIndex {
    pub fn record_count(&self) -> usize {
        self.ph.n()
    }
    pub fn build(file: &mut File, records: &[ValueRecord]) -> Result<ContentRef> {
        Self::try_build(file, records)?
            .ok_or_else(|| Error::Backend("PtrHash construction failed".into()))
    }
    /// None means only that the optional PHF accelerator could not be built.
    /// Invalid records and storage errors must not become a fallback success.
    pub(super) fn try_build(
        file: &mut File,
        records: &[ValueRecord],
    ) -> Result<Option<ContentRef>> {
        Self::try_build_with_local(file, records, None)
    }
    pub(super) fn try_build_with_local(
        file: &mut File,
        records: &[ValueRecord],
        local: Option<LocalPosition>,
    ) -> Result<Option<ContentRef>> {
        Self::try_build_impl(file, records, local, true)
    }
    pub(super) fn try_build_staged(
        file: &mut File,
        records: &[ValueRecord],
        local: Option<LocalPosition>,
    ) -> Result<Option<ContentRef>> {
        Self::try_build_impl(file, records, local, false)
    }
    fn try_build_impl(
        file: &mut File,
        records: &[ValueRecord],
        local: Option<LocalPosition>,
        durable: bool,
    ) -> Result<Option<ContentRef>> {
        if records.is_empty() || records.len() > MAX_RECORDS {
            return Err(Error::TooLarge);
        }
        let mut keys = Vec::with_capacity(records.len());
        let mut seen = std::collections::BTreeSet::new();
        let mut size = 0usize;
        for record in records {
            let key = identity(&record.locator);
            if !seen.insert(key) {
                return Err(Error::Conflict);
            }
            keys.push(key);
            size = size
                .checked_add(record.key.len())
                .and_then(|n| n.checked_add(record.value.len()))
                .ok_or(Error::TooLarge)?;
            if size > MAX_BYTES {
                return Err(Error::TooLarge);
            }
        }
        let Some(ph) = Phf::try_new(&keys, PtrHashParams::default_fast()) else {
            return Ok(None);
        };
        let mut locations = Vec::with_capacity(records.len());
        let mut shared_keys = Vec::with_capacity(records.len());
        let mut local_only = Vec::with_capacity(records.len());
        let mut extensions = Vec::with_capacity(records.len());
        let mut context = None;
        let mut packs = std::collections::BTreeMap::<[u8; 32], (u64, usize)>::new();
        for record in records {
            let location = local.map(|decode| decode(record)).transpose()?.flatten();
            if let Some(row) = &location {
                if row.content.id != record.locator || record.key != identity(&record.locator) {
                    return Err(Error::Integrity);
                }
                if let PhysicalCandidate::PackMember { pack } = &row.location {
                    pack.id.require_sha256()?;
                    let index = packs.len() + 1;
                    let entry = packs
                        .entry(*pack.id.digest_bytes())
                        .or_insert((pack.size_bytes, index));
                    if entry.0 != pack.size_bytes {
                        return Err(Error::ClaimMismatch);
                    }
                }
            }
            let extension = location
                .as_ref()
                .map(|row| position::compact_extension(row, &record.value))
                .transpose()?
                .flatten();
            let extension = extension.filter(|(scope, _)| {
                if context.is_none() {
                    context = Some(*scope);
                }
                context == Some(*scope)
            });
            let compact = extension.is_some()
                || location
                    .as_ref()
                    .map(position::single_local_value)
                    .transpose()?
                    .is_some_and(|value| value == record.value);
            let shared_key = location.as_ref().filter(|_| !compact).and_then(|_| {
                record.value[..record.value.len().min(128)]
                    .windows(record.key.len())
                    .position(|bytes| bytes == record.key)
            });
            if compact {
                size -= record.key.len() + record.value.len();
            } else if shared_key.is_some() {
                size -= record.key.len();
            }
            extensions.push(extension);
            local_only.push(compact);
            shared_keys.push(shared_key);
            locations.push(location);
        }
        let slots = ph.max_index();
        let payload = HEADER + slots * WIDTH;
        let dictionary = payload.checked_add(size).ok_or(Error::TooLarge)?;
        let navigation = (dictionary
            .checked_add(packs.len() * 40)
            .ok_or(Error::TooLarge)?
            + 127)
            & !127;
        if navigation > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        file.set_len(0)?;
        file.set_len(navigation as u64)?;
        {
            // SAFETY: exclusive output inode, stable size while mapped.
            let mut bytes = unsafe { MmapMut::map_mut(&*file)? };
            bytes[..8].copy_from_slice(if context.is_some() {
                b"PCASVAL5"
            } else {
                b"PCASVAL4"
            });
            if let Some(context) = context {
                bytes[88..120].copy_from_slice(&context);
            }
            for (at, n) in [
                (8, records.len()),
                (16, slots),
                (24, payload),
                (32, navigation),
                (72, dictionary),
                (80, packs.len()),
            ] {
                put(&mut bytes, at, n);
            }
            let mut offset = payload;
            for (((((record, key), local), shared_key), compact), extension) in records
                .iter()
                .zip(&keys)
                .zip(&locations)
                .zip(&shared_keys)
                .zip(&local_only)
                .zip(&extensions)
            {
                let slot = HEADER + ph.index(key) * WIDTH;
                if word(&bytes, slot + 40)? != 0 {
                    return Err(Error::Integrity);
                }
                bytes[slot..slot + 32].copy_from_slice(&key[..32]);
                let mut flags = PRESENT | if key[32] == 1 { ALGORITHM } else { 0 };
                if let Some(row) = local {
                    bytes[slot + 32..slot + 40]
                        .copy_from_slice(&row.content.size_bytes.to_le_bytes());
                    if let PhysicalCandidate::PackMember { pack } = &row.location {
                        flags |= packs[pack.id.digest_bytes()].1 as u64;
                    }
                } else {
                    flags |= GENERIC;
                    put(&mut bytes, slot + 32, record.key.len());
                }
                if *compact {
                    flags |= LOCAL_ONLY;
                }
                if let Some(key_offset) = shared_key {
                    flags |= KEY_IN_VALUE | ((*key_offset as u64) << 32);
                }
                if extension.is_some() {
                    flags |= INLINE_EXTENSION;
                }
                bytes[slot + 40..slot + 48].copy_from_slice(&flags.to_le_bytes());
                put(&mut bytes, slot + 48, if *compact { 0 } else { offset });
                put(
                    &mut bytes,
                    slot + 56,
                    if *compact { 0 } else { record.value.len() },
                );
                if *compact {
                    if let Some((_, value)) = extension {
                        bytes[slot + 48..slot + 64].copy_from_slice(value);
                    }
                    continue;
                }
                if shared_key.is_none() {
                    let end = offset + record.key.len();
                    bytes[offset..end].copy_from_slice(&record.key);
                    offset = end;
                }
                let end = offset + record.value.len();
                bytes[offset..end].copy_from_slice(&record.value);
                offset = end;
            }
            for (id, (size, index)) in &packs {
                let at = dictionary + (index - 1) * 40;
                bytes[at..at + 32].copy_from_slice(id);
                bytes[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
            }
            if durable {
                bytes.flush()?;
            }
        }
        file.seek(SeekFrom::Start(navigation as u64))?;
        // SAFETY: initialized upstream navigator, native artifact only.
        unsafe { ph.serialize(&mut *file) }
            .map_err(|e| Error::Backend(format!("serialize PHF: {e}")))?;
        let end = file.stream_position()?;
        file.seek(SeekFrom::Start(navigation as u64))?;
        let digest = ObjectId::for_reader(&mut (&mut *file).take(end - navigation as u64))?;
        file.seek(SeekFrom::Start(40))?;
        file.write_all(digest.digest_bytes())?;
        if durable {
            file.sync_all()?;
        }
        file.rewind()?;
        Ok(Some(ContentRef {
            id: ObjectId::for_reader(&mut *file)?,
            size_bytes: end,
        }))
    }

    /// Only managed immutable local artifacts. Remote exchange uses portable
    /// records below, never unchecked epserde input from an external Store.
    pub fn open(file: &File) -> Result<Self> {
        Self::open_range(file, 0, file.metadata()?.len())
    }
    /// A native index embedded in an immutable shared segment. memmap2 keeps
    /// the backing inode alive and handles page alignment for the range.
    pub fn open_range(file: &File, offset: u64, length: u64) -> Result<Self> {
        if offset.checked_add(length).ok_or(Error::Integrity)? > file.metadata()?.len() {
            return Err(Error::Integrity);
        }
        let len = usize::try_from(length).map_err(|_| Error::TooLarge)?;
        if !(HEADER..=MAX_BYTES + (64 << 20)).contains(&len) {
            return Err(Error::TooLarge);
        }
        // SAFETY: caller owns immutable publisher inode for the snapshot lifetime.
        let bytes = unsafe {
            memmap2::MmapOptions::new()
                .offset(offset)
                .len(len)
                .map(file)?
        };
        Self::open_mapped(Arc::new(bytes), 0, length)
    }
    pub(super) fn open_mapped(map: Arc<Mmap>, offset: u64, length: u64) -> Result<Self> {
        let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
        let len = usize::try_from(length).map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(len).ok_or(Error::Integrity)?;
        if end > map.len() || !(HEADER..=MAX_BYTES + (64 << 20)).contains(&len) {
            return Err(Error::Integrity);
        }
        let bytes = MappedRange { map, start, end };
        let modern = matches!(
            &bytes[..8],
            b"PCASVAL2" | b"PCASVAL3" | b"PCASVAL4" | b"PCASVAL5"
        );
        if (!modern && &bytes[..8] != b"PCASVAL1")
            || bytes[if &bytes[..8] == b"PCASVAL5" {
                120
            } else if modern {
                88
            } else {
                72
            }..HEADER]
                .iter()
                .any(|b| *b != 0)
        {
            return Err(Error::Integrity);
        }
        let count = num(&bytes, 8)?;
        let slots = num(&bytes, 16)?;
        let payload = num(&bytes, 24)?;
        let navigation = num(&bytes, 32)?;
        if count == 0
            || count > MAX_RECORDS
            || count > slots
            || slots > MAX_RECORDS * 2
            || payload != HEADER + slots * WIDTH
            || payload > navigation
            || navigation >= len
        {
            return Err(Error::Integrity);
        }
        let dictionary = if modern { num(&bytes, 72)? } else { navigation };
        let packs = if modern { num(&bytes, 80)? } else { 0 };
        if dictionary < payload
            || dictionary > navigation
            || packs > count
            || dictionary
                .checked_add(packs.checked_mul(40).ok_or(Error::Integrity)?)
                .is_none_or(|end| end > navigation)
        {
            return Err(Error::Integrity);
        }
        let nav = &bytes[navigation..];
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let ph = super::navigation::open(nav)?;
        #[cfg(any(test, feature = "query-metrics"))]
        super::partitions::write_metrics::record("nav_decode", started.elapsed(), nav.len() as u64, 0);
        if ph.n() != count || ph.max_index() != slots {
            return Err(Error::Integrity);
        }
        Ok(Self {
            bytes,
            ph,
            slots,
            payload,
            modern,
            dictionary,
            packs,
        })
    }
    pub(super) fn row_at(&self, slot: usize, locator: &ObjectId) -> Result<Option<RecordView<'_>>> {
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        let at = HEADER + slot * WIDTH;
        let (offset, len) = if self.modern {
            let flags = word(&self.bytes, at + 40)?;
            if flags & PRESENT == 0
                || self.bytes[at..at + 32] != *locator.digest_bytes()
                || (flags & ALGORITHM != 0) != (locator.algorithm() == DigestAlgorithm::Blake3)
            {
                return Ok(None);
            }
            if flags & LOCAL_ONLY != 0 && flags & KEY_IN_VALUE == 0 {
                let row = compact_row(&self.bytes, at, self.dictionary, self.packs)?;
                return Ok(Some((Cow::Owned(row.key), Cow::Owned(row.value))));
            }
            (
                num(&self.bytes, at + 48)?,
                if flags & GENERIC == 0 {
                    33
                } else {
                    num(&self.bytes, at + 32)?
                },
            )
        } else {
            if self.bytes[at + 33] == 0 || self.bytes[at..at + 33] != identity(locator) {
                return Ok(None);
            }
            (num(&self.bytes, at + 40)?, num(&self.bytes, at + 48)?)
        };
        let value_len = num(&self.bytes, at + 56)?;
        if self.modern && word(&self.bytes, at + 40)? & KEY_IN_VALUE != 0 {
            let flags = word(&self.bytes, at + 40)?;
            if !matches!(&self.bytes[..8], b"PCASVAL3" | b"PCASVAL4" | b"PCASVAL5")
                || flags & GENERIC != 0
            {
                return Err(Error::Integrity);
            }
            let key_offset = ((flags >> 32) & KEY_OFFSET_MASK) as usize;
            let key_start = offset.checked_add(key_offset).ok_or(Error::Integrity)?;
            let key_end = key_start.checked_add(33).ok_or(Error::Integrity)?;
            let end = offset.checked_add(value_len).ok_or(Error::Integrity)?;
            if offset < self.payload || end > self.dictionary || key_end > end {
                return Err(Error::Integrity);
            }
            let key = &self.bytes[key_start..key_end];
            if key != identity(locator) {
                return Err(Error::Integrity);
            }
            return Ok(Some((
                Cow::Borrowed(key),
                Cow::Borrowed(&self.bytes[offset..end]),
            )));
        }
        let end = offset.checked_add(len).ok_or(Error::Integrity)?;
        let value_end = end.checked_add(value_len).ok_or(Error::Integrity)?;
        if offset < self.payload || value_end > self.dictionary {
            return Err(Error::Integrity);
        }
        Ok(Some((
            Cow::Borrowed(&self.bytes[offset..end]),
            Cow::Borrowed(&self.bytes[end..value_end]),
        )))
    }
    /// False requests the legacy/generic record fallback. Otherwise the caller's
    /// result slot is filled directly, including a definitive miss.
    #[inline(always)]
    pub(super) fn local_into(
        &self,
        slot: usize,
        id: &ObjectId,
        out: &mut Option<LocatedContent>,
    ) -> Result<bool> {
        if !self.modern {
            return Ok(false);
        }
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        let at = HEADER + slot * WIDTH;
        let flags = word(&self.bytes, at + 40)?;
        if flags & PRESENT == 0
            || self.bytes[at..at + 32] != *id.digest_bytes()
            || (flags & ALGORITHM != 0) != (id.algorithm() == DigestAlgorithm::Blake3)
        {
            *out = None;
            return Ok(true);
        }
        if flags & GENERIC != 0 {
            return Ok(false);
        }
        *out = Some(position::decode(
            id.clone(),
            word(&self.bytes, at + 32)?,
            // All supported navigators bound dictionaries by MAX_RECORDS,
            // well below 2^32. Shared-key metadata never affects this lookup.
            flags & PACK_MASK,
            &self.bytes[self.dictionary..self.dictionary + self.packs * 40],
        )?);
        Ok(true)
    }
    
    fn exact(&self, slot: usize, locator: &ObjectId, key: &[u8]) -> Result<Option<Cow<'_, [u8]>>> {
        Ok(self
            .row_at(slot, locator)?
            .and_then(|(stored, value)| (stored.as_ref() == key).then_some(value)))
    }
    /// Returns the complete semantic key for the caller to validate. The digest
    /// is only navigation, never a substitute for that exact comparison.
    #[inline(always)]
    pub(super) fn probe(&self, locator: &ObjectId) -> usize {
        self.ph.index(&identity(locator))
    }
    #[inline(always)]
    pub(super) fn prefetch_probe(&self, locator: &ObjectId) -> usize {
        let slot = self.probe(locator);
        super::search::grouped::prefetch(&self.bytes, HEADER + slot * WIDTH);
        slot
    }
    
    pub(crate) fn inline_extension_at(
        &self,
        slot: usize,
        id: &ObjectId,
        context: &[u8; 32],
    ) -> Result<Option<Option<&[u8]>>> {
        if !self.modern {
            return Ok(None);
        }
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        let at = HEADER + slot * WIDTH;
        let flags = word(&self.bytes, at + 40)?;
        if flags & PRESENT == 0
            || self.bytes[at..at + 32] != *id.digest_bytes()
            || (flags & ALGORITHM != 0) != (id.algorithm() == DigestAlgorithm::Blake3)
        {
            return Ok(Some(None));
        }
        if flags & LOCAL_ONLY == 0 || flags & KEY_IN_VALUE != 0 {
            return Ok(None);
        }
        if flags & INLINE_EXTENSION == 0 {
            return Ok(Some(None));
        }
        if &self.bytes[..8] != b"PCASVAL5"
            || flags & !(PRESENT | ALGORITHM | LOCAL_ONLY | INLINE_EXTENSION | PACK_MASK) != 0
        {
            return Err(Error::Integrity);
        }
        Ok(Some(
            (self.bytes[88..120] == *context).then_some(&self.bytes[at + 48..at + 64]),
        ))
    }
    pub fn row(&self, locator: &ObjectId) -> Result<Option<RecordView<'_>>> {
        self.row_at(self.ph.index(&identity(locator)), locator)
    }
    pub fn get(&self, locator: &ObjectId, key: &[u8]) -> Result<Option<Cow<'_, [u8]>>> {
        self.exact(self.ph.index(&identity(locator)), locator, key)
    }
    pub fn get_batch<'a>(
        &'a self,
        queries: &[(&ObjectId, &[u8])],
    ) -> Result<Vec<Option<Cow<'a, [u8]>>>> {
        let mut out = vec![None; queries.len().min(1024)];
        self.get_batch_into(queries, &mut out)?;
        Ok(out)
    }
    pub fn get_batch_into<'a>(
        &'a self,
        queries: &[(&ObjectId, &[u8])],
        out: &mut [Option<Cow<'a, [u8]>>],
    ) -> Result<()> {
        if queries.len() > 1024 || out.len() != queries.len() {
            return Err(Error::TooLarge);
        }
        let mut slots = [0; 1024];
        self.ph
            .index_stream::<32, _>(queries.iter().map(|(id, _)| identity(id)))
            .enumerate()
            .for_each(|(i, slot)| {
                super::search::grouped::prefetch(&self.bytes, HEADER + slot * WIDTH);
                slots[i] = slot;
            });
        for (((id, key), slot), result) in queries.iter().zip(slots).zip(out) {
            *result = self.exact(slot, id, key)?;
        }
        Ok(())
    }
}

/// Canonical, architecture-independent facts. No native pointers or PHF state.
pub fn encode_records(records: &[ValueRecord]) -> Result<Vec<u8>> {
    if records.len() > MAX_RECORDS {
        return Err(Error::TooLarge);
    }
    let mut sorted: Vec<_> = records.iter().collect();
    sorted.sort_unstable_by_key(|r| identity(&r.locator));
    let mut bytes = b"PCASREC1".to_vec();
    bytes.extend_from_slice(&(records.len() as u64).to_le_bytes());
    let mut previous = None;
    for record in sorted {
        let key = identity(&record.locator);
        if previous == Some(key) {
            return Err(Error::Conflict);
        }
        previous = Some(key);
        let end = bytes
            .len()
            .checked_add(49)
            .and_then(|n| n.checked_add(record.key.len()))
            .and_then(|n| n.checked_add(record.value.len()))
            .ok_or(Error::TooLarge)?;
        if end > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&(record.key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(record.value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&record.key);
        bytes.extend_from_slice(&record.value);
    }
    Ok(bytes)
}
pub fn decode_records(bytes: &[u8]) -> Result<Vec<ValueRecord>> {
    if bytes.starts_with(b"PCASVAL2")
        || bytes.starts_with(b"PCASVAL3")
        || bytes.starts_with(b"PCASVAL4")
        || bytes.starts_with(b"PCASVAL5")
    {
        return portable::decode(bytes);
    }
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    if bytes.get(..8) != Some(b"PCASREC1") {
        return Err(Error::Integrity);
    }
    let count = num(bytes, 8)?;
    if count > MAX_RECORDS || count > bytes.len().saturating_sub(16) / 49 {
        return Err(Error::TooLarge);
    }
    let mut records = Vec::with_capacity(count);
    let mut at = 16;
    let mut previous = None;
    for _ in 0..count {
        let raw = bytes.get(at..at + 33).ok_or(Error::Integrity)?;
        let key: [u8; 33] = raw.try_into().map_err(|_| Error::Integrity)?;
        if previous.is_some_and(|p| p >= key) {
            return Err(Error::Integrity);
        }
        previous = Some(key);
        let algorithm = match key[32] {
            0 => DigestAlgorithm::Sha256,
            1 => DigestAlgorithm::Blake3,
            _ => return Err(Error::Integrity),
        };
        let locator = ObjectId::from_digest(
            algorithm,
            key[..32].try_into().map_err(|_| Error::Integrity)?,
        );
        let key_len = num(bytes, at + 33)?;
        let value_len = num(bytes, at + 41)?;
        at += 49;
        let end = at.checked_add(key_len).ok_or(Error::Integrity)?;
        let key = bytes.get(at..end).ok_or(Error::Integrity)?.to_vec();
        at = end;
        let end = at.checked_add(value_len).ok_or(Error::Integrity)?;
        let value = bytes.get(at..end).ok_or(Error::Integrity)?.to_vec();
        at = end;
        records.push(ValueRecord {
            locator,
            key,
            value,
        });
    }
    if at != bytes.len() {
        return Err(Error::Integrity);
    }
    Ok(records)
}






