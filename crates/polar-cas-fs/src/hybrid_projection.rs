//! Synchronous codec for a runtime-admitted frozen plan. No Catalog/network IO.
//! An imported variant is verified against its exact plan, not only a summary.
use crate::{
    projection::{cas_error, content_ref, descriptor},
    CasImageLease, HybridMeta, HybridMetaCompiler, Image, MetaEntry, MetaKind, MetaOptions,
};
use polar_cas::{
    hybrid_fs::{HybridCompilePlan, HybridProjection, VerifiedHybridProjection, HYBRID_SCHEMA},
    projection::{ProjectionNode, ProjectionTree},
    ContentRef, Error, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    os::fd::AsRawFd,
    sync::atomic::AtomicBool,
};

// Admission is per exact member, not a pack-wide boolean inferred from blob ownership.
type TrustedMembers = BTreeSet<(usize, [u8; 32], u64)>;

pub fn compile(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    packs: &[&Image],
    output: &mut File,
    options: MetaOptions,
    cancel: &AtomicBool,
) -> Result<VerifiedHybridProjection> {
    compile_inner(tree, plan, packs, output, options, cancel, None)
}

/// Compile immutable leased inputs without a privileged mount preparation.
/// Original generated inodes and exact attached member witnesses reuse
/// admission evidence. Managed blob ownership alone never proves members.
/// The existing Image-only compile entrypoint is deliberately still strict.
/// Retained snapshots across the plan are capped at MAX_COMPILE_SNAPSHOT_BYTES;
/// callers must enforce this cumulative bound while acquiring inputs as well.
pub fn compile_with_leases(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    packs: &[CasImageLease],
    output: &mut File,
    options: MetaOptions,
    cancel: &AtomicBool,
) -> Result<VerifiedHybridProjection> {
    plan.validate()?;
    if packs.len() != plan.native_packs.len() {
        return Err(Error::Integrity);
    }
    let mut snapshot_bytes = 0u64;
    for pack in packs {
        snapshot_bytes = snapshot_bytes
            .checked_add(pack.snapshot_bytes())
            .ok_or(Error::TooLarge)?;
        if snapshot_bytes > crate::MAX_COMPILE_SNAPSHOT_BYTES {
            return Err(Error::TooLarge);
        }
    }
    let mut images = Vec::with_capacity(packs.len());
    for (pack, expected) in packs.iter().zip(&plan.native_packs) {
        if pack.content() != expected {
            return Err(Error::Integrity);
        }
        images.push(Image::open_leased(pack)?);
    }
    let refs: Vec<_> = images.iter().collect();
    let mut trusted = TrustedMembers::new();
    for mapping in &plan.mappings {
        if let Some(pack) = &mapping.pack {
            let slot = plan
                .native_packs
                .iter()
                .position(|p| p == pack)
                .ok_or(Error::Integrity)?;
            if packs[slot].has_trusted_member(&mapping.content) {
                trusted.insert((
                    slot,
                    *mapping.content.id.digest_bytes(),
                    mapping.content.size_bytes,
                ));
            }
        }
    }
    compile_inner(tree, plan, &refs, output, options, cancel, Some(&trusted))
}

#[allow(clippy::too_many_arguments)]
fn compile_inner(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    packs: &[&Image],
    output: &mut File,
    mut options: MetaOptions,
    cancel: &AtomicBool,
    trusted: Option<&TrustedMembers>,
) -> Result<VerifiedHybridProjection> {
    plan.validate()?;
    if &plan.tree != tree.output().root() || packs.len() != plan.native_packs.len() {
        return Err(Error::Integrity);
    }
    for (p, e) in packs.iter().zip(&plan.native_packs) {
        if content_ref(p.descriptor()) != *e {
            return Err(Error::Integrity);
        }
    }
    let choices: BTreeMap<_, _> = plan
        .mappings
        .iter()
        .map(|m| (m.content.id.to_string(), m))
        .collect();
    let slots: BTreeMap<_, _> = plan
        .native_packs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.to_string(), i as u32))
        .collect();
    let mut entries = Vec::new();
    let mut used = BTreeSet::new();
    let (mut native, mut redirect) = (0, 0);
    for e in tree.entries() {
        if e.path.is_empty() {
            continue;
        }
        let kind = match &e.node {
            ProjectionNode::Directory => MetaKind::Directory {
                mode: plan.profile.directory_mode(),
            },
            ProjectionNode::Symlink { target } => MetaKind::Symlink { target },
            ProjectionNode::File {
                content,
                executable,
            } => {
                let m = choices
                    .get(&content.id.to_string())
                    .ok_or(Error::Integrity)?;
                if &m.content != content {
                    return Err(Error::Integrity);
                }
                used.insert(content.id.to_string());
                let mode = plan.profile.file_mode(*executable);
                match &m.pack {
                    Some(p) => {
                        native += 1;
                        MetaKind::File {
                            content: descriptor(content)?,
                            pack_index: *slots.get(&p.id.to_string()).ok_or(Error::Integrity)?,
                            mode,
                        }
                    }
                    None => {
                        redirect += 1;
                        MetaKind::Redirect {
                            content: descriptor(content)?,
                            mode,
                        }
                    }
                }
            }
        };
        entries.push(MetaEntry {
            path: &e.path,
            kind,
        });
    }
    if used.len() != choices.len() {
        return Err(Error::Integrity);
    }
    options.uid = plan.profile.owner.uid;
    options.gid = plan.profile.owner.gid;
    options.root_mode = plan.profile.directory_mode();
    let leased = trusted.is_some();
    if leased {
        // Mixed leased inputs audit only untrusted selected members. A managed
        // pack is never decoded merely because another input is an import.
        for entry in &entries {
            if let MetaKind::File {
                content,
                pack_index,
                ..
            } = &entry.kind
            {
                let slot = *pack_index as usize;
                if !trusted.is_some_and(|members| {
                    members.contains(&(slot, content.sha256, content.size_bytes))
                }) {
                    packs[slot]
                        .member(&content.sha256, content.size_bytes)
                        .map_err(cas_error)?
                        .verify()
                        .map_err(cas_error)?;
                }
            }
        }
    }
    let built = if leased {
        HybridMetaCompiler::build_trusted(&entries, packs, output, options, cancel)
    } else {
        HybridMetaCompiler::build(&entries, packs, output, options, cancel)
    }
    .map_err(cas_error)?;
    let record = HybridProjection {
        schema: HYBRID_SCHEMA.into(),
        tree: plan.tree.clone(),
        meta: content_ref(built.meta),
        native_packs: plan.native_packs.clone(),
        profile: plan.profile.clone(),
        plan_fingerprint: plan.fingerprint()?,
        compiler: HybridMetaCompiler::fingerprint().into(),
        native_file_count: native,
        redirect_file_count: redirect,
    };
    let readonly = File::open(format!("/proc/self/fd/{}", output.as_raw_fd()))?;
    verify_inner(tree, plan, &record, &readonly, packs, trusted, true)
}

/// Checks exact namespace, mapping mode, selected pack slots, metadata and native
/// bytes. Redirects are validated as requirements; Host validates provider bytes.
/// Plan fingerprint alone is not a semantic witness.
pub fn verify(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    r: &HybridProjection,
    file: &File,
    packs: &[&Image],
) -> Result<VerifiedHybridProjection> {
    verify_inner(tree, plan, r, file, packs, None, false)
}

#[allow(clippy::too_many_arguments)]
fn verify_inner(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    r: &HybridProjection,
    file: &File,
    packs: &[&Image],
    trusted: Option<&TrustedMembers>,
    generated: bool,
) -> Result<VerifiedHybridProjection> {
    r.validate()?;
    plan.validate()?;
    if &r.tree != tree.output().root()
        || r.tree != plan.tree
        || r.profile != plan.profile
        || r.plan_fingerprint != plan.fingerprint()?
        || r.native_packs != plan.native_packs
        || packs.len() != r.native_packs.len()
    {
        return Err(Error::Integrity);
    }
    for (p, e) in packs.iter().zip(&r.native_packs) {
        if content_ref(p.descriptor()) != *e {
            return Err(Error::Integrity);
        }
    }
    let meta = if generated {
        HybridMeta::open_generated(file, descriptor(&r.meta)?, packs)
    } else {
        HybridMeta::open(file, descriptor(&r.meta)?, packs)
    }
    .map_err(cas_error)?;
    if meta.entry_count().map_err(cas_error)? != tree.entries().len() as u64 {
        return Err(Error::Integrity);
    }
    let choices: BTreeMap<_, _> = plan
        .mappings
        .iter()
        .map(|m| (m.content.id.to_string(), m))
        .collect();
    let mut used = BTreeSet::new();
    let (mut native, mut redirect) = (0u64, 0u64);
    for e in tree.entries() {
        let st = meta.stat(&e.path).map_err(cas_error)?;
        if st.uid != r.profile.owner.uid || st.gid != r.profile.owner.gid {
            return Err(Error::Integrity);
        }
        match &e.node {
            ProjectionNode::Directory => {
                if st.kind != 1 || st.mode != r.profile.directory_mode() {
                    return Err(Error::Integrity);
                }
            }
            ProjectionNode::Symlink { target } => {
                if st.kind != 3
                    || st.mode != 0o777
                    || meta.read_link(&e.path).map_err(cas_error)? != *target
                {
                    return Err(Error::Integrity);
                }
            }
            ProjectionNode::File {
                content,
                executable,
            } => {
                if st.size_bytes != content.size_bytes
                    || st.mode != r.profile.file_mode(*executable)
                {
                    return Err(Error::Integrity);
                }
                let choice = choices
                    .get(&content.id.to_string())
                    .ok_or(Error::Integrity)?;
                if choice.content != *content {
                    return Err(Error::Integrity);
                }
                used.insert(content.id.to_string());
                match &choice.pack {
                    Some(expected_pack) => {
                        if st.kind != 2 {
                            return Err(Error::Integrity);
                        }
                        let slot = r
                            .native_packs
                            .iter()
                            .position(|p| p == expected_pack)
                            .ok_or(Error::Integrity)?;
                        match meta.native_pack_slot(&e.path).map_err(cas_error)? {
                            Some(actual) if actual as usize == slot => (),
                            None if content.size_bytes == 0 => {
                                packs[slot]
                                    .check_native_member(content.id.digest_bytes(), 0)
                                    .map_err(cas_error)?;
                            }
                            _ => return Err(Error::Integrity),
                        }
                        let member = meta
                            .member(&e.path, descriptor(content)?)
                            .map_err(cas_error)?;
                        if !trusted.is_some_and(|members| {
                            members.contains(&(
                                slot,
                                *content.id.digest_bytes(),
                                content.size_bytes,
                            ))
                        }) {
                            member.verify().map_err(cas_error)?;
                        }
                        native += 1;
                    }
                    None => {
                        if st.kind != 4
                            || meta.redirect(&e.path).map_err(cas_error)? != descriptor(content)?
                        {
                            return Err(Error::Integrity);
                        }
                        redirect += 1;
                    }
                }
            }
        }
    }
    if used.len() != choices.len()
        || native != r.native_file_count
        || redirect != r.redirect_file_count
    {
        return Err(Error::Integrity);
    }
    VerifiedHybridProjection::from_trusted_adapter(r.clone())
}

pub fn redirect_requirements(
    tree: &ProjectionTree,
    plan: &HybridCompilePlan,
    r: &HybridProjection,
    file: &File,
    packs: &[&Image],
) -> Result<Vec<ContentRef>> {
    verify(tree, plan, r, file, packs)?;
    Ok(plan.redirects().cloned().collect())
}

/// Receive/reopen without retaining the compiler's full mapping request. The
/// actual per-content binding is reconstructed from M's inode mappings/xattrs,
/// checked against T and hashed into the same normalized plan fingerprint.
/// Empty native files have no extent/slot and are normalized to native-empty.
pub fn verify_received(
    tree: &ProjectionTree,
    r: &HybridProjection,
    file: &File,
    packs: &[&Image],
) -> Result<(VerifiedHybridProjection, Vec<ContentRef>)> {
    use polar_cas::hybrid_fs::{mapping_fingerprint, HybridMapping};
    r.validate()?;
    if &r.tree != tree.output().root() || packs.len() != r.native_packs.len() {
        return Err(Error::Integrity);
    }
    for (p, e) in packs.iter().zip(&r.native_packs) {
        if content_ref(p.descriptor()) != *e {
            return Err(Error::Integrity);
        }
    }
    let meta = HybridMeta::open(file, descriptor(&r.meta)?, packs).map_err(cas_error)?;
    if meta.entry_count().map_err(cas_error)? != tree.entries().len() as u64 {
        return Err(Error::Integrity);
    }
    let (mut native, mut redirected) = (0u64, 0u64);
    let mut observed: BTreeMap<String, HybridMapping> = BTreeMap::new();
    for e in tree.entries() {
        let st = meta.stat(&e.path).map_err(cas_error)?;
        if st.uid != r.profile.owner.uid || st.gid != r.profile.owner.gid {
            return Err(Error::Integrity);
        }
        match &e.node {
            ProjectionNode::Directory => {
                if st.kind != 1 || st.mode != r.profile.directory_mode() {
                    return Err(Error::Integrity);
                }
            }
            ProjectionNode::Symlink { target } => {
                if st.kind != 3
                    || st.mode != 0o777
                    || meta.read_link(&e.path).map_err(cas_error)? != *target
                {
                    return Err(Error::Integrity);
                }
            }
            ProjectionNode::File {
                content,
                executable,
            } => {
                if st.size_bytes != content.size_bytes
                    || st.mode != r.profile.file_mode(*executable)
                {
                    return Err(Error::Integrity);
                }
                let source = match st.kind {
                    4 => {
                        if meta.redirect(&e.path).map_err(cas_error)? != descriptor(content)? {
                            return Err(Error::Integrity);
                        }
                        redirected += 1;
                        None
                    }
                    2 => {
                        let slot = meta.native_pack_slot(&e.path).map_err(cas_error)?;
                        let p = match slot {
                            Some(i) => r.native_packs.get(i as usize).ok_or(Error::Integrity)?,
                            None if content.size_bytes == 0 => {
                                r.native_packs.first().ok_or(Error::Integrity)?
                            }
                            _ => return Err(Error::Integrity),
                        };
                        meta.member(&e.path, descriptor(content)?)
                            .map_err(cas_error)?
                            .verify()
                            .map_err(cas_error)?;
                        native += 1;
                        Some(p.clone())
                    }
                    _ => return Err(Error::Integrity),
                };
                let mapping = HybridMapping {
                    content: content.clone(),
                    pack: source,
                };
                if let Some(previous) = observed.insert(content.id.to_string(), mapping.clone()) {
                    if previous != mapping {
                        return Err(Error::Integrity);
                    }
                }
            }
        }
    }
    let mappings: Vec<_> = observed.into_values().collect();
    if native != r.native_file_count
        || redirected != r.redirect_file_count
        || mapping_fingerprint(&r.tree, &r.profile, &r.native_packs, &mappings)?
            != r.plan_fingerprint
    {
        return Err(Error::Integrity);
    }
    let requirements = mappings
        .iter()
        .filter(|m| m.pack.is_none())
        .map(|m| m.content.clone())
        .collect();
    Ok((
        VerifiedHybridProjection::from_trusted_adapter(r.clone())?,
        requirements,
    ))
}
