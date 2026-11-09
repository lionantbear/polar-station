//! Loose publication and discovery extend the existing physical index store.
use super::*;

pub const MAX_LOOSE_BATCH: usize = 4096;

/// Metadata candidates and an already securely opened loose backing. Each
/// returned lease owns an independent open file description, including repeats.
pub struct ManagedCandidates {
    pub candidates: Vec<PhysicalCandidate>,
    pub loose: Option<polar_cas::ports::ManagedReplica>,
}

impl SharedIndexSnapshot {
    pub fn lookup_candidates(
        &self,
        content: &ContentRef,
        budget: usize,
    ) -> Result<Vec<PhysicalCandidate>> {
        if budget == 0 || budget > MAX_PACKS {
            return Err(Error::TooLarge);
        }
        let rows = self.catalog.local_candidates(&content.id)?;
        if rows.iter().any(|row| row.content != *content) {
            return Err(Error::ClaimMismatch);
        }
        Ok(rows
            .into_iter()
            .take(budget)
            .map(|row| row.location)
            .collect())
    }
    pub fn covered_loose_batches(&self) -> usize {
        self.covered_loose.len()
    }
}

impl SharedPackStore {
    /// Payload must already be durable and immutable. This operation never
    /// takes the summary lock, reads payload bytes, or fabricates Pack records.
    pub fn publish_loose(
        &self,
        contents: &[ContentRef],
        stop: &AtomicBool,
    ) -> Result<Vec<ObjectId>> {
        if contents.len() > MAX_LOOSE_BATCH {
            return Err(Error::TooLarge);
        }
        let mut ids = Vec::new();
        for algorithm in [DigestAlgorithm::Sha256, DigestAlgorithm::Blake3] {
            let mut rows: Vec<_> = contents
                .iter()
                .filter(|c| c.id.algorithm() == algorithm)
                .cloned()
                .collect();
            rows.sort_unstable_by(|a, b| a.id.digest_bytes().cmp(b.id.digest_bytes()));
            rows.dedup();
            if rows.is_empty() {
                continue;
            }
            for content in &rows {
                cancelled(stop)?;
                self.store
                    .open_shared_file(content)?
                    .ok_or(Error::NotFound)?
                    .sync_all()?;
                self.store.sync_parents(&content.id.content_key())?;
            }
            let mut temp = self.temporary()?;
            let (id, _) = format::write_loose_index(
                temp.as_file_mut(),
                algorithm,
                rows.into_iter().map(Ok),
                stop,
            )?;
            cancelled(stop)?;
            self.publish_file(&temp, &format!("loose/{}.idx", id.hex()))?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Pure snapshot search. The candidate budget is retained for source compatibility;
    /// ordinary lookup now selects one location without opening it.
    pub fn lookup_batch(
        &self,
        contents: &[ContentRef],
        budget: usize,
    ) -> Result<Vec<Result<Vec<PhysicalCandidate>>>> {
        if budget == 0 || budget > MAX_PACKS {
            return Err(Error::TooLarge);
        }
        let ids: Vec<_> = contents.iter().map(|c| c.id.clone()).collect();
        Ok(self
            .search_snapshot()?
            .locate_batch(&ids)?
            .into_iter()
            .map(|hit| Ok(hit.map(|h| h.location).into_iter().collect()))
            .collect())
    }
    /// Explicit open phase retained for old Reader adapters, not a search API.
    pub fn lookup_managed_batch(
        &self,
        contents: &[ContentRef],
        budget: usize,
    ) -> Result<Vec<Result<ManagedCandidates>>> {
        let found = self.lookup_batch(contents, budget)?;
        Ok(contents
            .iter()
            .zip(found)
            .map(|(content, candidates)| {
                let candidates = candidates?;
                let loose = if candidates.contains(&PhysicalCandidate::Loose) {
                    self.store.open_shared_managed(content)?
                } else {
                    None
                };
                Ok(ManagedCandidates { candidates, loose })
            })
            .collect())
    }
    pub fn lookup_candidates(
        &self,
        content: &ContentRef,
        budget: usize,
    ) -> Result<Vec<PhysicalCandidate>> {
        if budget == 0 || budget > MAX_PACKS {
            return Err(Error::TooLarge);
        }
        self.search_snapshot()?.lookup_candidates(content, budget)
    }

    /// Used by recovery before publishing; includes batches not yet aggregated.
    pub fn unindexed_loose(&self, contents: &[ContentRef]) -> Result<Vec<ContentRef>> {
        if contents.len() > MAX_LOOSE_BATCH {
            return Err(Error::TooLarge);
        }
        let snapshot = self.snapshot()?;
        let mut found = vec![false; contents.len()];
        for (at, content) in contents.iter().enumerate() {
            found[at] = snapshot
                .locate(&content.id)?
                .is_some_and(|h| h.location == PhysicalCandidate::Loose);
        }
        for (n, name) in self
            .listing_except("loose", Some(&snapshot.covered_loose))?
            .into_iter()
            .enumerate()
        {
            let id = ObjectId::try_from(format!("sha256:{}", &name[..64]))?;
            let key = format!("loose/{name}");
            let index = if n < INDEX_CACHE_CAPACITY {
                self.index(&key, Some(&id))?
            } else {
                Arc::new(Index::open(&self.open_file(&key)?, Some(&id))?)
            };
            if index.loose_algorithm.is_none() {
                return Err(Error::Integrity);
            }
            for (at, content) in contents.iter().enumerate() {
                found[at] |= index.contains_loose(content)?;
            }
        }
        let result = contents
            .iter()
            .zip(found)
            .filter(|(_, found)| !found)
            .map(|(content, _)| content.clone())
            .collect();
        Ok(result)
    }
}
