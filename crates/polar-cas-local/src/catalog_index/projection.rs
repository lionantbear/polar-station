use super::*;
use crate::shared_index::partitions::Root;



fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(Error::Integrity);
    }
    Ok(())
}

pub(super) fn validate(projections: &BTreeMap<String, CatalogProjection>) -> Result<()> {
    if projections.len() > 256 {
        return Err(Error::TooLarge);
    }
    for (name, projection) in projections {
        validate_name(name)?;
        projection.query.id.require_sha256()?;
        projection.selection.id.require_sha256()?;
    }
    Ok(())
}

impl CatalogSnapshot {
    /// Pure lookup in this pinned generation; never opens the query or originals.
    pub fn projection(&self, name: &str) -> Result<Option<&CatalogProjection>> {
        validate_name(name)?;
        Ok(self.0.projections.get(name))
    }
}

impl UnifiedCatalogIndex {
    /// Select an established domain projection, or install the prepared initial
    /// pair if it is still absent. This is first-open admission, not an update
    /// with a stale expected head. Domain bytes must be prepared before calling.
    /// The existing head lock covers only selection and metadata publication;
    /// there is no conflict retry and no domain extraction under this lock.
    pub fn initialize_projection(
        &self,
        name: &str,
        initial: CatalogProjection,
    ) -> Result<CatalogProjection> {
        validate_name(name)?;
        initial.query.id.require_sha256()?;
        initial.selection.id.require_sha256()?;
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        let head = self.head.as_ref().ok_or(Error::UnsupportedCapability)?;
        let _publication = self.publication_guard()?;
        let _acquisition = crate::shared_index::lifecycle::acquire(storage)?;
        let cached = self.current()?;
        let cached_root = cached.index.as_ref().map(|index| index.reference().clone());
        let (pinned_root, projection) = {
            let _writer = head.initialization_writer()?;
            let selected_root = head.current()?;
            let mut root = match &selected_root {
                Some(reference) => Root::read(storage, reference)?,
                None if cached_root.is_none() => {
                    Root::empty(persistent::metadata(&State::default())?)
                }
                None => return Err(Error::Integrity),
            };
            let mut metadata = persistent::decode_metadata(&root.metadata)?;
            if let Some(established) = metadata.projections.get(name).cloned() {
                (selected_root.ok_or(Error::Integrity)?, established)
            } else {
                metadata.schema = "polar.cas.catalog-index.v5".into();
                metadata
                    .projections
                    .insert(name.to_owned(), initial.clone());
                validate(&metadata.projections)?;
                root.metadata = serde_json::to_value(metadata).map_err(|_| Error::Integrity)?;
                // The same root codec preserves every immutable partition ref.
                // No partition opening, record walk, or domain loading occurs
                // while this existing short disk-head lock is held.
                let reference = root.publish(storage)?;
                // Do not call commit: this scope already owns the head lock.
                // In particular, OutcomeUncertain is never converted to success.
                head.publish(&reference)?;
                (reference, initial)
            }
        };
        // Pin once under the head lock; hydrate exactly that immutable version
        // after unlocking. A newer disk head must not mix into this snapshot.
        let selected = if cached_root.as_ref() == Some(&pinned_root) {
            cached.clone()
        } else {
            Self::open_local(storage.clone(), &pinned_root)?.current()?
        };
        if selected.projections.get(name) != Some(&projection) {
            return Err(Error::Integrity);
        }
        let mut current = self
            .state
            .write()
            .map_err(|_| Error::Backend("Catalog state poisoned".into()))?;
        if !Arc::ptr_eq(&current, &cached) {
            return Err(Error::Conflict);
        }
        *current = selected;
        Ok(projection)
    }

    /// Publish a domain query root and its original-selection root atomically.
    /// The domain prepares and validates both immutable objects before calling.
    /// No scanning, importing, implicit refresh or conflict retry occurs here.
    pub fn publish_projection(
        &self,
        name: &str,
        expected: Option<&CatalogProjection>,
        projection: CatalogProjection,
    ) -> Result<()> {
        self.publish_projection_with_cancel(name, expected, projection, None, &[])
    }

    /// Select prepared domain roots and physical-row extensions in one commit.
    /// The caller validates the two directions before requesting publication.
    pub fn publish_projection_with_extensions(
        &self,
        name: &str,
        expected: Option<&CatalogProjection>,
        projection: CatalogProjection,
        extensions: &[super::extensions::ExtensionUpdate],
    ) -> Result<()> {
        self.publish_projection_with_cancel(name, expected, projection, None, extensions)
    }

    /// Publish a prepared active domain update in the existing physical-summary
    /// synchronization domain. Refresh exactly once after entering that domain;
    /// the original expected projection is still required, with no conflict retry.
    /// The deadline bounds summary-lock acquisition only. Cancellation is checked
    /// after lock waits and before commit, not used to roll back an atomic commit.
    pub fn publish_active_projection(
        &self,
        name: &str,
        expected: Option<&CatalogProjection>,
        projection: CatalogProjection,
        stop: &std::sync::atomic::AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<()> {
        self.publish_active_projection_with_extensions(
            name,
            expected,
            projection,
            &[],
            stop,
            deadline,
        )
    }

    /// Active publication with physical-row extensions under the same summary
    /// lock, expected projection check and cancellation boundary as domain roots.
    pub fn publish_active_projection_with_extensions(
        &self,
        name: &str,
        expected: Option<&CatalogProjection>,
        projection: CatalogProjection,
        extensions: &[super::extensions::ExtensionUpdate],
        stop: &std::sync::atomic::AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<()> {
        validate_name(name)?;
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        let _summary = crate::shared_index::save_publication_lock(storage, stop, deadline)?;
        self.refresh()?;
        self.publish_projection_with_cancel(name, expected, projection, Some(stop), extensions)
    }

    fn publish_projection_with_cancel(
        &self,
        name: &str,
        expected: Option<&CatalogProjection>,
        projection: CatalogProjection,
        stop: Option<&std::sync::atomic::AtomicBool>,
        extensions: &[super::extensions::ExtensionUpdate],
    ) -> Result<()> {
        let check_cancelled = || {
            if stop.is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire)) {
                Err(Error::Io(std::io::ErrorKind::Interrupted.into()))
            } else {
                Ok(())
            }
        };
        check_cancelled()?;
        validate_name(name)?;
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        let _publication = self.publication_guard()?;
        check_cancelled()?;
        let old = self.current()?;
        if old.projections.get(name) != expected {
            return Err(Error::Conflict);
        }
        let changes = super::extensions::changes(&old, extensions)?;
        let mut next = (*old).clone();
        next.projections.insert(name.to_owned(), projection);
        validate(&next.projections)?;
        next.index = Some(Arc::new(persistent::publish_values(
            storage,
            old.index.as_deref(),
            &changes,
            persistent::metadata(&next)?,
        )?));
        
        check_cancelled()?;
        self.commit(&old, next)
    }
}


