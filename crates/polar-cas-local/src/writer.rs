//! Trusted synchronous save binding. Adapter commits and internal index artifacts
//! do NOT call this hook: async copies own their original operation attribution.
use crate::{catalog_index::UnifiedCatalogIndex, LocalCas};
use polar_cas::{
    catalog::writers::{SaveAttribution, WriterRecord},
    source::LocationId,
    ContentRef, Error, ObjectKeyLayout, Result,
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct LocalSaveBinding {
    index: Arc<UnifiedCatalogIndex>,
    attribution: SaveAttribution,
    destination: LocationId,
}

impl LocalCas {
    /// Host-only configuration. A raw/unbound store keeps legacy unknown-origin
    /// semantics; request handlers must obtain this binding from their runtime.
    /// This does not reconfigure a previously bound handle or grant Store access.
    pub fn with_save_writer(
        mut self,
        index: Arc<UnifiedCatalogIndex>,
        attribution: SaveAttribution,
        destination: LocationId,
    ) -> Result<Self> {
        if self.save_binding.is_some() {
            return Err(Error::Conflict);
        }
        attribution.validate()?;
        if !matches!(self.key_layout, ObjectKeyLayout::SharedContent)
            || !index.accepts_local_writer(self.root())
        {
            return Err(Error::UnsupportedCapability);
        }
        self.save_binding = Some(Arc::new(LocalSaveBinding {
            index,
            attribution,
            destination,
        }));
        Ok(self)
    }

    pub(super) fn record_shared_save(&self, content: &ContentRef) -> Result<()> {
        let Some(binding) = &self.save_binding else {
            return Ok(());
        };
        // No success is returned until the durable writer head is published.
        // There is no old synchronous operation log to recover: a failed caller
        // explicitly retries its save. Bytes and the old valid head are retained.
        let first_saved_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid("system clock before epoch".into()))?
            .as_millis()
            .try_into()
            .map_err(|_| Error::TooLarge)?;
        let record = WriterRecord {
            content: content.clone(),
            attribution: binding.attribution.clone(),
            destination: binding.destination.clone(),
            first_saved_at_ms,
        };
        let stop = std::sync::atomic::AtomicBool::new(false);
        binding
            .index
            .begin_writer_publication(
                &stop,
                std::time::Instant::now() + crate::catalog_index::WRITER_PUBLICATION_LOCK_TIMEOUT,
            )?
            .record_writes(&[record])
    }
}


