//! PtrHash adapter. We reuse its construction, navigation, and serialization;
//! the CAS-owned record/ordering/dictionary envelope has an explicit version.
use super::*;
use epserde::prelude::*;
use memmap2::{Mmap, MmapMut};
use std::io::{Seek, SeekFrom};
const HEADER: usize = 128;
const WIDTH: usize = 64;
use crate::shared_index::navigation::{identity, Key, Phf};
use crate::shared_index::position::{self, ALGORITHM, PRESENT};
use ptr_hash::{
    hash::{FxHash, KeyHasher},
    PtrHashParams,
};
// Keep this type at its original path: epserde TypeHash includes module_path!().
// Both record codecs share it; relocating it would invalidate existing indexes.
#[derive(Clone, Epserde)]
#[epserde(deep_copy)]
pub(in crate::shared_index) struct DiskHasher;
impl KeyHasher<Key> for DiskHasher {
    type H = u64;
    #[inline(always)]
    fn hash(key: &Key, seed: u64) -> u64 {
        <FxHash as KeyHasher<Key>>::hash(key, seed)
    }
}
fn number(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("bounded field"))
}
pub(super) struct PerfectIndex {
    pub id: ObjectId,
    pub count: u64,
    bytes: Mmap,
    slots: usize,
    order: usize,
    dictionary: usize,
    packs: usize,
    ph: Phf,
}
impl PerfectIndex {
    pub fn open(file: &File, expected: Option<&ObjectId>) -> Result<Self> {
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| Error::TooLarge)?;
        if len < HEADER + 32 {
            return Err(Error::Integrity);
        }
        // SAFETY: publisher-owned inode is immutable for the snapshot lifetime.
        let bytes = unsafe { Mmap::map(file)? };
        if &bytes[..8] != b"PCASPHF1"
            || number(&bytes, 8) != WIDTH as u64
            || bytes[104..HEADER].iter().any(|b| *b != 0)
        {
            return Err(Error::UnsupportedCapability);
        }
        let count = number(&bytes, 16);
        let slots = number(&bytes, 24);
        let order = number(&bytes, 32);
        let dictionary = number(&bytes, 40);
        let packs = number(&bytes, 48);
        let navigation = number(&bytes, 56);
        let nav_len = number(&bytes, 64);
        if count == 0
            || count > slots
            || packs > count
            || slots > u32::MAX as u64
            || order != HEADER as u64 + slots * WIDTH as u64
            || dictionary != order + count * 4
            || navigation
                != (dictionary + packs.checked_mul(40).ok_or(Error::TooLarge)? + 127) & !127
            || navigation
                .checked_add(nav_len)
                .and_then(|n| n.checked_add(32))
                != Some(len as u64)
        {
            return Err(Error::Integrity);
        }
        let id = ObjectId::from_sha256(bytes[len - 32..].try_into().map_err(|_| Error::Integrity)?);
        let nav_bytes = &bytes[navigation as usize..len - 32];
        if expected.is_some_and(|want| *want != id)
            || ObjectId::for_bytes(nav_bytes).digest_bytes() != &bytes[72..104]
        {
            return Err(Error::Integrity);
        }
        // SAFETY: this trusted publisher's typed locator is independently checksummed
        // before epserde constructs any unchecked navigation. Type/layout/platform
        // are checked by epserde. Position rows remain lazy mmap pages; full file
        // verification belongs to audit, as in the grouped index format.
        let ph = unsafe {
            Phf::deserialize_full(&mut std::io::Cursor::new(
                &bytes[navigation as usize..len - 32],
            ))
        }
        .map_err(|e| Error::Backend(format!("PHF format: {e}")))?;
        if ph.n() != count as usize || ph.max_index() != slots as usize {
            return Err(Error::Integrity);
        }
        Ok(Self {
            id,
            count,
            bytes,
            slots: slots as usize,
            order: order as usize,
            dictionary: dictionary as usize,
            packs: packs as usize,
            ph,
        })
    }
    pub fn file_bytes(&self) -> usize {
        self.bytes.len()
    }
    fn slot_for_row(&self, row: u64) -> Result<usize> {
        if row >= self.count {
            return Err(Error::Integrity);
        }
        let at = self.order + row as usize * 4;
        let slot = u32::from_le_bytes(
            self.bytes[at..at + 4]
                .try_into()
                .map_err(|_| Error::Integrity)?,
        ) as usize;
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        Ok(slot)
    }
    fn slot(&self, slot: usize) -> Result<Option<LocatedContent>> {
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        let at = HEADER + slot * WIDTH;
        let location = number(&self.bytes, at + 40);
        if location & PRESENT == 0 {
            return Ok(None);
        }
        let content = ContentRef {
            id: ObjectId::from_digest(
                if location & ALGORITHM == 0 {
                    DigestAlgorithm::Sha256
                } else {
                    DigestAlgorithm::Blake3
                },
                self.bytes[at..at + 32]
                    .try_into()
                    .map_err(|_| Error::Integrity)?,
            ),
            size_bytes: number(&self.bytes, at + 32),
        };
        position::decode(
            content.id,
            content.size_bytes,
            location & !(ALGORITHM | PRESENT),
            &self.bytes[self.dictionary..self.dictionary + self.packs * 40],
        )
        .map(Some)
    }

    fn exact(&self, slot: usize, id: &ObjectId) -> Result<Option<LocatedContent>> {
        if slot >= self.slots {
            return Err(Error::Integrity);
        }
        let at = HEADER + slot * WIDTH;
        if &self.bytes[at..at + 32] != id.digest_bytes()
            || (number(&self.bytes, at + 40) & ALGORITHM != 0)
                != (id.algorithm() == DigestAlgorithm::Blake3)
        {
            return Ok(None);
        }
        self.slot(slot)
    }
    pub fn row(&self, row: u64) -> Result<LocatedContent> {
        self.slot(self.slot_for_row(row)?)?.ok_or(Error::Integrity)
    }
    pub fn locate_row(&self, row: u64, id: &ObjectId) -> Result<Option<LocatedContent>> {
        self.exact(self.slot_for_row(row)?, id)
    }
    pub fn prefetch_row(&self, row: u64) -> Result<()> {
        self.prefetch_slot(self.slot_for_row(row)?);
        Ok(())
    }
    fn prefetch_slot(&self, slot: usize) {
        grouped::prefetch(&self.bytes, HEADER + slot * WIDTH)
    }
    pub fn locate(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        self.exact(self.ph.index(&identity(id)), id)
    }
    pub fn locate_missing_into<const W: usize>(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if W == 0 || W > 64 || ids.len() > 1024 || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        if ids.len() == 1 {
            if out[0].is_none() {
                out[0] = self.locate(&ids[0])?;
            }
            return Ok(());
        }
        if ids.len() <= 64 {
            self.fill_missing::<W, 64>(ids, out)
        } else {
            self.fill_missing::<W, 1024>(ids, out)
        }
    }
    // Keep large-batch scratch frames out of single/small-batch call stacks.
    #[inline(never)]
    fn fill_missing<const W: usize, const CAP: usize>(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        let mut positions = [0usize; CAP];
        let mut active = 0;
        let selected = ids.iter().enumerate().filter_map(|(i, id)| {
            if out[i].is_some() {
                None
            } else {
                positions[active] = i;
                active += 1;
                Some(identity(id))
            }
        });
        let mut slots = [0usize; CAP];
        let mut next = 0;
        self.ph.index_stream::<W, _>(selected).for_each(|slot| {
            self.prefetch_slot(slot);
            slots[next] = slot;
            next += 1;
        });
        for at in 0..active {
            let i = positions[at];
            out[i] = self.exact(slots[at], &ids[i])?;
        }
        Ok(())
    }
    pub fn locate_binary(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        let key = identity(id);
        let mut low = 0;
        let mut high = self.count;
        while low < high {
            let mid = low + (high - low) / 2;
            if identity(&self.row(mid)?.content.id) < key {
                low = mid + 1
            } else {
                high = mid
            }
        }
        if low == self.count {
            Ok(None)
        } else {
            self.locate_row(low, id)
        }
    }
    pub fn locate_binary_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if ids.len() > 1024 || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        for (id, hit) in ids.iter().zip(out) {
            *hit = self.locate_binary(id)?;
        }
        Ok(())
    }
    pub fn audit(&self) -> Result<()> {
        if ObjectId::for_bytes(&self.bytes[..self.bytes.len() - 32]) != self.id {
            return Err(Error::Integrity);
        }
        let mut previous = None;
        for row in 0..self.count {
            let value = self.row(row)?;
            let key = identity(&value.content.id);
            if previous.is_some_and(|p| p >= key) || self.locate(&value.content.id)? != Some(value)
            {
                return Err(Error::Integrity);
            }
            previous = Some(key);
        }
        Ok(())
    }
    pub fn build(
        file: &mut File,
        rows: impl IntoIterator<Item = Result<LocatedContent>>,
        scratch: &Path,
        stop: &AtomicBool,
    ) -> Result<(ObjectId, u64)> {
        let mut sorted = tempfile::tempfile_in(scratch)?;
        let (count, dictionary) =
            grouped::write_sorted_rows(&mut sorted, rows, SearchLayout::Compact48, stop)?;
        // SAFETY: private scratch file, no further writes during this mapping.
        let sorted_bytes = unsafe { Mmap::map(&sorted)? };
        let source_row = |row: u64| -> Result<LocatedContent> {
            let at = HEADER + row as usize * 48;
            let location = number(&sorted_bytes, at + 40);
            let content = ContentRef {
                id: ObjectId::from_digest(
                    if location & ALGORITHM == 0 {
                        DigestAlgorithm::Sha256
                    } else {
                        DigestAlgorithm::Blake3
                    },
                    sorted_bytes[at..at + 32]
                        .try_into()
                        .map_err(|_| Error::Integrity)?,
                ),
                size_bytes: number(&sorted_bytes, at + 32),
            };
            let p = location & !ALGORITHM;
            let location = if p == 0 {
                PhysicalCandidate::Loose
            } else {
                let (digest, size) = dictionary[p as usize - 1];
                PhysicalCandidate::PackMember {
                    pack: ContentRef {
                        id: ObjectId::from_sha256(digest),
                        size_bytes: size,
                    },
                }
            };
            Ok(LocatedContent { content, location })
        };
        // The upstream fast single-part navigator uses 32-bit slots. Larger
        // runs keep the wide grouped format instead of truncating row IDs.
        if count == 0 || count >= u32::MAX as u64 / 2 {
            return GroupedIndex::build(
                file,
                (0..count).map(source_row),
                SearchLayout::Aligned64,
                stop,
            );
        }
        let keys: Vec<_> = (0..count)
            .map(|row| source_row(row).map(|r| identity(&r.content.id)))
            .collect::<Result<_>>()?;
        cancelled(stop)?;
        let Some(ph) = Phf::try_new(&keys, PtrHashParams::default_fast()) else {
            return GroupedIndex::build(
                file,
                (0..count).map(source_row),
                SearchLayout::Aligned64,
                stop,
            );
        };
        cancelled(stop)?;
        let slots = ph.max_index();
        let order = HEADER + slots * WIDTH;
        let dict = order + count as usize * 4;
        let nav = (dict + dictionary.len() * 40 + 127) & !127;
        file.set_len(0)?;
        file.set_len(nav as u64)?;
        {
            // SAFETY: uniquely mapped output, stable length until this mapping drops.
            let mut bytes = unsafe { MmapMut::map_mut(&*file)? };
            bytes[..8].copy_from_slice(b"PCASPHF1");
            for (at, value) in [
                (8, WIDTH as u64),
                (16, count),
                (24, slots as u64),
                (32, order as u64),
                (40, dict as u64),
                (48, dictionary.len() as u64),
                (56, nav as u64),
            ] {
                bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
            }
            for (row, key) in keys.iter().enumerate() {
                cancelled(stop)?;
                let slot = ph.index(key);
                let at = HEADER + slot * WIDTH;
                if bytes[at + 40..at + 48] != [0; 8] {
                    return Err(Error::Integrity);
                }
                bytes[at..at + 48]
                    .copy_from_slice(&sorted_bytes[HEADER + row * 48..HEADER + (row + 1) * 48]);
                let location = number(&bytes, at + 40) | PRESENT;
                bytes[at + 40..at + 48].copy_from_slice(&location.to_le_bytes());
                bytes[order + row * 4..order + row * 4 + 4]
                    .copy_from_slice(&(slot as u32).to_le_bytes());
            }
            for (i, (digest, size)) in dictionary.iter().enumerate() {
                let at = dict + i * 40;
                bytes[at..at + 32].copy_from_slice(digest);
                bytes[at + 32..at + 40].copy_from_slice(&size.to_le_bytes());
            }
            bytes.flush()?;
        }
        file.seek(SeekFrom::Start(nav as u64))?;
        // SAFETY: fully initialized, successfully constructed typed navigator.
        unsafe { ph.serialize(&mut *file) }
            .map_err(|e| Error::Backend(format!("serialize PHF: {e}")))?;
        let nav_len = file.stream_position()? - nav as u64;
        file.seek(SeekFrom::Start(nav as u64))?;
        let nav_digest = ObjectId::for_reader(&mut (&mut *file).take(nav_len))?;
        file.seek(SeekFrom::Start(64))?;
        file.write_all(&nav_len.to_le_bytes())?;
        file.write_all(nav_digest.digest_bytes())?;
        file.rewind()?;
        let id = ObjectId::for_reader(&mut *file)?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(id.digest_bytes())?;
        file.sync_all()?;
        Ok((id, count))
    }
}


