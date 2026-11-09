//! Once the Catalog head owns complete members, a per-Pack registration keeps
//! only identity/count. Uncommitted member lists remain available for recovery.
use super::*;

impl SharedPackStore {
    pub(super) fn receipt_snapshot(&self, index: &Index) -> Result<Arc<SharedIndexSnapshot>> {
        let snapshot = self.snapshot()?;
        if !snapshot.generation.covered.iter().any(|reference| {
            reference.index == index.id && index.pack().is_ok_and(|pack| &reference.pack == pack)
        }) {
            return Err(Error::Integrity);
        }
        Ok(snapshot)
    }

    pub(super) fn receipt_contains(&self, index: &Index, content: &ContentRef) -> Result<bool> {
        let snapshot = self.receipt_snapshot(index)?;
        for row in snapshot.catalog.local_candidates(&content.id)? {
            if row.content != *content {
                return Err(Error::Integrity);
            }
            if matches!(row.location, PhysicalCandidate::PackMember { ref pack } if pack == index.pack()?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Caller holds summary. A producer owns its existing per-Pack lock until
    /// finish, so a busy producer is deferred rather than blocked or modified.
    pub(super) fn compact_members(
        &self,
        snapshot: &SharedIndexSnapshot,
        stop: &AtomicBool,
    ) -> Result<()> {
        for reference in &snapshot.generation.covered {
            cancelled(stop)?;
            let hex = reference.pack.id.hex();
            let _guard = match self.lock(&hex) {
                Ok(guard) => guard,
                Err(Error::InProgress) => continue,
                Err(error) => return Err(error),
            };
            let key = format!("packs/{hex}.idx");
            let index = match self.index(&key, Some(&reference.index)) {
                Ok(index) => index,
                // Explicitly withdrawn registrations must stay withdrawn.
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            };
            if index.pack()? != &reference.pack {
                return Err(Error::Integrity);
            }
            if !index.is_receipt() {
                let mut temp = self.temporary()?;
                index.write_receipt(temp.as_file_mut())?;
                temp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o444))?;
                temp.as_file().sync_all()?;
                
                self.dir.rename(
                    format!(
                        "tmp/{}",
                        temp.path()
                            .file_name()
                            .ok_or(Error::Integrity)?
                            .to_string_lossy()
                    ),
                    &self.dir,
                    &key,
                )?;
                self.sync("packs")?;
                self.sync("tmp")?;
                
            }
            // The current head and its compact registration are durable before
            // the last name of the obsolete complete member list is removed.
            let pending = format!("pending/{hex}.idx");
            match self.index(&pending, Some(&reference.index)) {
                Ok(pending_index) => {
                    if pending_index.pack()? != &reference.pack {
                        return Err(Error::Integrity);
                    }
                    self.dir.remove_file(&pending)?;
                    self.sync("pending")?;
                }
                Err(error) if missing(&error) => (),
                Err(error) => return Err(error),
            }
            let mut cache = self
                .indexes
                .lock()
                .map_err(|_| Error::Backend("shared index cache poisoned".into()))?;
            cache.pop(&key);
            cache.pop(&pending);
            
        }
        Ok(())
    }
}
