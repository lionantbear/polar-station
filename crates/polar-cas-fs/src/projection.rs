//! Bridge the existing sealed Polar ProjectionTree into the native compiler.
//! No second manifest format and no implicit provider/network resolution.
use crate::{
    Image, ImageDescriptor, MetaEntry, MetaKind, MetaOptions, NativeMeta, NativeMetaCompiler,
};
use polar_cas::{
    native_fs::{
        NativeProjection, NativeTreeProfile, VerifiedNativeProjection, NATIVE_PROJECTION_SCHEMA,
    },
    projection::{ProjectionNode, ProjectionTree},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use std::{collections::BTreeMap, fs::File, sync::atomic::AtomicBool};

/// Compile-time binding only. No physical positions; never persisted to SQL.
#[derive(Clone, Debug)]
pub struct PackedFile {
    pub content: ContentRef,
    /// Exact selected pack descriptor; slot numbers are internal to this adapter.
    pub pack: ContentRef,
}

pub fn descriptor(content: &ContentRef) -> Result<ImageDescriptor> {
    if content.id.algorithm() != DigestAlgorithm::Sha256 {
        return Err(Error::MissingDigest);
    }
    Ok(ImageDescriptor {
        sha256: *content.id.digest_bytes(),
        size_bytes: content.size_bytes,
    })
}
pub fn content_ref(image: ImageDescriptor) -> ContentRef {
    ContentRef {
        id: ObjectId::from_sha256(image.sha256),
        size_bytes: image.size_bytes,
    }
}
/// Preserve native failure categories across the CAS adapter boundary.
pub fn cas_error(error: crate::Error) -> Error {
    use polar_cas_fs_sys as sys;
    match error.status {
        sys::PCFS_CORRUPT => Error::Integrity,
        sys::PCFS_LIMIT => Error::TooLarge,
        sys::PCFS_NOT_FOUND => Error::NotFound,
        sys::PCFS_UNSUPPORTED => Error::UnsupportedCapability,
        sys::PCFS_INVALID => Error::Invalid("invalid native EROFS input".into()),
        _ => Error::Backend(format!("native EROFS: {error}")),
    }
}

/// Direct synchronous compiler. Run the entire operation on a blocking worker;
/// NativeMeta handles remain on that worker; builds are synchronous.
/// Success is not CAS publication, fs-verity enablement or mount activation.
pub fn compile_projection(
    tree: &ProjectionTree,
    files: &[PackedFile],
    packs: &[&Image],
    output: &mut File,
    profile: NativeTreeProfile,
    mut options: MetaOptions,
    cancel: &AtomicBool,
) -> Result<VerifiedNativeProjection> {
    let pack_refs: Vec<_> = packs.iter().map(|p| content_ref(p.descriptor())).collect();
    let mut slots = BTreeMap::new();
    for (i, p) in pack_refs.iter().enumerate() {
        if slots.insert(p.id.to_string(), (p, i as u32)).is_some() {
            return Err(Error::Conflict);
        }
    }
    let mut selected = BTreeMap::new();
    for file in files {
        descriptor(&file.content)?;
        let (p, index) = slots
            .get(&file.pack.id.to_string())
            .ok_or(Error::MissingLocal)?;
        if **p != file.pack {
            return Err(Error::Integrity);
        }
        let key = file.content.id.to_string();
        if let Some((old, slot)) = selected.insert(key, (&file.content, *index)) {
            if *old != file.content || slot != *index {
                return Err(Error::Conflict);
            }
        }
    }
    let mut used = std::collections::BTreeSet::new();
    let mut entries = Vec::with_capacity(tree.entries().len().saturating_sub(1));
    for entry in tree.entries() {
        if entry.path.is_empty() {
            continue;
        } // native writer creates root.
        let kind = match &entry.node {
            ProjectionNode::Directory => MetaKind::Directory {
                mode: profile.directory_mode(),
            },
            ProjectionNode::Symlink { target } => MetaKind::Symlink { target },
            ProjectionNode::File {
                content,
                executable,
            } => {
                descriptor(content)?; // Missing SHA alias is explicit, never relabel.
                let (actual, index) = selected
                    .get(&content.id.to_string())
                    .ok_or(Error::MissingLocal)?;
                if *actual != content {
                    return Err(Error::Integrity);
                }
                used.insert(content.id.to_string());
                MetaKind::File {
                    content: descriptor(content)?,
                    pack_index: *index,
                    mode: profile.file_mode(*executable),
                }
            }
        };
        entries.push(MetaEntry {
            path: &entry.path,
            kind,
        });
    }
    if used.len() != selected.len() {
        return Err(Error::Invalid("unused packed file binding".into()));
    }
    options.root_mode = profile.directory_mode();
    options.uid = profile.owner.uid;
    options.gid = profile.owner.gid;
    let result = NativeMetaCompiler::build_cancellable(&entries, packs, output, options, cancel)
        .map_err(cas_error)?;
    let record = NativeProjection {
        schema: NATIVE_PROJECTION_SCHEMA.into(),
        tree: tree.output().root().clone(),
        meta: content_ref(result.meta),
        packs: pack_refs,
        profile,
        compiler: NativeMetaCompiler::fingerprint().into(),
    };
    // Independent traversal of the completed native view before issuing proof.
    // Open the still-owned staging inode as READONLY, not its external path.
    // A dup of O_RDWR is still O_RDWR and is rightly rejected by native open.
    use std::os::fd::AsRawFd;
    let readonly = File::open(format!("/proc/self/fd/{}", output.as_raw_fd()))?;
    verify_projection(tree, &record, &readonly, packs)?;
    VerifiedNativeProjection::from_trusted_adapter(record)
}

/// Restore/import validation. A serialized receipt does not skip this check.
/// Selected files are read and hashed by the native verifier; metadata modes,
/// owners, symlink targets and the full entry count are checked as well.
pub fn verify_projection(
    tree: &ProjectionTree,
    record: &NativeProjection,
    meta_file: &File,
    packs: &[&Image],
) -> Result<VerifiedNativeProjection> {
    record.validate()?;
    if tree.output().root() != &record.tree || packs.len() != record.packs.len() {
        return Err(Error::Integrity);
    }
    for (image, expected) in packs.iter().zip(&record.packs) {
        if content_ref(image.descriptor()) != *expected {
            return Err(Error::Integrity);
        }
    }
    let meta = NativeMeta::open(meta_file, descriptor(&record.meta)?, packs).map_err(cas_error)?;
    // count_entries() includes root, so hidden or extra paths cannot be admitted.
    if meta.entry_count().map_err(cas_error)? != tree.entries().len() as u64 {
        return Err(Error::Integrity);
    }
    for entry in tree.entries() {
        let st = meta.stat(&entry.path).map_err(cas_error)?;
        let (kind, mode) = match &entry.node {
            ProjectionNode::Directory => (1, record.profile.directory_mode()),
            ProjectionNode::File { executable, .. } => (2, record.profile.file_mode(*executable)),
            ProjectionNode::Symlink { .. } => (3, 0o777),
        };
        if st.kind != kind
            || st.mode != mode
            || st.uid != record.profile.owner.uid
            || st.gid != record.profile.owner.gid
        {
            return Err(Error::Integrity);
        }
        match &entry.node {
            ProjectionNode::File { content, .. } => {
                if st.size_bytes != content.size_bytes {
                    return Err(Error::Integrity);
                }
                meta.member(&entry.path, descriptor(content)?)
                    .map_err(cas_error)?
                    .verify()
                    .map_err(cas_error)?;
            }
            ProjectionNode::Symlink { target } => {
                if meta.read_link(&entry.path).map_err(cas_error)? != *target {
                    return Err(Error::Integrity);
                }
            }
            ProjectionNode::Directory => (),
        }
    }
    VerifiedNativeProjection::from_trusted_adapter(record.clone())
}

/// Decode a bounded, explicit member inventory before replica admission.
/// The pack itself must first be persisted via the existing CAS writer.
pub fn verify_pack_members(
    image: &Image,
    members: &[ContentRef],
) -> Result<polar_cas::native_fs::VerifiedPackedMembers> {
    use polar_cas::{
        native_fs::{ErofsPackBinding, VerifiedPackedMembers},
        verification::VerifiedContent,
    };
    if members.len() > polar_cas::catalog::MAX_REGISTER_SOURCES {
        return Err(Error::TooLarge);
    }
    let mut verified = Vec::with_capacity(members.len());
    for member in members {
        let expected = descriptor(member)?;
        image
            .check_native_member(&expected.sha256, expected.size_bytes)
            .map_err(cas_error)?;
        image
            .member(&expected.sha256, expected.size_bytes)
            .map_err(cas_error)?
            .verify()
            .map_err(cas_error)?;
        verified.push(VerifiedContent::from_trusted_admission(
            vec![member.id.clone()],
            member.size_bytes,
        )?);
    }
    VerifiedPackedMembers::from_trusted_adapter(
        ErofsPackBinding::native(content_ref(image.descriptor()))?,
        verified,
    )
}
