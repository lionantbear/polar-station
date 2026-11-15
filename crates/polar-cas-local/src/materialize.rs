//! Immutable directory publications assembled from proven local files.
use crate::admission::{FileMode, LocalFile};
use cap_std::{ambient_authority, fs::Dir};
use polar_cas::{
    canonical::canonical_json_bytes,
    projection::{
        FilePlacement, ProjectionNode, ProjectionProfile, ProjectionStatistics, ProjectionTree,
        TreeProjectionRecord, LOCAL_TREE_SCHEMA,
    },
    source::SourceLocator,
    Error, Result,
};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
mod driver;
mod filesystem;
mod inspect;
mod observer;
pub use observer::LocalTreeObserver;
pub use driver::NativeMaterializer;
use filesystem::{Stage, Target};

#[derive(Clone)]
pub struct LocalTreeRequest {
    tree: ProjectionTree,
    target: SourceLocator,
    profile: ProjectionProfile,
    files: BTreeMap<(String, bool), LocalFile>,
    _owner: Arc<dyn Send + Sync>,
}
impl LocalTreeRequest {
    pub fn new(
        tree: ProjectionTree,
        target: SourceLocator,
        profile: ProjectionProfile,
        files: Vec<LocalFile>,
    ) -> Result<Self> {
        crate::native::validate_artifact(&target)?;
        if files.len() > tree.limits().tree.max_entries {
            return Err(Error::TooLarge);
        }
        let mut indexed: BTreeMap<(String, bool), LocalFile> = BTreeMap::new();
        for file in files {
            for id in file.content().ids() {
                let key = (id.to_string(), file.mode() == FileMode::Executable);
                if indexed
                    .get(&key)
                    .is_some_and(|old| old.path() != file.path())
                {
                    return Err(Error::Conflict);
                }
                indexed.insert(key, file.clone());
            }
        }
        let request = Self {
            tree,
            target,
            profile,
            files: indexed,
            _owner: Arc::new(()),
        };
        request.validate_files()?;
        Ok(request)
    }
    /// Keep an execution guard alive through blocking filesystem I/O, including
    /// when the async caller is cancelled. Does not create a worker or queue.
    pub fn retain(self, owner: impl Send + Sync + 'static) -> Self {
        Self {
            _owner: Arc::new((self._owner, owner)),
            ..self
        }
    }
    fn file(&self, content: &polar_cas::ContentRef, executable: bool) -> Result<&LocalFile> {
        self.files
            .get(&(content.id.to_string(), executable))
            .ok_or(Error::NotFound)
    }
    fn validate_files(&self) -> Result<()> {
        for entry in self.tree.entries() {
            if let ProjectionNode::File {
                content,
                executable,
            } = &entry.node
            {
                self.file(content, *executable)?.validate_for(
                    content,
                    if *executable {
                        FileMode::Executable
                    } else {
                        FileMode::ReadOnly
                    },
                    self.profile.owner,
                )?;
            }
        }
        Ok(())
    }
}
pub struct NativeProjection {
    root: PathBuf,
    record: TreeProjectionRecord,
    reused: bool,
    statistics: ProjectionStatistics,
}
impl NativeProjection {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn record(&self) -> &TreeProjectionRecord {
        &self.record
    }
    pub fn reused(&self) -> bool {
        self.reused
    }
    pub fn statistics(&self) -> &ProjectionStatistics {
        &self.statistics
    }
}
#[derive(Clone)]
pub struct LocalTreePublisher {
    root: PathBuf,
    dir: Arc<Dir>,
}
impl LocalTreePublisher {
    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::Invalid("absolute projection root required".into()));
        }
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let dir = Arc::new(Dir::open_ambient_dir(&root, ambient_authority())?);
        Ok(Self { root, dir })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn read_observer(&self) -> LocalTreeObserver {
        LocalTreeObserver::from_publisher(self.clone())
    }
    pub async fn materialize_tree(&self, request: LocalTreeRequest) -> Result<NativeProjection> {
        let publisher = self.clone();
        tokio::task::spawn_blocking(move || publisher.publish(&request))
            .await
            .map_err(|_| Error::OutcomeUncertain)?
    }
    pub async fn inspect_tree(
        &self,
        request: LocalTreeRequest,
    ) -> Result<Option<NativeProjection>> {
        let publisher = self.clone();
        tokio::task::spawn_blocking(move || {
            request.validate_files()?;
            let target = match Target::open(&publisher, &request, false) {
                Ok(target) => target,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None)
                }
                Err(error) => return Err(error),
            };
            publisher.inspect(&request, &target)
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }
    fn publish(&self, request: &LocalTreeRequest) -> Result<NativeProjection> {
        filesystem::require_atomic_rename()?;
        request.validate_files()?;
        let target = Target::open(self, request, true)?;
        if let Some(existing) = self.inspect(request, &target)? {
            return Ok(existing);
        }
        let mut stage = Stage::new(&target.parent_path)?;
        stage.create_directory(Path::new("data"))?;
        for entry in request
            .tree
            .entries()
            .iter()
            .filter(|e| !e.path.is_empty() && matches!(e.node, ProjectionNode::Directory))
        {
            stage.create_directory(&Path::new("data").join(&entry.path))?;
        }
        let mut placements = Vec::new();
        for entry in request.tree.entries() {
            let relative = Path::new("data").join(&entry.path);
            match &entry.node {
                ProjectionNode::Directory => (),
                ProjectionNode::File {
                    content,
                    executable,
                } => {
                    let file = request.file(content, *executable)?;
                    file.validate_for(content, file.mode(), request.profile.owner)?;
                    let method = file.place(
                        &stage.path().join(&relative),
                        request.profile.files,
                        request.profile.owner,
                    )?;
                    placements.push(FilePlacement {
                        path: entry.path.clone(),
                        method,
                    });
                }
                ProjectionNode::Symlink { target } => {
                    stage.dir.symlink_contents(target, relative)?
                }
            }
        }
        let record = TreeProjectionRecord {
            schema: LOCAL_TREE_SCHEMA.into(),
            input: request.tree.input().clone(),
            manifest: request.tree.output().root().clone(),
            layout: request.tree.layout().clone(),
            profile: request.profile.clone(),
            files: placements,
        };
        let bytes = canonical_json_bytes(&record).map_err(|_| Error::Integrity)?;
        if bytes.len() > request.tree.limits().max_bundle_bytes {
            return Err(Error::TooLarge);
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.path().join("receipt.json"))?;
        file.write_all(&bytes)?;
        filesystem::permissions(&file, 0o444)?;
        file.sync_all()?;
        stage.finish()?;
        if !stage.commit(&target)? {
            return self
                .inspect(request, &target)?
                .ok_or(Error::OutcomeUncertain);
        }
        target.sync()?;
        // Recheck exact namespace/inodes after publication. Failed checks never
        // delete or chmod a now-visible immutable bundle.
        let mut published = self
            .inspect(request, &target)?
            .ok_or(Error::OutcomeUncertain)?;
        published.reused = false;
        Ok(published)
    }
}
