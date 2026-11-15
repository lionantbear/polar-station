//! Immutable, mmap-backed fixed records. Mapping is restricted to files owned
//! by the shared-index publisher; arbitrary external files are never mapped.
use memmap2::Mmap;
use polar_cas::{ContentRef, DigestAlgorithm, Error, ObjectId, Result};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
};

const MAGIC: &[u8; 8] = b"PCASIDX1";
pub(super) const HEADER: usize = 64 + 256 * 8;
const FOOTER: usize = 32;
const RECEIPT_MAGIC: &[u8; 8] = b"PCASPKR1";
const RECEIPT_BYTES: usize = 8 + 41 + 33 + 8 + FOOTER;
pub(super) const MAX_ROWS: u64 = 1 << 40;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Record {
    pub digest: [u8; 32],
    pub size: u64,
    pub pack: [u8; 32],
    pub pack_size: u64,
}
impl Record {
    pub fn new(content: &ContentRef, pack: &ContentRef) -> Result<Self> {
        content.id.require_sha256()?;
        pack.id.require_sha256()?;
        Ok(Self {
            digest: *content.id.digest_bytes(),
            size: content.size_bytes,
            pack: *pack.id.digest_bytes(),
            pack_size: pack.size_bytes,
        })
    }
    pub fn image(self) -> ContentRef {
        ContentRef {
            id: ObjectId::from_sha256(self.pack),
            size_bytes: self.pack_size,
        }
    }
}

fn integer(bytes: &[u8], at: usize) -> Result<u64> {
    let value = bytes
        .get(at..at.checked_add(8).ok_or(Error::Integrity)?)
        .ok_or(Error::Integrity)?;
    Ok(u64::from_be_bytes(
        value.try_into().map_err(|_| Error::Integrity)?,
    ))
}

pub(super) struct Index {
    bytes: Mmap,
    pub id: ObjectId,
    pub count: u64,
    width: usize,
    pub loose_algorithm: Option<DigestAlgorithm>,
    pack: Option<ContentRef>,
    ranges: [u64; 256],
}
impl Index {
    pub fn open(file: &File, expected: Option<&ObjectId>) -> Result<Self> {
        let length = file.metadata()?.len();
        if length < RECEIPT_BYTES as u64 || length > MAX_ROWS * 80 + HEADER as u64 + FOOTER as u64 {
            return Err(Error::Integrity);
        }
        // SAFETY: only internally published immutable inodes reach here.
        // Registration compaction replaces the name, never edits the old inode.
        // All participating processes promise no mutation/truncation for the
        // complete mapping lifetime; GC never edits a published inode.
        let bytes = unsafe { Mmap::map(file)? };
        if bytes.get(..8) == Some(RECEIPT_MAGIC) {
            if bytes.len() != RECEIPT_BYTES {
                return Err(Error::Integrity);
            }
            let end = bytes.len() - FOOTER;
            if ObjectId::for_bytes(&bytes[..end]).digest_bytes() != &bytes[end..] {
                return Err(Error::Integrity);
            }
            let mut reader =
                super::binary::Decoder::new(&bytes[..end], RECEIPT_MAGIC, RECEIPT_BYTES)?;
            let pack = reader.content()?;
            polar_cas::native_fs::ErofsPackBinding::native(pack.clone())?;
            let id = reader.object()?;
            id.require_sha256()?;
            let count = reader.u64()?;
            reader.finish()?;
            if count > polar_cas::shared::MAX_PACK_MEMBERS as u64
                || expected.is_some_and(|expected| expected != &id)
            {
                return Err(Error::Integrity);
            }
            return Ok(Self {
                bytes,
                id,
                count,
                width: 0,
                loose_algorithm: None,
                pack: Some(pack),
                ranges: [0; 256],
            });
        }
        if length < (HEADER + FOOTER) as u64 {
            return Err(Error::Integrity);
        }
        if bytes.get(..8) != Some(MAGIC) {
            return Err(Error::UnsupportedCapability);
        }
        if bytes[9..16] != [0; 7] {
            return Err(Error::Integrity);
        }
        let width = match bytes[8] {
            0 | 2 | 3 => 40,
            1 => 80,
            _ => return Err(Error::UnsupportedCapability),
        };
        let count = integer(&bytes, 16)?;
        if count > MAX_ROWS
            || count
                .checked_mul(width as u64)
                .and_then(|n| n.checked_add((HEADER + FOOTER) as u64))
                != Some(length)
        {
            return Err(Error::Integrity);
        }
        let loose_algorithm = match bytes[8] {
            2 => Some(DigestAlgorithm::Sha256),
            3 => Some(DigestAlgorithm::Blake3),
            _ => None,
        };
        let pack = if bytes[8] == 0 {
            let pack = ContentRef {
                id: ObjectId::from_sha256(bytes[24..56].try_into().map_err(|_| Error::Integrity)?),
                size_bytes: integer(&bytes, 56)?,
            };
            polar_cas::native_fs::ErofsPackBinding::native(pack.clone())?;
            Some(pack)
        } else {
            if bytes[24..64] != [0; 40] {
                return Err(Error::Integrity);
            }
            None
        };
        let id = ObjectId::from_sha256(
            bytes[bytes.len() - 32..]
                .try_into()
                .map_err(|_| Error::Integrity)?,
        );
        if expected.is_some_and(|expected| expected != &id) {
            return Err(Error::Integrity);
        }
        let mut ranges = [0; 256];
        let mut previous = 0;
        for (i, item) in ranges.iter_mut().enumerate() {
            *item = integer(&bytes, 64 + 8 * i)?;
            if *item < previous || *item > count {
                return Err(Error::Integrity);
            }
            previous = *item;
        }
        if previous != count {
            return Err(Error::Integrity);
        }
        Ok(Self {
            bytes,
            id,
            count,
            width,
            loose_algorithm,
            pack,
            ranges,
        })
    }
    pub fn pack(&self) -> Result<&ContentRef> {
        self.pack.as_ref().ok_or(Error::Integrity)
    }
    pub fn is_receipt(&self) -> bool {
        self.width == 0
    }
    pub fn write_receipt(&self, file: &mut File) -> Result<()> {
        let mut writer = super::binary::Encoder::new(RECEIPT_MAGIC, RECEIPT_BYTES)?;
        writer.content(self.pack()?)?;
        writer.object(&self.id)?;
        writer.u64(self.count)?;
        let mut bytes = writer.finish();
        let digest = ObjectId::for_bytes(&bytes);
        bytes.extend_from_slice(digest.digest_bytes());
        file.write_all(&bytes)?;
        Ok(())
    }
    pub fn row(&self, at: u64) -> Result<Record> {
        if at >= self.count || self.loose_algorithm.is_some() || self.is_receipt() {
            return Err(Error::Integrity);
        }
        let at = usize::try_from(at)
            .map_err(|_| Error::TooLarge)?
            .checked_mul(self.width)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or(Error::Integrity)?;
        let row = self
            .bytes
            .get(at..at + self.width)
            .ok_or(Error::Integrity)?;
        let (pack, pack_size) = if let Some(pack) = &self.pack {
            (*pack.id.digest_bytes(), pack.size_bytes)
        } else {
            (
                row[40..72].try_into().map_err(|_| Error::Integrity)?,
                integer(row, 72)?,
            )
        };
        Ok(Record {
            digest: row[..32].try_into().map_err(|_| Error::Integrity)?,
            size: integer(row, 32)?,
            pack,
            pack_size,
        })
    }
    // Compare only the key during search; decode the complete record on a hit.
    #[inline(always)]
    fn digest_at(&self, at: u64) -> Result<&[u8; 32]> {
        if at >= self.count {
            return Err(Error::Integrity);
        }
        let offset = usize::try_from(at)
            .map_err(|_| Error::TooLarge)?
            .checked_mul(self.width)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or(Error::Integrity)?;
        self.bytes
            .get(offset..offset + 32)
            .ok_or(Error::Integrity)?
            .try_into()
            .map_err(|_| Error::Integrity)
    }
    pub fn find(&self, digest: &[u8; 32], max_hits: usize) -> Result<Vec<Record>> {
        let mut found = Vec::new();
        self.find_each(digest, max_hits, |row| {
            found.push(row);
            Ok(())
        })?;
        Ok(found)
    }
    pub fn find_each(
        &self,
        digest: &[u8; 32],
        max_hits: usize,
        mut visit: impl FnMut(Record) -> Result<()>,
    ) -> Result<()> {
        if self.is_receipt() {
            return Err(Error::UnsupportedCapability);
        }
        let shard = digest[0] as usize;
        let mut low = if shard == 0 {
            0
        } else {
            self.ranges[shard - 1]
        };
        let end = self.ranges[shard];
        let mut high = end;
        while low < high {
            let mid = low + (high - low) / 2;
            if self.digest_at(mid)? < digest {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let mut hits = 0;
        while low < end {
            if self.digest_at(low)? != digest {
                break;
            }
            let row = self.row(low)?;
            if hits == max_hits {
                return Err(Error::TooLarge);
            }
            visit(row)?;
            hits += 1;
            low += 1;
        }
        Ok(())
    }
    pub fn loose_row(&self, at: u64) -> Result<ContentRef> {
        let algorithm = self.loose_algorithm.ok_or(Error::Integrity)?;
        let digest = *self.digest_at(at)?;
        let offset = usize::try_from(at).map_err(|_| Error::TooLarge)? * 40 + HEADER;
        let size_bytes = integer(&self.bytes, offset + 32)?;
        if size_bytes > i64::MAX as u64 {
            return Err(Error::Integrity);
        }
        Ok(ContentRef {
            id: ObjectId::from_digest(algorithm, digest),
            size_bytes,
        })
    }
    pub fn contains_loose(&self, content: &ContentRef) -> Result<bool> {
        if self.loose_algorithm != Some(content.id.algorithm()) {
            return Ok(false);
        }
        let shard = content.id.digest_bytes()[0] as usize;
        let mut low = if shard == 0 {
            0
        } else {
            self.ranges[shard - 1]
        };
        let end = self.ranges[shard];
        let mut high = end;
        while low < high {
            let mid = low + (high - low) / 2;
            if self.digest_at(mid)? < content.id.digest_bytes() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == end || self.digest_at(low)? != content.id.digest_bytes() {
            return Ok(false);
        }
        if self.loose_row(low)?.size_bytes != content.size_bytes {
            return Err(Error::Integrity);
        }
        Ok(true)
    }
    pub fn audit(&self) -> Result<()> {
        let id = ObjectId::for_bytes(&self.bytes[..self.bytes.len() - FOOTER]);
        if self.is_receipt() {
            return if id.digest_bytes() == &self.bytes[self.bytes.len() - FOOTER..] {
                Ok(())
            } else {
                Err(Error::Integrity)
            };
        }
        if id != self.id {
            return Err(Error::Integrity);
        }
        if self.loose_algorithm.is_some() {
            let mut previous = None;
            for n in 0..self.count {
                let row = self.loose_row(n)?;
                let key = (*row.id.digest_bytes(), row.size_bytes);
                if previous.is_some_and(|old: ([u8; 32], u64)| old.0 >= key.0) {
                    return Err(Error::Integrity);
                }
                let shard = key.0[0] as usize;
                if n >= self.ranges[shard] || (shard > 0 && n < self.ranges[shard - 1]) {
                    return Err(Error::Integrity);
                }
                previous = Some(key);
            }
            return Ok(());
        }
        let mut previous: Option<Record> = None;
        for n in 0..self.count {
            let row = self.row(n)?;
            if previous
                .is_some_and(|old| old >= row || (old.digest == row.digest && old.size != row.size))
            {
                return Err(Error::Integrity);
            }
            let shard = row.digest[0] as usize;
            if n >= self.ranges[shard] || (shard > 0 && n < self.ranges[shard - 1]) {
                return Err(Error::Integrity);
            }
            previous = Some(row);
        }
        Ok(())
    }
}

pub(super) fn write_index(
    file: &mut File,
    pack: Option<&ContentRef>,
    rows: impl IntoIterator<Item = Result<Record>>,
    cancelled: &AtomicBool,
) -> Result<(ObjectId, u64)> {
    let mut header = [0_u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8] = u8::from(pack.is_none());
    if let Some(pack) = pack {
        pack.id.require_sha256()?;
        header[24..56].copy_from_slice(pack.id.digest_bytes());
        header[56..64].copy_from_slice(&pack.size_bytes.to_be_bytes());
    }
    file.write_all(&header)?;
    let mut counts = [0_u64; 256];
    let mut count = 0_u64;
    let mut previous: Option<Record> = None;
    {
        let mut output = std::io::BufWriter::with_capacity(64 * 1024, &mut *file);
        for row in rows {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Io(std::io::ErrorKind::Interrupted.into()));
            }
            let row = row?;
            if let Some(old) = previous {
                if row < old || (row.digest == old.digest && row.size != old.size) {
                    return Err(Error::Integrity);
                }
                if row == old {
                    continue;
                }
                if pack.is_some() && row.digest == old.digest {
                    return Err(Error::Integrity);
                }
            }
            if pack
                .is_some_and(|p| row.pack != *p.id.digest_bytes() || row.pack_size != p.size_bytes)
                || row.size > i64::MAX as u64
                || row.pack_size > i64::MAX as u64
            {
                return Err(Error::Integrity);
            }
            output.write_all(&row.digest)?;
            output.write_all(&row.size.to_be_bytes())?;
            if pack.is_none() {
                output.write_all(&row.pack)?;
                output.write_all(&row.pack_size.to_be_bytes())?;
            }
            counts[row.digest[0] as usize] += 1;
            count = count
                .checked_add(1)
                .filter(|n| *n <= MAX_ROWS)
                .ok_or(Error::TooLarge)?;
            previous = Some(row);
        }
        output.flush()?;
    }
    header[16..24].copy_from_slice(&count.to_be_bytes());
    let mut sum = 0_u64;
    for (i, n) in counts.into_iter().enumerate() {
        sum += n;
        header[64 + i * 8..72 + i * 8].copy_from_slice(&sum.to_be_bytes());
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

/// Compact loose records use the same range table, digest search and immutable
/// publication discipline as Pack indexes, without a fictitious Pack identity.
pub(super) fn write_loose_index(
    file: &mut File,
    algorithm: DigestAlgorithm,
    rows: impl IntoIterator<Item = Result<ContentRef>>,
    cancelled: &AtomicBool,
) -> Result<(ObjectId, u64)> {
    let mut header = [0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8] = match algorithm {
        DigestAlgorithm::Sha256 => 2,
        DigestAlgorithm::Blake3 => 3,
    };
    file.write_all(&header)?;
    let mut counts = [0u64; 256];
    let mut count = 0u64;
    let mut previous: Option<ContentRef> = None;
    {
        let mut output = std::io::BufWriter::with_capacity(64 * 1024, &mut *file);
        for row in rows {
            super::cancelled(cancelled)?;
            let row = row?;
            if row.id.algorithm() != algorithm || row.size_bytes > i64::MAX as u64 {
                return Err(Error::Integrity);
            }
            if let Some(old) = &previous {
                if old.id.digest_bytes() > row.id.digest_bytes()
                    || (old.id == row.id && old.size_bytes != row.size_bytes)
                {
                    return Err(Error::Integrity);
                }
                if old == &row {
                    continue;
                }
            }
            output.write_all(row.id.digest_bytes())?;
            output.write_all(&row.size_bytes.to_be_bytes())?;
            counts[row.id.digest_bytes()[0] as usize] += 1;
            count = count
                .checked_add(1)
                .filter(|n| *n <= MAX_ROWS)
                .ok_or(Error::TooLarge)?;
            previous = Some(row);
        }
        output.flush()?;
    }
    header[16..24].copy_from_slice(&count.to_be_bytes());
    let mut sum = 0u64;
    for (i, n) in counts.into_iter().enumerate() {
        sum += n;
        header[64 + i * 8..72 + i * 8].copy_from_slice(&sum.to_be_bytes());
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
