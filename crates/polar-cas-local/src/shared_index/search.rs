//! Unified immutable exact search and competing persistent navigators.
use super::*;
pub(super) mod grouped;
pub(super) mod perfect;
use grouped::GroupedIndex;
pub use grouped::SearchLayout;
use perfect::PerfectIndex;
use polar_cas::shared::LocatedContent;

enum Navigator {
    Grouped(Box<GroupedIndex>),
    Perfect(Box<PerfectIndex>),
}
pub struct SearchIndex {
    pub id: ObjectId,
    pub count: u64,
    navigator: Navigator,
}
macro_rules! dispatch {
    ($self:expr, $method:ident $(::<$w:ident>)? ($($arg:expr),*)) => {
        match &$self.navigator {
            Navigator::Grouped(i) => i.$method $(::<$w>)? ($($arg),*),
            Navigator::Perfect(i) => i.$method $(::<$w>)? ($($arg),*),
        }
    }
}
impl SearchIndex {
    /// Map only immutable files published in the trusted shared storage domain.
    pub fn open(file: &File, expected: Option<&ObjectId>) -> Result<Self> {
        use std::os::unix::fs::FileExt;
        let mut magic = [0; 8];
        file.read_exact_at(&mut magic, 0)?;
        if &magic == b"PCASPHF1" {
            let i = PerfectIndex::open(file, expected)?;
            Ok(Self {
                id: i.id.clone(),
                count: i.count,
                navigator: Navigator::Perfect(Box::new(i)),
            })
        } else {
            let i = GroupedIndex::open(file, expected)?;
            Ok(Self {
                id: i.id.clone(),
                count: i.count,
                navigator: Navigator::Grouped(Box::new(i)),
            })
        }
    }
    /// The grouped layout is retained as an independently measured control.
    pub fn build(
        file: &mut File,
        rows: impl IntoIterator<Item = Result<LocatedContent>>,
        layout: SearchLayout,
        stop: &AtomicBool,
    ) -> Result<(ObjectId, u64)> {
        GroupedIndex::build(file, rows, layout, stop)
    }
    /// Production format: complete records reside directly at perfect-hash slots.
    pub fn build_fast(
        file: &mut File,
        rows: impl IntoIterator<Item = Result<LocatedContent>>,
        scratch: &Path,
        stop: &AtomicBool,
    ) -> Result<(ObjectId, u64)> {
        PerfectIndex::build(file, rows, scratch, stop)
    }
    pub fn file_bytes(&self) -> usize {
        dispatch!(self, file_bytes())
    }
    pub fn row(&self, row: u64) -> Result<LocatedContent> {
        dispatch!(self, row(row))
    }
    pub fn locate(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        dispatch!(self, locate(id))
    }
    pub fn locate_row(&self, row: u64, id: &ObjectId) -> Result<Option<LocatedContent>> {
        dispatch!(self, locate_row(row, id))
    }
    pub fn prefetch_row(&self, row: u64) -> Result<()> {
        dispatch!(self, prefetch_row(row))
    }
    pub fn locate_binary(&self, id: &ObjectId) -> Result<Option<LocatedContent>> {
        dispatch!(self, locate_binary(id))
    }
    pub fn locate_binary_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        dispatch!(self, locate_binary_batch_into(ids, out))
    }
    pub fn locate_batch(&self, ids: &[ObjectId]) -> Result<Vec<Option<LocatedContent>>> {
        if ids.len() > 1024 {
            return Err(Error::TooLarge);
        }
        let mut out = vec![None; ids.len()];
        self.locate_batch_into::<32>(ids, &mut out)?;
        Ok(out)
    }
    pub fn locate_batch_into<const W: usize>(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<LocatedContent>],
    ) -> Result<()> {
        if W == 0 || W > 64 || ids.len() > 1024 || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        out.fill(None);
        self.locate_missing_into::<W>(ids, out)
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
        dispatch!(self, locate_missing_into::<W>(ids, out))
    }
    pub fn audit(&self) -> Result<()> {
        dispatch!(self, audit())
    }
}
