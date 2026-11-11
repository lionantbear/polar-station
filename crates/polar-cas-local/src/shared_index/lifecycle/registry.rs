//! One bounded binary ledger for managed heads and exceptional retained bytes.
use super::*;
use crate::shared_index::binary::{Decoder, Encoder};
use cap_std::fs::Dir;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    path::Path,
};
const LIMIT: usize = 128 << 20;
const COUNT: usize = 1_000_000;
const MAGIC: &[u8] = b"PCASLIF1";

pub(super) struct Registry {
    pub heads: BTreeMap<Vec<u8>, Option<ContentRef>>,
    pub owned: BTreeMap<String, ContentRef>,
    pub kept: BTreeMap<String, ContentRef>,
    dir: Dir,
    _writer: File,
}
impl Registry {
    pub fn open(store: &LocalCas) -> Result<Self> {
        let dir = directory(&store.dir, ".polar-index-artifacts-v1")?;
        let writer = File::from(
            openat(
                &dir,
                "retention.lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        if !writer.metadata()?.is_file() {
            return Err(Error::Integrity);
        }
        writer.lock()?;
        let mut state = Self {
            heads: BTreeMap::new(),
            owned: BTreeMap::new(),
            kept: BTreeMap::new(),
            dir,
            _writer: writer,
        };
        let file = match openat(
            &state.dir,
            "retention.bin",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(state),
            Err(e) => return Err(std::io::Error::from(e).into()),
        };
        if !file.metadata()?.is_file() || file.metadata()?.len() > LIMIT as u64 {
            return Err(Error::Integrity);
        }
        let mut bytes = Vec::new();
        file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() < 32 {
            return Err(Error::Integrity);
        }
        let (data, digest) = bytes.split_at(bytes.len() - 32);
        if polar_cas::ObjectId::for_bytes(data).digest_bytes() != digest {
            return Err(Error::Integrity);
        }
        let mut r = Decoder::new(data, MAGIC, LIMIT)?;
        let heads = r.count()?;
        if heads > COUNT {
            return Err(Error::TooLarge);
        }
        for _ in 0..heads {
            let size = r.count()?;
            if size > 4096 {
                return Err(Error::TooLarge);
            }
            let path = r.raw(size)?.to_vec();
            let value = match r.byte()? {
                0 => None,
                1 => Some(r.content()?),
                _ => return Err(Error::Integrity),
            };
            if !path.starts_with(b"/") || state.heads.insert(path, value).is_some() {
                return Err(Error::Integrity);
            }
        }
        for map in [&mut state.owned, &mut state.kept] {
            let count = r.count()?;
            if count > COUNT {
                return Err(Error::TooLarge);
            }
            for _ in 0..count {
                let reference = r.content()?;
                reference.id.require_sha256()?;
                if map.insert(reference.id.to_string(), reference).is_some() {
                    return Err(Error::Integrity);
                }
            }
        }
        r.finish()?;
        Ok(state)
    }
    pub fn save(&self) -> Result<()> {
        if [self.heads.len(), self.owned.len(), self.kept.len()]
            .into_iter()
            .any(|n| n > COUNT)
        {
            return Err(Error::TooLarge);
        }
        let mut w = Encoder::new(MAGIC, LIMIT - 32)?;
        w.count(self.heads.len())?;
        for (path, root) in &self.heads {
            w.count(path.len())?;
            w.raw(path)?;
            w.byte(u8::from(root.is_some()))?;
            if let Some(root) = root {
                w.content(root)?;
            }
        }
        for map in [&self.owned, &self.kept] {
            w.count(map.len())?;
            for reference in map.values() {
                w.content(reference)?;
            }
        }
        let mut bytes = w.finish();
        let digest = polar_cas::ObjectId::for_bytes(&bytes);
        bytes.extend_from_slice(digest.digest_bytes());
        match self.dir.remove_file("retention.pending") {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let mut file = File::from(
            openat(
                &self.dir,
                "retention.pending",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        
        file.write_all(&bytes)?;
        file.sync_all()?;
        ;
        self.dir
            .rename("retention.pending", &self.dir, "retention.bin")?;
        self.dir.open(".")?.sync_all()?;
        Ok(())
    }
}

pub(crate) fn register_head(
    store: &LocalCas,
    path: &Path,
    current: Option<ContentRef>,
) -> Result<()> {
    let _acquisition = acquire(store)?;
    let mut registry = Registry::open(store)?;
    let key = path.as_os_str().as_bytes().to_vec();
    if let std::collections::btree_map::Entry::Vacant(entry) = registry.heads.entry(key) {
        entry.insert(current);
        registry.save()?;
    }
    Ok(())
}
pub(crate) fn selecting(store: &LocalCas, path: &Path, root: &ContentRef) -> Result<()> {
    let mut registry = Registry::open(store)?;
    let key = path.as_os_str().as_bytes().to_vec();
    let selected = registry.heads.get_mut(&key).ok_or(Error::Integrity)?;
    if selected.as_ref() == Some(root) && registry.owned.contains_key(&root.id.to_string()) {
        return Ok(());
    }
    *selected = Some(root.clone());
    registry.owned.insert(root.id.to_string(), root.clone());
    registry.save()
}
/// Caller holds acquisition; only exceptional index/business collisions enroll.
pub(crate) fn keep(store: &LocalCas, reference: &ContentRef) -> Result<()> {
    let mut registry = Registry::open(store)?;
    if let Some(old) = registry.kept.get(&reference.id.to_string()) {
        if old != reference {
            return Err(Error::Integrity);
        }
        return Ok(());
    }
    registry
        .kept
        .insert(reference.id.to_string(), reference.clone());
    registry.save()
}
