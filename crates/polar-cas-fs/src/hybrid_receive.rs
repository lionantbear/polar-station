//! Receiving Host: verify the actual M and every redirect provider before any
//! new member/variant registration. No filesystem mount during preparation.
use crate::{
    coverage::CoverageBudget,
    host::{HostPins, PreparedImage},
    hybrid_host::PreparedHybridView,
    hybrid_transfer::verify_transfer,
    projection::{cas_error, descriptor, verify_pack_members},
    verity::VerityPolicy,
    HybridMeta, Image,
};
use polar_cas::{
    catalog::{hybrid_fs::HybridFsCatalog, native_fs::NativeFsCatalog, MAX_REGISTER_SOURCES},
    hybrid_fs::{transfer::HybridTransferPlan, VerifiedHybridProjection},
    native_fs::VerifiedPackedMembers,
    projection::ProjectionNode,
    source::LocationId,
    ContentRef, Error, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub struct ReceivedHybridProjection {
    view: PreparedHybridView,
    providers: Vec<PreparedImage>,
    witness: VerifiedHybridProjection,
    memberships: Vec<(LocationId, VerifiedPackedMembers)>,
}
impl ReceivedHybridProjection {
    /// `pack_files` and `locations` match plan.packs(): unique physical packs,
    /// NOT only native devices. Native order is reconstructed from the exact
    /// descriptor list; a P used in both roles needs only one transferred file.
    /// Execute on a blocking worker. No C image handle survives this call.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        plan: &HybridTransferPlan,
        meta: File,
        pack_files: Vec<File>,
        locations: &[LocationId],
        policy: VerityPolicy,
        pins: &HostPins,
        budget: CoverageBudget,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Backend("Hybrid receive cancelled".into()));
        }
        let descriptors: Vec<_> = plan.packs().cloned().collect();
        if pack_files.len() != descriptors.len() || locations.len() != descriptors.len() {
            return Err(Error::Invalid(
                "one receiving location and file per unique physical pack required".into(),
            ));
        }
        let mut names = BTreeSet::new();
        if locations.iter().any(|v| !names.insert(v.as_str())) {
            return Err(Error::Conflict);
        }
        let meta = PreparedImage::prepare(
            meta,
            plan.projection().meta.clone(),
            policy,
            pins.pin(std::slice::from_ref(&plan.projection().meta))?,
        )?;
        let mut prepared = Vec::with_capacity(descriptors.len());
        let mut index = BTreeMap::new();
        for (file, expected) in pack_files.into_iter().zip(&descriptors) {
            let at = prepared.len();
            prepared.push(PreparedImage::prepare(
                file,
                expected.clone(),
                policy,
                pins.pin(std::slice::from_ref(expected))?,
            )?);
            index.insert(expected.id.to_string(), at);
        }
        let images: Vec<_> = prepared
            .iter()
            .map(|p| {
                let d = descriptor(p.content())?;
                Image::open(p.file(), &d.sha256, d.size_bytes).map_err(cas_error)
            })
            .collect::<Result<_>>()?;
        let slots = |refs: &[ContentRef]| -> Result<Vec<usize>> {
            refs.iter()
                .map(|c| {
                    index
                        .get(&c.id.to_string())
                        .copied()
                        .ok_or(Error::Integrity)
                })
                .collect()
        };
        let native_slots = slots(&plan.projection().native_packs)?;
        let provider_slots = slots(plan.redirect_packs())?;
        let native: Vec<_> = native_slots.iter().map(|i| &images[*i]).collect();
        let providers: Vec<_> = provider_slots.iter().map(|i| &images[*i]).collect();
        let verified = verify_transfer(plan, meta.file(), &native, &providers, budget, cancel)?;
        let mut groups: Vec<BTreeMap<String, ContentRef>> = vec![BTreeMap::new(); images.len()];
        for (content, chosen) in plan.redirects().iter().zip(&verified.selected_providers) {
            let slot = *provider_slots.get(*chosen).ok_or(Error::Integrity)?;
            groups[slot].insert(content.id.to_string(), content.clone());
        }
        let required: BTreeSet<_> = plan.redirects().iter().map(|c| c.id.to_string()).collect();
        let decoded = HybridMeta::open(meta.file(), descriptor(&plan.projection().meta)?, &native)
            .map_err(cas_error)?;
        for entry in verified.tree.entries() {
            let ProjectionNode::File { content, .. } = &entry.node else {
                continue;
            };
            if required.contains(&content.id.to_string()) {
                continue;
            }
            let native_slot = match decoded.native_pack_slot(&entry.path).map_err(cas_error)? {
                Some(slot) => slot as usize,
                None if content.size_bytes == 0 => {
                    // Empty native files encode no device slot. Establish actual
                    // pack membership before creating any corresponding Replica.
                    let mut found = None;
                    for (slot, image) in native.iter().enumerate() {
                        match image.member(content.id.digest_bytes(), 0) {
                            Ok(_) => {
                                found = Some(slot);
                                break;
                            }
                            Err(e) if e.status == polar_cas_fs_sys::PCFS_NOT_FOUND => (),
                            Err(e) => return Err(cas_error(e)),
                        }
                    }
                    found.ok_or(Error::NotFound)?
                }
                _ => return Err(Error::Integrity),
            };
            let slot = *native_slots.get(native_slot).ok_or(Error::Integrity)?;
            groups[slot].insert(content.id.to_string(), content.clone());
        }
        drop(decoded);
        let mut memberships = Vec::new();
        for (i, group) in groups.into_iter().enumerate() {
            let members: Vec<_> = group.into_values().collect();
            if members.is_empty() {
                memberships.push((locations[i].clone(), verify_pack_members(&images[i], &[])?));
            } else {
                for batch in members.chunks(MAX_REGISTER_SOURCES) {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(Error::Backend(
                            "Hybrid member verification cancelled".into(),
                        ));
                    }
                    memberships.push((
                        locations[i].clone(),
                        verify_pack_members(&images[i], batch)?,
                    ));
                }
            }
        }
        drop(native);
        drop(providers);
        drop(images);
        let native_prepared = native_slots
            .iter()
            .map(|i| prepared[*i].try_clone())
            .collect::<Result<Vec<_>>>()?;
        let provider_prepared = provider_slots
            .iter()
            .map(|i| prepared[*i].try_clone())
            .collect::<Result<Vec<_>>>()?;
        let view = PreparedHybridView::from_verified_parts(
            &verified.witness,
            meta,
            native_prepared,
            plan.redirects().to_vec(),
        )?;
        Ok(Self {
            view,
            providers: provider_prepared,
            witness: verified.witness,
            memberships,
        })
    }
    /// Caller must first admit T/M/P through normal CAS copy, including local
    /// manifest hydration where required. One Catalog value implements both
    /// capabilities, preventing accidental registration across two databases.
    /// Member batches first; variant LAST. Cancelling the waiter cannot release
    /// prepared files/pins while the spawned SQL operation is still running.
    pub async fn register<C>(self: Arc<Self>, catalog: Arc<C>) -> Result<String>
    where
        C: NativeFsCatalog + HybridFsCatalog + ?Sized + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| Error::Backend(format!("hybrid registration requires Tokio: {e}")))?;
        runtime
            .spawn(async move {
                for (location, witness) in &self.memberships {
                    catalog.register_pack_members(location, witness).await?;
                }
                catalog.register_hybrid_projection(&self.witness).await
            })
            .await
            .map_err(|e| Error::Backend(format!("hybrid registration task failed: {e}")))?
    }
    pub fn view(&self) -> &PreparedHybridView {
        &self.view
    }
    pub fn membership_batches(&self) -> usize {
        self.memberships.len()
    }
    /// Kernel provider mount/activation remains a separate Host operation.
    /// These exact immutable files retain all leases after this handoff.
    pub fn into_mount_inputs(self) -> (PreparedHybridView, Vec<PreparedImage>) {
        (self.view, self.providers)
    }
}
