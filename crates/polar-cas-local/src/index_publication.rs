//! Durability batching inside one existing immutable-index publication.
//! Files are synchronized before linking; ancestors are synchronized once,
//! child-first, before the caller may publish the resulting root.
use crate::LocalCas;
use polar_cas::{ContentRef, ObjectId, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) struct IndexPublication<'a> {
    store: &'a LocalCas,
    directories: BTreeSet<PathBuf>,
    content_directories: BTreeSet<PathBuf>,
    pending: BTreeMap<String, ContentRef>,
}
impl<'a> IndexPublication<'a> {
    pub(crate) fn new(store: &'a LocalCas) -> Self {
        Self {
            store,
            directories: BTreeSet::new(),
            content_directories: BTreeSet::new(),
            pending: BTreeMap::new(),
        }
    }
    pub(crate) fn publish(&mut self, bytes: &[u8]) -> Result<ContentRef> {
        let reference = ContentRef {
            id: ObjectId::for_bytes(bytes),
            size_bytes: bytes.len() as u64,
        };
        let mut temporary = self.store.index_temporary()?;
        
        temporary.write_all(bytes)?;
        
        
        temporary.as_file().sync_all()?;
        
        let key = reference.id.content_key();
        let marker = format!(".polar-index-artifacts-v1/{key}");
        let existing = self.store.root.join(&key);
        let source = if self.store.contains_shared(&reference)? {
            if !self.store.is_index_artifact(&reference.id)? {
                crate::shared_index::lifecycle::keep(self.store, &reference)?;
            }
            existing.as_path()
        } else {
            temporary.path()
        };
        self.store.link_file(&reference, &marker, source)?;
        // Create all content directories before syncing the CAS root. Content
        // links themselves must wait until the role namespace is durable.
        self.store
            .dir
            .create_dir_all(Path::new(&key).parent().unwrap())?;
        for parent in Path::new(&marker).parent().unwrap().ancestors() {
            self.directories.insert(if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_owned()
            });
        }
        for parent in Path::new(&key).parent().unwrap().ancestors() {
            if !parent.as_os_str().is_empty() {
                self.content_directories.insert(parent.to_owned());
            }
        }
        self.pending.insert(key, reference.clone());
        Ok(reference)
    }
    pub(crate) fn open_staged(&self, reference: &ContentRef) -> Result<std::fs::File> {
        let marker = format!(".polar-index-artifacts-v1/{}", reference.id.content_key());
        let (file, metadata) =
            crate::admission::files::open_read_file(&self.store.dir, Path::new(&marker))?;
        if metadata.len() != reference.size_bytes {
            return Err(polar_cas::Error::Integrity);
        }
        Ok(file)
    }
    pub(crate) fn finish(&mut self) -> Result<()> {
        self.sync(&self.directories)?;
        for (key, reference) in &self.pending {
            let marker = self
                .store
                .root
                .join(format!(".polar-index-artifacts-v1/{key}"));
            self.store.link_file(reference, key, &marker)?;
            if !self.store.is_index_artifact(&reference.id)? {
                crate::shared_index::lifecycle::keep(self.store, reference)?;
            }
        }
        // CAS-root entries were created and synchronized in the role phase;
        // linking content changes only the existing content ancestors below it.
        self.sync(&self.content_directories)?;
        self.directories.clear();
        self.content_directories.clear();
        self.pending.clear();
        Ok(())
    }
    fn sync(&self, paths: &BTreeSet<PathBuf>) -> Result<()> {
        let mut directories: Vec<_> = paths.iter().collect();
        directories.sort_by_key(|path| {
            std::cmp::Reverse(if path.as_path() == Path::new(".") {
                0
            } else {
                path.components().count()
            })
        });
        for path in directories {
            
            self.store.sync_directory(path)?;
            
        }
        Ok(())
    }
}
