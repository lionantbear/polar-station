//! Decode all present redirect copies, not just the first one that resolves.
//! Used by sender and receiver preflight; the kernel and Catalog remain separate.
use crate::{erofs::check, Error, Image, ImageDescriptor};
use polar_cas_fs_sys as sys;
use std::{
    ffi::c_void,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub struct CoverageBudget {
    pub max_probes: u64,
    pub max_verified_bytes: u64,
}
impl Default for CoverageBudget {
    fn default() -> Self {
        Self {
            max_probes: 4_000_000,
            max_verified_bytes: 64 << 30,
        }
    }
}
/// An ephemeral provider choice after full validation. Not a database map or a
/// proof that the underlying inode cannot be changed by another process.
#[derive(Debug)]
pub struct VerifiedCoverage {
    selected: Vec<usize>,
    pub probes: u64,
    pub verified_bytes: u64,
}
impl VerifiedCoverage {
    pub fn selected(&self) -> &[usize] {
        &self.selected
    }
}
unsafe extern "C" fn cancelled(context: *mut c_void) -> i32 {
    // SAFETY: verify_coverage borrows this AtomicBool for the synchronous call.
    i32::from(unsafe { &*context.cast::<AtomicBool>() }.load(Ordering::Relaxed))
}
pub fn verify_coverage(
    providers: &[&Image],
    requirements: &[ImageDescriptor],
    budget: CoverageBudget,
    cancel: &AtomicBool,
) -> Result<VerifiedCoverage, Error> {
    if providers.len() > 128 || requirements.len() > 1_000_000 {
        return Err(Error {
            status: sys::PCFS_LIMIT,
        });
    }
    let handles: Vec<_> = providers.iter().map(|p| p.handle.as_ptr()).collect();
    let wanted: Vec<_> = requirements
        .iter()
        .map(|c| sys::pcfs_required_content_v1 {
            sha256: c.sha256,
            size_bytes: c.size_bytes,
        })
        .collect();
    let mut selected = vec![u32::MAX; requirements.len()];
    let mut result = sys::pcfs_coverage_result_v1::default();
    // SAFETY: all arrays and handles live for this synchronous call; native
    // result is private until success. C validates uniqueness, bounds and bytes.
    check(unsafe {
        sys::pcfs_pack_verify_coverage(
            handles.as_ptr(),
            handles.len(),
            wanted.as_ptr(),
            wanted.len(),
            budget.max_probes,
            budget.max_verified_bytes,
            Some(cancelled),
            (cancel as *const AtomicBool).cast_mut().cast(),
            selected.as_mut_ptr(),
            &mut result,
        )
    })?;
    if result.covered != requirements.len() as u64
        || selected.iter().any(|i| *i as usize >= providers.len())
    {
        return Err(Error {
            status: sys::PCFS_CORRUPT,
        });
    }
    Ok(VerifiedCoverage {
        selected: selected.into_iter().map(|n| n as usize).collect(),
        probes: result.probes,
        verified_bytes: result.verified_bytes,
    })
}
