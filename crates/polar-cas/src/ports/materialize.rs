use crate::{
    projection::{
        export::ProjectionPreparation, ProjectionNode, ProjectionProfile, ProjectionTree,
        TreeProjectionRecord,
    },
    source::SourceLocator,
    ContentRef, Error, Result,
};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct ProjectionSource {
    pub content: ContentRef,
    pub executable: bool,
    pub locator: SourceLocator,
}
/// In-process bounded input; locator claims still require native verification.
#[derive(Clone)]
pub struct ProjectionIo {
    pub tree: ProjectionTree,
    pub target: SourceLocator,
    pub profile: ProjectionProfile,
    pub sources: Vec<ProjectionSource>,
    _owner: Arc<dyn Send + Sync>,
}
impl ProjectionIo {
    pub fn new(
        tree: ProjectionTree,
        target: SourceLocator,
        profile: ProjectionProfile,
        sources: Vec<ProjectionSource>,
    ) -> Result<Self> {
        let value = Self {
            tree,
            target,
            profile,
            sources,
            _owner: Arc::new(()),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn retain(self, owner: impl Send + Sync + 'static) -> Self {
        Self {
            _owner: Arc::new((self._owner, owner)),
            ..self
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        if self.sources.len() > self.tree.limits().tree.max_entries {
            return Err(Error::TooLarge);
        }
        let mut expected = BTreeMap::new();
        for entry in self.tree.entries() {
            if let ProjectionNode::File {
                content,
                executable,
            } = &entry.node
            {
                expected.insert((content.id.to_string(), *executable), content);
            }
        }
        for source in &self.sources {
            source.locator.validate()?;
            if expected.remove(&(source.content.id.to_string(), source.executable))
                != Some(&source.content)
            {
                return Err(Error::Integrity);
            }
        }
        if !expected.is_empty() {
            return Err(Error::MissingLocal);
        }
        Ok(())
    }
}
/// Adapter evidence, not a deserializable completion flag. Catalog validates
/// it against the frozen intent and writes only its bounded aggregate.
pub struct ProjectionCommit {
    pub target: SourceLocator,
    pub root: PathBuf,
    pub record: TreeProjectionRecord,
    pub preparation: ProjectionPreparation,
    pub reused: bool,
}
/// Destination-only inspection input. It intentionally has no source inode,
/// staging, request-key or write capability. Hardlink observers may additionally
/// retain Host-injected read-only identity witnesses, but never prepare them.
#[derive(Clone)]
pub struct ProjectionTargetIo {
    pub tree: ProjectionTree,
    pub target: SourceLocator,
    pub profile: ProjectionProfile,
}
impl ProjectionTargetIo {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        if self.target.revision.is_some() { return Err(Error::Invalid("projection revision".into())); }
        for name in self.target.key.split('/') { crate::manifest::validate_name(name)?; }
        Ok(())
    }
}

#[async_trait]
pub trait ProjectionObserver: Send + Sync {
    async fn observe_projection(&self, request: ProjectionTargetIo)
        -> Result<Option<ProjectionCommit>>;
}

#[async_trait]
pub trait LocalMaterializer: Send + Sync {
    fn read_observer(&self) -> Option<Arc<dyn ProjectionObserver>> { None }

    /// Explicit write-side recovery of a previously published target. Unlike
    /// observe, this may fsync the verified destination before Catalog commit;
    /// it must not prepare missing source variants or change target contents.
    async fn adopt_existing(&self, _request: ProjectionTargetIo) -> Result<Option<ProjectionCommit>> {
        Err(Error::UnsupportedCapability)
    }
    async fn materialize(&self, request: ProjectionIo) -> Result<ProjectionCommit>;
    async fn inspect_projection(&self, request: ProjectionIo) -> Result<Option<ProjectionCommit>>;
}
