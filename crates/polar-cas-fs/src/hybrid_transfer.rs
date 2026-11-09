//! Sender/receiver codec preflight for immutable Hybrid M and packed providers.
//! Transport-plan claims are always compared to the actual decoded M + Tree.
use crate::{
    coverage::{verify_coverage, CoverageBudget},
    hybrid_projection,
    projection::{cas_error, content_ref, descriptor},
    Image,
};
use polar_cas::{
    hybrid_fs::{
        transfer::{bundle_limits, HybridTransferPlan},
        VerifiedHybridProjection,
    },
    projection::{ProjectionLayout, ProjectionTree},
    Error, Result,
};
use std::{
    fs::File,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct VerifiedHybridTransfer {
    pub(crate) witness: VerifiedHybridProjection,
    pub(crate) selected_providers: Vec<usize>,
    pub(crate) tree: ProjectionTree,
}
impl VerifiedHybridTransfer {
    pub fn projection(&self) -> &VerifiedHybridProjection {
        &self.witness
    }
    pub fn tree(&self) -> &ProjectionTree {
        &self.tree
    }
}
/// Call on a blocking worker. Does not mount, enable verity, publish, register,
/// choose a different Meta, or manufacture a ready provider Replica.
pub fn verify_transfer(
    plan: &HybridTransferPlan,
    meta: &File,
    native_packs: &[&Image],
    redirect_packs: &[&Image],
    budget: CoverageBudget,
    cancel: &AtomicBool,
) -> Result<VerifiedHybridTransfer> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Backend(
            "Hybrid transfer validation cancelled".into(),
        ));
    }
    if native_packs.len() != plan.projection().native_packs.len()
        || redirect_packs.len() != plan.redirect_packs().len()
    {
        return Err(Error::Integrity);
    }
    for (image, expected) in redirect_packs.iter().zip(plan.redirect_packs()) {
        if content_ref(image.descriptor()) != *expected {
            return Err(Error::Integrity);
        }
    }
    let tree = ProjectionTree::plan(
        plan.tree().root(),
        plan.tree().directories(),
        &ProjectionLayout::Preserve,
        plan.tree().root().id.algorithm(),
        bundle_limits(&plan.request().options),
    )?;
    let (witness, requirements) =
        hybrid_projection::verify_received(&tree, plan.projection(), meta, native_packs)?;
    if requirements != plan.redirects() {
        return Err(Error::Integrity);
    }
    let wanted: Vec<_> = requirements.iter().map(descriptor).collect::<Result<_>>()?;
    let coverage = verify_coverage(redirect_packs, &wanted, budget, cancel).map_err(cas_error)?;
    Ok(VerifiedHybridTransfer {
        witness,
        selected_providers: coverage.selected().to_vec(),
        tree,
    })
}
