//! Unified immutable search: navigation never touches a payload or a directory.
//! Grouped probing and staged prefetch follow the SwissTable/F14 and DPDK bulk
//! techniques documented in design §2.4.12. This is a disk layout, not a dumped
//! Rust hash table. Rows stay sorted for streaming incremental compaction.
use super::*;
use memmap2::Mmap;
use polar_cas::shared::LocatedContent;
use std::io::{BufWriter, Seek, SeekFrom};

const MAGIC: &[u8; 8] = b"PCASLOC1";
const HEADER: usize = 128;
fn bucket_width(count: u64) -> usize {
    if count <= u32::MAX as u64 {
        64
    } else {
        128
    }
}
const SLOTS: usize = 8;
const ALGORITHM: u64 = 1 << 63;
const MAX_BATCH: usize = 1024;

/// Alternative row sizes used by the fixed format/algorithm benchmark.
#[derive(Clone, Copy, Debug)]
pub enum SearchLayout {
    Compact48,
    Aligned64,
}
impl SearchLayout {
    fn width(self) -> usize {
        match self {
            Self::Compact48 => 48,
            Self::Aligned64 => 64,
        }
    }
}

pub(super) struct GroupedIndex {
    bytes: Mmap,
    pub id: ObjectId,
    pub count: u64,
    width: usize,
    buckets: usize,
    navigation: usize,
    dictionary: usize,
    packs: usize,
    ranges: [u64; 257],
}

fn algorithm(id: &ObjectId) -> u64 {
    match id.algorithm() {
        DigestAlgorithm::Sha256 => 0,
        DigestAlgorithm::Blake3 => ALGORITHM,
    }
}
fn key(row: &LocatedContent) -> ([u8; 32], u64) {
    (*row.content.id.digest_bytes(), algorithm(&row.content.id))
}
fn number(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("validated record boundary"),
    )
}
fn hash(id: &ObjectId) -> u64 {
    // Content identities already have cryptographic entropy. Mixing the prefix
    // only chooses a bucket; collisions still compare the complete identity.
    let mut h =
        u64::from_le_bytes(id.digest_bytes()[..8].try_into().expect("digest")) ^ algorithm(id);
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}
fn tag(h: u64) -> u16 {
    ((h >> 48) as u16) | 1
}

#[inline]
fn masks(bytes: &[u8], wanted: u16) -> (u32, u32) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::*;
        // SAFETY: the header establishes every bucket's 16 readable tag bytes;
        // SSE2 is guaranteed on x86_64. No aligned load requirement.
        unsafe {
            let tags = _mm_loadu_si128(bytes.as_ptr().cast());
            let hits =
                _mm_movemask_epi8(_mm_cmpeq_epi16(tags, _mm_set1_epi16(wanted as i16))) as u32;
            let empty = _mm_movemask_epi8(_mm_cmpeq_epi16(tags, _mm_setzero_si128())) as u32;
            (hits, empty)
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let mut hits = 0;
        let mut empty = 0;
        for slot in 0..SLOTS {
            let t = u16::from_le_bytes(bytes[slot * 2..slot * 2 + 2].try_into().expect("tag"));
            if t == wanted {
                hits |= 3 << (slot * 2);
            }
            if t == 0 {
                empty |= 3 << (slot * 2);
            }
        }
        (hits, empty)
    }
}
#[inline]
pub(in crate::shared_index) fn prefetch(bytes: &[u8], at: usize) {
    if at >= bytes.len() {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: address is within the mapped index and prefetch does not dereference
    // a Rust reference or extend its lifetime.
    unsafe {
        std::arch::x86_64::_mm_prefetch(
            bytes.as_ptr().add(at).cast(),
            std::arch::x86_64::_MM_HINT_T0,
        );
    }
}

impl GroupedIndex {
    /// Only publisher-owned, immutable files may be mapped. Publication never
    /// modifies or truncates an inode retained by a live snapshot.
    pub fn open(file: &File, expected: Option<&ObjectId>) -> Result<Self> {
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| Error::TooLarge)?;
        if len < HEADER + 32 {
            return Err(Error::Integrity);
        }
        // SAFETY: same immutable publisher contract as format::Index.
        let bytes = unsafe { Mmap::map(file)? };
        if &bytes[..8] != MAGIC {
            return Err(Error::UnsupportedCapability);
        }
        let width = number(&bytes, 8) as usize;
        let count = number(&bytes, 16);
        let buckets = usize::try_from(number(&bytes, 24)).map_err(|_| Error::TooLarge)?;
        let navigation = usize::try_from(number(&bytes, 32)).map_err(|_| Error::TooLarge)?;
        let dictionary = usize::try_from(number(&bytes, 40)).map_err(|_| Error::TooLarge)?;
        let packs = usize::try_from(number(&bytes, 48)).map_err(|_| Error::TooLarge)?;
        if !matches!(width, 48 | 64)
            || count > format::MAX_ROWS
            || !buckets.is_power_of_two()
            || bytes[56..HEADER].iter().any(|b| *b != 0)
        {
            return Err(Error::Integrity);
        }
        let rows_end = count
            .checked_mul(width as u64)
            .and_then(|n| n.checked_add(HEADER as u64))
            .ok_or(Error::TooLarge)?;
        let expected_nav = rows_end.checked_add(127).ok_or(Error::TooLarge)? & !127;
        let nav_end = (buckets as u64)
            .checked_mul(bucket_width(count) as u64)
            .and_then(|n| n.checked_add(expected_nav))
            .ok_or(Error::TooLarge)?;
        let end = (packs as u64)
            .checked_mul(40)
            .and_then(|n| n.checked_add(nav_end))
            .and_then(|n| n.checked_add(32))
            .ok_or(Error::TooLarge)?;
        if expected_nav != navigation as u64 || nav_end != dictionary as u64 || end != len as u64 {
            return Err(Error::Integrity);
        }
        let id = ObjectId::from_sha256(bytes[len - 32..].try_into().map_err(|_| Error::Integrity)?);
        if expected.is_some_and(|e| e != &id) {
            return Err(Error::Integrity);
        }
        let mut ranges = [0u64; 257];
        ranges[256] = count;
        for (prefix, bound) in ranges.iter_mut().enumerate().take(256).skip(1) {
            let mut low = 0;
            let mut high = count;
            while low < high {
                let mid = low + (high - low) / 2;
                if bytes[HEADER + mid as usize * width] < prefix as u8 {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            *bound = low;
        }
        Ok(Self {
            bytes,
            id,
            count,
            width,
            buckets,
            navigation,
            dictionary,
            packs,
            ranges,
        })
    }
    pub fn file_bytes(&self) -> usize {
        self.bytes.len()
    }
    fn row_offset(&self, row: u64) -> Result<usize> {
        if row >= self.count {
            return Err(Error::Integrity);
        }
        Ok(HEADER + row as usize * self.width)
    }
    fn matches(&self, row: u64, id: &ObjectId) -> Result<bool> {
        let at = self.row_offset(row)?;
        Ok(&self.bytes[at..at + 32] == id.digest_bytes()
            && number(&self.bytes, at + 40) & ALGORITHM == algorithm(id))
    }
    pub fn row(&self, row: u64) -> Result<LocatedContent> {
        let at = self.row_offset(row)?;
        let location = number(&self.bytes, at + 40);
        let kind = if location & ALGORITHM == 0 {
            DigestAlgorithm::Sha256
        } else {
            DigestAlgorithm::Blake3
        };
        let content = ContentRef {
            id: ObjectId::from_digest(
                kind,
                self.bytes[at..at + 32]
                    .try_into()
                    .map_err(|_| Error::Integrity)?,
            ),
            size_bytes: number(&self.bytes, at + 32),
        };
        let pack = location & !ALGORITHM;
        let location = if pack == 0 {
            PhysicalCandidate::Loose
        } else {
            if pack > self.packs as u64 {
                return Err(Error::Integrity);
            }
            let offset = self.dictionary + (pack as usize - 1) * 40;
            PhysicalCandidate::PackMember {
                pack: ContentRef {
                    id: ObjectId::from_sha256(
                        self.bytes[offset..offset + 32]
                            .try_into()
                            .map_err(|_| Error::Integrity)?,
                    ),
                    size_bytes: number(&self.bytes, offset + 32),
                },
            }
        };
        Ok(LocatedContent { content, location })
    }
    fn bucket(&self, bucket: usize) -> &[u8] {
        let at = self.navigation + (bucket & (self.buckets - 1)) * bucket_width(self.count);
        &self.bytes[at..at + bucket_width(self.count)]
    }
    fn prefetch_bucket(&self, bucket: usize) {
        prefetch(
            &self.bytes,
            self.navigation + (bucket & (self.buckets - 1)) * bucket_width(self.count),
        );
    }
    fn slot_row(&self, bucket: &[u8], slot: usize) -> u64 {
        if self.count <= u32::MAX as u64 {
            u32::from_le_bytes(
                bucket[16 + slot * 4..20 + slot * 4]
                    .try_into()
                    .expect("slot"),
            ) as u64
        } else {
            number(bucket, 16 + slot * 8)
        }
    }
    fn find_in_bucket(&self, id: &ObjectId, bucket: usize, mut hits: u32) -> Result<Option<u64>> {
        let bytes = self.bucket(bucket);
        while hits != 0 {
            let slot = hits.trailing_zeros() as usize / 2;
            hits &= !(3 << (slot * 2));
            let row = self.slot_row(bytes, slot);
            if self.matches(row, id)? {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
    pub fn locate(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        let h = hash(id);
        let first = h as usize & (self.buckets - 1);
        for n in 0..self.buckets {
            let bucket = first.wrapping_add(n) & (self.buckets - 1);
            let (hits, empty) = masks(self.bucket(bucket), tag(h));
            if let Some(row) = self.find_in_bucket(id, bucket, hits)? {
                return self.row(row).map(Some);
            }
            if empty != 0 {
                return Ok(None);
            }
        }
        Err(Error::Integrity)
    }
    /// Resolve a row handle against the complete identity, for alternate navigators.
    pub fn locate_row(&self, row: u64, id: &ObjectId) -> Result<Option<LocatedContent>> {
        if self.matches(row, id)? {
            self.row(row).map(Some)
        } else {
            Ok(None)
        }
    }
    pub fn prefetch_row(&self, row: u64) -> Result<()> {
        prefetch(&self.bytes, self.row_offset(row)?);
        Ok(())
    }
    /// Pure comparison-search control over the EXACT same rows and outputs.
    pub fn locate_binary(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        let prefix = id.digest_bytes()[0] as usize;
        let mut low = self.ranges[prefix];
        let mut high = self.ranges[prefix + 1];
        while low < high {
            let mid = low + (high - low) / 2;
            let at = self.row_offset(mid)?;
            let cmp = self.bytes[at..at + 32].cmp(id.digest_bytes());
            let less = cmp.is_lt()
                || (cmp.is_eq() && number(&self.bytes, at + 40) & ALGORITHM < algorithm(id));
            if less {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low < self.count && self.matches(low, id)? {
            self.row(low).map(Some)
        } else {
            Ok(None)
        }
    }
    /// Interleaved comparison-search control: independent binary-search chains
    /// share a prefetch window, while retaining the same ordered row layout.
    pub fn locate_binary_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if ids.len() > MAX_BATCH || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        out.fill(None);
        for (keys, results) in ids.chunks(32).zip(out.chunks_mut(32)) {
            let mut low = [0u64; 32];
            let mut high = [0u64; 32];
            for (i, id) in keys.iter().enumerate() {
                let prefix = id.digest_bytes()[0] as usize;
                low[i] = self.ranges[prefix];
                high[i] = self.ranges[prefix + 1];
            }
            loop {
                let mut pending = false;
                for i in 0..keys.len() {
                    if low[i] < high[i] {
                        pending = true;
                        self.prefetch_row(low[i] + (high[i] - low[i]) / 2)?;
                    }
                }
                if !pending {
                    break;
                }
                for (i, id) in keys.iter().enumerate() {
                    if low[i] >= high[i] {
                        continue;
                    }
                    let mid = low[i] + (high[i] - low[i]) / 2;
                    let at = self.row_offset(mid)?;
                    let cmp = self.bytes[at..at + 32].cmp(id.digest_bytes());
                    if cmp.is_lt()
                        || (cmp.is_eq() && number(&self.bytes, at + 40) & ALGORITHM < algorithm(id))
                    {
                        low[i] = mid + 1;
                    } else {
                        high[i] = mid;
                    }
                }
            }
            for (i, id) in keys.iter().enumerate() {
                if low[i] < self.count {
                    results[i] = self.locate_row(low[i], id)?;
                }
            }
        }
        Ok(())
    }
    /// Fill only unresolved input positions, allowing leveled runs to share the
    /// caller's key/output buffers without sorting or copying active identities.
    pub fn locate_missing_into<const W: usize>(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if W == 0 || W > 64 || ids.len() > MAX_BATCH || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        for (keys, results) in ids.chunks(W).zip(out.chunks_mut(W)) {
            let mut buckets = [0usize; W];
            let mut tags = [0u16; W];
            let mut live = [false; W];
            for (i, id) in keys.iter().enumerate() {
                if results[i].is_some() {
                    continue;
                }
                let h = hash(id);
                buckets[i] = h as usize & (self.buckets - 1);
                tags[i] = tag(h);
                live[i] = true;
                self.prefetch_bucket(buckets[i]);
            }
            for _ in 0..self.buckets {
                let mut hits = [0u32; W];
                let mut empty = [false; W];
                // All candidate rows are prefetched before any full key compare.
                for i in 0..keys.len() {
                    if !live[i] {
                        continue;
                    }
                    let (matched, free) = masks(self.bucket(buckets[i]), tags[i]);
                    hits[i] = matched;
                    empty[i] = free != 0;
                    let mut mask = matched;
                    while mask != 0 {
                        let slot = mask.trailing_zeros() as usize / 2;
                        mask &= !(3 << (slot * 2));
                        let row = self.slot_row(self.bucket(buckets[i]), slot);
                        prefetch(&self.bytes, self.row_offset(row)?);
                    }
                }
                let mut pending = false;
                for i in 0..keys.len() {
                    if !live[i] {
                        continue;
                    }
                    if let Some(row) = self.find_in_bucket(&keys[i], buckets[i], hits[i])? {
                        results[i] = Some(self.row(row)?);
                        live[i] = false;
                    } else if empty[i] {
                        live[i] = false;
                    } else {
                        buckets[i] = (buckets[i] + 1) & (self.buckets - 1);
                        self.prefetch_bucket(buckets[i]);
                        pending = true;
                    }
                }
                if !pending {
                    break;
                }
            }
            if live[..keys.len()].iter().any(|v| *v) {
                return Err(Error::Integrity);
            }
        }
        Ok(())
    }
    pub fn audit(&self) -> Result<()> {
        if ObjectId::for_bytes(&self.bytes[..self.bytes.len() - 32]) != self.id {
            return Err(Error::Integrity);
        }
        let mut previous = None;
        for row in 0..self.count {
            let record = self.row(row)?;
            if previous.as_ref().is_some_and(|p| *p >= key(&record))
                || self.locate(&record.content.id)? != Some(record.clone())
            {
                return Err(Error::Integrity);
            }
            previous = Some(key(&record));
        }
        Ok(())
    }
    /// Stream sorted records, choosing one deterministic location per identity.
    /// Only the compact navigation array and the per-Pack dictionary are held in
    /// RAM during construction; the full row set is streamed to the output file.
    pub fn build(
        file: &mut File,
        rows: impl IntoIterator<Item = Result<LocatedContent>>,
        layout: SearchLayout,
        stop: &AtomicBool,
    ) -> Result<(ObjectId, u64)> {
        let (count, dictionary) = write_sorted_rows(file, rows, layout, stop)?;
        let navigation = (file.stream_position()? + 127) & !127;
        file.set_len(navigation)?;
        let buckets = usize::try_from(count.div_ceil(6).max(1))
            .map_err(|_| Error::TooLarge)?
            .checked_next_power_of_two()
            .ok_or(Error::TooLarge)?;
        let mut nav = vec![
            0u8;
            buckets
                .checked_mul(bucket_width(count))
                .ok_or(Error::TooLarge)?
        ];
        {
            // SAFETY: private build file; the mapped row region is not modified
            // or truncated while constructing navigation. Mapping drops before append.
            let bytes = unsafe { Mmap::map(&*file)? };
            for row in 0..count {
                cancelled(stop)?;
                let at = HEADER + row as usize * layout.width();
                let algo = if number(&bytes, at + 40) & ALGORITHM == 0 {
                    DigestAlgorithm::Sha256
                } else {
                    DigestAlgorithm::Blake3
                };
                let id = ObjectId::from_digest(
                    algo,
                    bytes[at..at + 32]
                        .try_into()
                        .map_err(|_| Error::Integrity)?,
                );
                let h = hash(&id);
                let mut bucket = h as usize & (buckets - 1);
                loop {
                    cancelled(stop)?;
                    let base = bucket * bucket_width(count);
                    if let Some(slot) =
                        (0..SLOTS).find(|slot| nav[base + slot * 2..base + slot * 2 + 2] == [0, 0])
                    {
                        nav[base + slot * 2..base + slot * 2 + 2]
                            .copy_from_slice(&tag(h).to_le_bytes());
                        if count <= u32::MAX as u64 {
                            nav[base + 16 + slot * 4..base + 20 + slot * 4]
                                .copy_from_slice(&(row as u32).to_le_bytes());
                        } else {
                            nav[base + 16 + slot * 8..base + 24 + slot * 8]
                                .copy_from_slice(&row.to_le_bytes());
                        }
                        break;
                    }
                    bucket = (bucket + 1) & (buckets - 1);
                }
            }
        }
        file.seek(SeekFrom::Start(navigation))?;
        file.write_all(&nav)?;
        let dictionary_offset = file.stream_position()?;
        for (digest, size) in &dictionary {
            file.write_all(digest)?;
            file.write_all(&size.to_le_bytes())?;
        }
        let mut header = [0u8; HEADER];
        header[..8].copy_from_slice(MAGIC);
        for (at, value) in [
            (8, layout.width() as u64),
            (16, count),
            (24, buckets as u64),
            (32, navigation),
            (40, dictionary_offset),
            (48, dictionary.len() as u64),
        ] {
            header[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        file.rewind()?;
        file.write_all(&header)?;
        file.rewind()?;
        let id = ObjectId::for_reader(&mut *file)?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(id.digest_bytes())?;
        file.sync_all()?;
        Ok((id, count))
    }
}
fn prefer(a: &PhysicalCandidate, b: &PhysicalCandidate) -> bool {
    match (a, b) {
        (PhysicalCandidate::Loose, PhysicalCandidate::PackMember { .. }) => true,
        (PhysicalCandidate::PackMember { pack: a }, PhysicalCandidate::PackMember { pack: b }) => {
            a.id.digest_bytes() < b.id.digest_bytes()
        }
        _ => false,
    }
}

type PackDictionary = Vec<([u8; 32], u64)>;
pub(super) fn write_sorted_rows(
    file: &mut File,
    rows: impl IntoIterator<Item = Result<LocatedContent>>,
    layout: SearchLayout,
    stop: &AtomicBool,
) -> Result<(u64, PackDictionary)> {
    file.set_len(0)?;
    file.rewind()?;
    file.write_all(&[0; HEADER])?;
    let mut packs = BTreeMap::<[u8; 32], (u64, u64)>::new();
    let mut dictionary = Vec::new();
    let mut count = 0u64;
    let mut previous: Option<LocatedContent> = None;
    {
        let mut writer = BufWriter::with_capacity(64 * 1024, &mut *file);
        let mut emit = |row: LocatedContent| -> Result<()> {
            let location = match &row.location {
                PhysicalCandidate::Loose => 0,
                PhysicalCandidate::PackMember { pack } => {
                    pack.id.require_sha256()?;
                    let key = (*pack.id.digest_bytes(), pack.size_bytes);
                    let (size, ordinal) = packs.entry(key.0).or_insert_with(|| {
                        dictionary.push(key);
                        (key.1, dictionary.len() as u64)
                    });
                    if *size != pack.size_bytes {
                        return Err(Error::Integrity);
                    }
                    *ordinal
                }
            } | algorithm(&row.content.id);
            writer.write_all(row.content.id.digest_bytes())?;
            writer.write_all(&row.content.size_bytes.to_le_bytes())?;
            writer.write_all(&location.to_le_bytes())?;
            if layout.width() == 64 {
                writer.write_all(&[0; 16])?;
            }
            count += 1;
            if count > format::MAX_ROWS {
                return Err(Error::TooLarge);
            }
            Ok(())
        };
        for row in rows {
            cancelled(stop)?;
            let row = row?;
            if row.content.size_bytes > i64::MAX as u64 {
                return Err(Error::Integrity);
            }
            if let Some(old) = previous.as_mut() {
                match key(old).cmp(&key(&row)) {
                    std::cmp::Ordering::Greater => return Err(Error::Integrity),
                    std::cmp::Ordering::Equal => {
                        if old.content.size_bytes != row.content.size_bytes {
                            return Err(Error::Integrity);
                        }
                        if prefer(&row.location, &old.location) {
                            *old = row;
                        }
                        continue;
                    }
                    std::cmp::Ordering::Less => emit(previous.take().ok_or(Error::Integrity)?)?,
                }
            }
            previous = Some(row);
        }
        if let Some(last) = previous {
            emit(last)?;
        }
        writer.flush()?;
    }

    Ok((count, dictionary))
}


