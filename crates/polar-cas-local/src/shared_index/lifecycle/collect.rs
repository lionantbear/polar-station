//! Conservative collection of dependencies of explicitly managed historical roots.
use super::registry::Registry;
use super::*;
use crate::shared_index::partitions::Root;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Read,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
};



#[derive(Default, Debug, serde::Serialize)]
pub struct CollectionReceipt {
    pub busy: bool,
    pub removed_roots: usize,
    pub removed_objects: usize,
    pub removed_bytes: u64,
}
fn insert(map: &mut BTreeMap<String, ContentRef>, reference: &ContentRef) -> Result<()> {
    if map
        .insert(reference.id.to_string(), reference.clone())
        .is_some_and(|old| old != *reference)
    {
        return Err(Error::Integrity);
    }
    Ok(())
}
fn dependencies(root: &Root) -> Result<BTreeMap<String, ContentRef>> {
    let mut refs = BTreeMap::new();
    for part in root.partitions.values() {
        insert(&mut refs, part.portable_file())?;
        if part.storage.is_none() {
            insert(&mut refs, &part.native)?;
        }
    }
    Ok(refs)
}
fn missing(error: &Error) -> bool {
    matches!(error, Error::NotFound)
        || matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
}
fn role(store: &LocalCas, reference: &ContentRef) -> Result<Option<std::fs::Metadata>> {
    let path = format!(".polar-index-artifacts-v1/{}", reference.id.content_key());
    match crate::admission::files::open_read_file(&store.dir, Path::new(&path)) {
        Ok((file, _)) => {
            let metadata = file.metadata()?;
            if metadata.len() != reference.size_bytes {
                return Err(Error::Integrity);
            }
            Ok(Some(metadata))
        }
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}
fn root_roles(store: &LocalCas) -> Result<BTreeMap<String, ContentRef>> {
    let mut roots = BTreeMap::new();
    let role_dir = match directory(&store.dir, ".polar-index-artifacts-v1/sha256") {
        Ok(directory) => directory,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(roots),
        Err(e) => return Err(e.into()),
    };
    for shard in role_dir.entries()? {
        let shard = shard?;
        let name = shard.file_name();
        let shard_dir = directory(&role_dir, &name)?;
        for entry in shard_dir.entries()? {
            let entry = entry?;
            if roots.len() >= 1_000_000 {
                return Err(Error::TooLarge);
            }
            if !entry.file_type()?.is_file() {
                return Err(Error::Integrity);
            }
            let key = format!(
                "sha256/{}/{}",
                name.to_string_lossy(),
                entry.file_name().to_string_lossy()
            );
            let id = polar_cas::ObjectId::from_content_key(&key)?;
            let marker = format!(".polar-index-artifacts-v1/{key}");
            let (mut file, metadata) =
                crate::admission::files::open_read_file(&store.dir, Path::new(&marker))?;
            let mut prefix = [0; 8];
            if metadata.len() >= 8 {
                file.read_exact(&mut prefix)?;
            }
            // Old JSON roots are retained; other artifact formats aren't roots.
            if &prefix == b"PCASROT2" || prefix.first() == Some(&b'{') {
                let reference = ContentRef {
                    id,
                    size_bytes: metadata.len(),
                };
                insert(&mut roots, &reference)?;
            }
        }
    }
    Ok(roots)
}

pub(super) fn collect(store: &LocalCas) -> Result<CollectionReceipt> {
    let file = guard_file(store)?;
    match file.try_lock() {
        Ok(()) => (),
        Err(std::fs::TryLockError::WouldBlock) => {
            return Ok(CollectionReceipt {
                busy: true,
                ..Default::default()
            })
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let mut registry = Registry::open(store)?;
    if registry.owned.is_empty() {
        return Ok(CollectionReceipt::default());
    }
    let mut roots = root_roles(store)?;
    for reference in registry.owned.values() {
        insert(&mut roots, reference)?;
    }
    let mut live: BTreeSet<String> = registry.kept.keys().cloned().collect();
    for (path, last) in &registry.heads {
        let path = PathBuf::from(OsString::from_vec(path.clone()));
        let current = crate::catalog_index::head::Head::current_at(&path)?;
        if current.is_none() && last.is_some() {
            return Err(Error::Integrity);
        }
        if let Some(reference) = current {
            insert(&mut roots, &reference)?;
            live.insert(reference.id.to_string());
        }
    }
    let mut closures = BTreeMap::new();
    let mut candidates = BTreeMap::new();
    for (id, reference) in &roots {
        let owned = registry.owned.contains_key(id);
        if owned {
            insert(&mut candidates, reference)?;
        }
        let root = match Root::read(store, reference) {
            Ok(root) => root,
            // Roots are unlinked only after their dead dependencies are synced.
            Err(error) if owned && missing(&error) && !live.contains(id) => continue,
            Err(error) => return Err(error),
        };
        let dependencies = dependencies(&root)?;
        if owned {
            for reference in dependencies.values() {
                insert(&mut candidates, reference)?;
            }
        } else {
            live.insert(id.clone());
        }
        let root_file = store.open_shared_file(reference)?.ok_or(Error::NotFound)?;
        if root_file.metadata()?.nlink() > 2 {
            live.insert(id.clone());
        }
        match root_file.try_lock() {
            // The exclusive acquisition guard above prevents new root leases
            // until collection finishes. Probe one root at a time; retaining
            // these descriptors would exhaust RLIMIT_NOFILE as history grows.
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => {
                live.insert(id.clone());
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        closures.insert(id.clone(), dependencies);
    }
    // Follow root-to-root dependencies too, without interpreting opaque values.
    let mut pending: Vec<_> = live.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        if let Some(dependencies) = closures.get(&id) {
            for reference in dependencies.values() {
                if live.insert(reference.id.to_string()) {
                    pending.push(reference.id.to_string());
                }
            }
        }
    }
    // Preflight every removal and every selected dependency before any unlink.
    // Historical roots commonly share unchanged immutable segments. Validate
    // each content identity once, without weakening length consistency checks.
    let mut checked = BTreeMap::new();
    for (id, dependencies) in &closures {
        if live.contains(id) {
            for reference in dependencies.values() {
                let key = reference.id.to_string();
                if let Some(previous) = checked.get(&key) {
                    if previous != reference {
                        return Err(Error::Integrity);
                    }
                    continue;
                }
                
                store.open_shared_file(reference)?.ok_or(Error::NotFound)?;
                checked.insert(key, reference.clone());
            }
        }
    }
    let mut remove = Vec::new();
    let mut gone = BTreeSet::new();
    for (id, reference) in &candidates {
        if live.contains(id) {
            continue;
        }
        let Some(marker) = role(store, reference)? else {
            if registry.owned.contains_key(id) && store.open_shared_file(reference)?.is_none() {
                gone.insert(id.clone());
            }
            continue;
        };
        if marker.nlink() > 2 {
            continue;
        }
        if let Some(file) = store.open_shared_file(reference)? {
            let actual = file.metadata()?;
            if actual.dev() != marker.dev() || actual.ino() != marker.ino() {
                return Err(Error::Integrity);
            }
        }
        remove.push(reference.clone());
    }
    let mut receipt = CollectionReceipt::default();
    // Crash ordering: first make all segment removals durable; only then remove
    // the roots which describe them. The ledger can lag and be reconciled later.
    for root_phase in [false, true] {
        let mut directories = BTreeSet::new();
        for reference in remove
            .iter()
            .filter(|r| roots.contains_key(&r.id.to_string()) == root_phase)
        {
            for key in [
                reference.id.content_key(),
                format!(".polar-index-artifacts-v1/{}", reference.id.content_key()),
            ] {
                match store.dir.remove_file(&key) {
                    Ok(()) => {
                        directories
                            .insert(Path::new(&key).parent().ok_or(Error::Integrity)?.to_owned());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
            receipt.removed_objects += 1;
            receipt.removed_bytes += reference.size_bytes;
            if root_phase {
                receipt.removed_roots += 1;
            }
        }
        for directory in directories {
            super::directory(&store.dir, directory)?
                .open(".")?
                .sync_all()?;
        }
        
    }
    gone.extend(remove.iter().map(|r| r.id.to_string()));
    registry.owned.retain(|id, _| !gone.contains(id));
    if !gone.is_empty() {
        registry.save()?;
    }
    Ok(receipt)
}

