//! Recover absent hints with a bounded metadata-only traversal. A cursor names
//! the shard to restart, never an unstable directory cookie or a payload claim.
use super::*;
use crate::backup_audit::open_directory;

pub(super) struct LooseScan {
    _guard: File,
    shard: u16,
    entries: Option<cap_std::fs::ReadDir>,
}
#[derive(Debug, Default)]
pub struct LooseScanReceipt {
    pub examined: usize,
    pub published: usize,
    pub complete: bool,
}
impl SharedPackStore {
    pub fn scan_loose_step(&self, stop: &AtomicBool) -> Result<LooseScanReceipt> {
        let mut state = self
            .scan
            .lock()
            .map_err(|_| Error::Backend("loose scan poisoned".into()))?;
        // On any failure drop the iterator and its lock; next attempt starts
        // from the last durable shard, including an unpublished partial batch.
        let mut scan = match state.take() {
            Some(scan) => scan,
            None => {
                let guard = self.lock("loose-scan")?;
                let shard = match self.open_file("loose-scan-cursor") {
                    Ok(mut file) => {
                        if file.metadata()?.len() != 2 {
                            return Err(Error::Integrity);
                        }
                        let mut bytes = [0; 2];
                        file.read_exact(&mut bytes)?;
                        let value = u16::from_be_bytes(bytes);
                        if value >= 512 {
                            return Err(Error::Integrity);
                        }
                        value
                    }
                    Err(error) if missing(&error) => 0,
                    Err(error) => return Err(error),
                };
                LooseScan {
                    _guard: guard,
                    shard,
                    entries: None,
                }
            }
        };
        let mut receipt = LooseScanReceipt::default();
        let mut batch = Vec::new();
        while scan.shard < 512 && receipt.examined < loose::MAX_LOOSE_BATCH {
            cancelled(stop)?;
            let algorithm = if scan.shard < 256 {
                DigestAlgorithm::Sha256
            } else {
                DigestAlgorithm::Blake3
            };
            let prefix = format!("{:02x}", scan.shard % 256);
            if scan.entries.is_none() {
                let directory = (|| {
                    let algorithm_dir =
                        open_directory(&self.store.dir, Path::new(algorithm.as_str()))?;
                    open_directory(&algorithm_dir, Path::new(&prefix))
                })();
                match directory {
                    Ok(dir) => scan.entries = Some(dir.entries()?),
                    Err(error) if missing(&error) => {
                        scan.shard += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let entry = match scan.entries.as_mut().ok_or(Error::Integrity)?.next() {
                Some(entry) => entry?,
                None => {
                    scan.entries = None;
                    scan.shard += 1;
                    continue;
                }
            };
            receipt.examined += 1;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Integrity)?;
            let key = format!("{}/{prefix}/{name}", algorithm.as_str());
            let id = ObjectId::from_content_key(&key)?;
            if !entry.file_type()?.is_file() {
                return Err(Error::Integrity);
            }
            if self.store.is_index_artifact(&id)? {
                continue;
            }
            let content = ContentRef {
                id,
                size_bytes: entry.metadata()?.len(),
            };
            // Same confined open, length and mode validation as foreground use.
            if self.store.open_shared_file(&content)?.is_some() {
                batch.push(content);
            }
        }
        let pending = self.unindexed_loose(&batch)?;
        receipt.published = pending.len();
        self.publish_loose(&pending, stop)?;
        receipt.complete = scan.shard == 512;
        let next = if receipt.complete { 0 } else { scan.shard };
        self.save_scan_cursor(next)?;
        if !receipt.complete {
            *state = Some(scan);
        }
        Ok(receipt)
    }
    fn save_scan_cursor(&self, shard: u16) -> Result<()> {
        let mut temp = self.temporary()?;
        temp.write_all(&shard.to_be_bytes())?;
        temp.as_file().sync_all()?;
        let name = temp
            .path()
            .file_name()
            .ok_or(Error::Integrity)?
            .to_string_lossy();
        self.dir
            .rename(format!("tmp/{name}"), &self.dir, "loose-scan-cursor")?;
        self.sync(".")
    }
    /// Release a live scan owner on cancellation, allowing another process to
    /// restart its unfinished shard immediately.
    pub fn cancel_loose_scan(&self) -> Result<()> {
        self.scan
            .lock()
            .map_err(|_| Error::Backend("loose scan poisoned".into()))?
            .take();
        Ok(())
    }
}
