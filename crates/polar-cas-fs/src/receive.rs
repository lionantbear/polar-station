//! Receiving-Host gate: verify bytes and the full logical view before publishing
//! any native mapping. A transport receipt alone never constructs this type.
use crate::{
    host::{HostPins, PreparedNativeView},
    projection::{cas_error, descriptor, verify_pack_members},
    verity::VerityPolicy,
    Image,
};
use polar_cas::{
    catalog::{native_fs::NativeFsCatalog, MAX_REGISTER_SOURCES},
    manifest::bundle::BundleLimits,
    native_fs::{transfer::NativeTransferPlan, VerifiedPackedMembers},
    projection::{ProjectionLayout, ProjectionTree},
    source::LocationId,
    ContentRef, Error, Result,
};
use std::{collections::BTreeSet, fs::File, sync::Arc};

/// One explicit namespace per ordered P. Location ids belong to the receiving
/// Catalog, not to the sender's Host paths. No credentials or implicit discovery.
fn validate_locations(plan: &NativeTransferPlan, locations: &[LocationId]) -> Result<()> {
    if locations.len() != plan.projection().packs.len() {
        return Err(Error::Invalid(
            "one receiving location per ordered pack required".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    if locations.iter().any(|id| !ids.insert(id.as_str())) {
        return Err(Error::Conflict);
    }
    Ok(())
}

pub struct ReceivedNativeProjection {
    view: PreparedNativeView,
    memberships: Vec<(LocationId, VerifiedPackedMembers)>,
}
impl ReceivedNativeProjection {
    /// Blocking native/verity preparation. Execute the entire function on a
    /// blocking worker. Returned fields contain no thread-affine native handles.
    /// No Catalog write or native filesystem mount occurs here.
    pub fn prepare(
        plan: &NativeTransferPlan,
        meta: File,
        packs: Vec<File>,
        locations: &[LocationId],
        policy: VerityPolicy,
        pins: &HostPins,
    ) -> Result<Self> {
        validate_locations(plan, locations)?;
        if packs.len() != locations.len() {
            return Err(Error::Integrity);
        }
        let limits = BundleLimits {
            tree: plan.request().options.tree_limits,
            max_bundle_bytes: plan.request().options.max_metadata_bytes,
            max_directories: plan
                .request()
                .options
                .tree_limits
                .max_entries
                .saturating_add(1),
            max_storages: 0,
            max_sources: 0,
            max_routes_per_object: 0,
        };
        let tree = ProjectionTree::plan(
            plan.tree().root(),
            plan.tree().directories(),
            &ProjectionLayout::Preserve,
            plan.tree().root().id.algorithm(),
            limits,
        )?;
        let view = PreparedNativeView::prepare(
            &tree,
            plan.projection().clone(),
            meta,
            packs,
            policy,
            pins,
        )?;
        let images: Vec<Image> = view
            .packs()
            .iter()
            .map(|prepared| {
                let expected = descriptor(prepared.content())?;
                Image::open(prepared.file(), &expected.sha256, expected.size_bytes)
                    .map_err(cas_error)
            })
            .collect::<Result<_>>()?;
        let mut groups: Vec<Vec<ContentRef>> = vec![Vec::new(); images.len()];
        for content in plan.tree().closure().files() {
            let expected = descriptor(content)?;
            let mut found = false;
            for (index, image) in images.iter().enumerate() {
                match image.member(&expected.sha256, expected.size_bytes) {
                    Ok(_) => {
                        groups[index].push(content.clone());
                        found = true;
                        break;
                    }
                    Err(error) if error.status == polar_cas_fs_sys::PCFS_NOT_FOUND => {}
                    // A present wrong-size/corrupt member is not hidden by
                    // trying another pack. It is a failed admission.
                    Err(error) => return Err(cas_error(error)),
                }
            }
            if !found {
                return Err(Error::NotFound);
            }
        }
        let mut memberships = Vec::new();
        for (index, group) in groups.iter().enumerate() {
            if group.is_empty() {
                // Still register the exact backing namespace for an unused
                // dependency. This creates no fabricated member replica.
                memberships.push((
                    locations[index].clone(),
                    verify_pack_members(&images[index], &[])?,
                ));
            } else {
                for batch in group.chunks(MAX_REGISTER_SOURCES) {
                    memberships.push((
                        locations[index].clone(),
                        verify_pack_members(&images[index], batch)?,
                    ));
                }
            }
        }
        Ok(Self { view, memberships })
    }
    /// The ordinary copy/ingest path must first admit T/M/P identities.
    /// On a new Host use copy_native_manifest_nodes to index/persist the bundle's
    /// canonical directory nodes locally (or reuse an already-admitted closure). Commit
    /// verified membership batches, then the projection LAST. A failure can
    /// leave valid member replicas, but never a partially validated projection.
    /// Repeating the same verified admission is idempotent in NativeFsCatalog.
    /// The view/pins remain alive across every await and through caller handoff.
    /// The completion task owns self: cancelling only the waiter does NOT
    /// release M/P pins while an already-dispatched SQL write can still finish.
    /// There is no caller-visible abort handle and no implicit retry policy.
    pub async fn register(self: Arc<Self>, catalog: Arc<dyn NativeFsCatalog>) -> Result<String> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            Error::Backend(format!("native registration requires Tokio runtime: {e}"))
        })?;
        runtime
            .spawn(async move {
                for (location, members) in &self.memberships {
                    catalog.register_pack_members(location, members).await?;
                }
                catalog
                    .register_native_projection(self.view.verification())
                    .await
            })
            .await
            .map_err(|e| Error::Backend(format!("native registration task failed: {e}")))?
    }
    pub fn view(&self) -> &PreparedNativeView {
        &self.view
    }
    pub fn membership_batches(&self) -> usize {
        self.memberships.len()
    }
    pub fn into_view(self) -> PreparedNativeView {
        self.view
    }
}


