//! Benchmark-only PtrHash payload layout: the slot directly contains the exact
//! key and location; no slot->sorted-row indirection and no per-batch allocation.
use super::*;
use memmap2::{Mmap, MmapMut};
pub struct DirectRecords {
    bytes: Mmap,
    width: usize,
    pack: ContentRef,
}
impl DirectRecords {
    pub fn build(
        path: &std::path::Path,
        width: usize,
        rows: &[LocatedContent],
        ph: &FastPtrHash<FxHash, [u8; 32]>,
        pack: &ContentRef,
    ) -> Self {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.set_len((ph.max_index() * width) as u64).unwrap();
        // SAFETY: private file, uniquely mapped for initialization; never resized.
        let mut bytes = unsafe { MmapMut::map_mut(&file).unwrap() };
        for row in rows {
            let at = ph.index(row.content.id.digest_bytes()) * width;
            bytes[at..at + 32].copy_from_slice(row.content.id.digest_bytes());
            bytes[at + 32..at + 40].copy_from_slice(&row.content.size_bytes.to_le_bytes());
            let kind = if matches!(row.location, PhysicalCandidate::Loose) {
                1u64
            } else {
                2
            };
            bytes[at + 40..at + 48].copy_from_slice(&kind.to_le_bytes());
        }
        Self {
            bytes: bytes.make_read_only().unwrap(),
            width,
            pack: pack.clone(),
        }
    }
    pub fn prefetch(&self, slot: usize) {
        let at = slot * self.width;
        assert!(at < self.bytes.len());
        #[cfg(target_arch = "x86_64")]
        // SAFETY: slot validated inside the immutable mapping; SSE is baseline.
        unsafe {
            std::arch::x86_64::_mm_prefetch(
                self.bytes.as_ptr().add(at).cast(),
                std::arch::x86_64::_MM_HINT_T0,
            );
        }
    }
    pub fn locate(&self, slot: usize, key: &ObjectId) -> Option<LocatedContent> {
        let at = slot * self.width;
        if &self.bytes[at..at + 32] != key.digest_bytes() {
            return None;
        }
        let value = |offset| u64::from_le_bytes(self.bytes[offset..offset + 8].try_into().unwrap());
        let kind = value(at + 40);
        // Empty PHF slots have kind=0, including a hypothetical all-zero query.
        if kind == 0 {
            return None;
        }
        Some(LocatedContent {
            content: ContentRef {
                id: key.clone(),
                size_bytes: value(at + 32),
            },
            location: if kind == 1 {
                PhysicalCandidate::Loose
            } else {
                PhysicalCandidate::PackMember {
                    pack: self.pack.clone(),
                }
            },
        })
    }
    pub fn file_bytes(&self) -> usize {
        self.bytes.len()
    }
}
