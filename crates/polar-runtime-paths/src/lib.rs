//! Runtime data paths, separate from program installation; no ambient discovery.

use anyhow::{ensure, Context, Result};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const LAYOUT_VERSION: u32 = 2;
mod binding;
mod layout;
pub use binding::{RuntimeDataBinding, RuntimeInstanceBinding};
pub use layout::RuntimeDirectoryLayout;

/// Space-scoped objects, instance-owned metadata, and ephemeral CAS directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasPaths {
    pub local_cas_root: PathBuf,
    pub catalog_file: PathBuf,
    pub hot_cache_root: PathBuf,
    pub staging_root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    layout_version: u32,
    installation_id: String,
    #[serde(default, skip_serializing_if = "RuntimeDirectoryLayout::is_default")]
    directories: RuntimeDirectoryLayout,
}

/// An explicit, already bound installation. This type never consults the
/// environment and never treats an unmarked legacy directory as a new root.
#[derive(Debug, Clone)]
pub struct RuntimeRoot {
    path: PathBuf,
    installation_id: String,
    dir: Arc<Dir>,
    directories: RuntimeDirectoryLayout,
}

impl RuntimeRoot {
    pub fn initialize(path: impl AsRef<Path>, installation_id: &str) -> Result<Self> {
        Self::initialize_with_layout(path, installation_id, RuntimeDirectoryLayout::default())
    }

    /// Host bootstrap may share a root with unrelated engineering files.
    /// Inspect only paths owned by the selected layout; never adopt aliases.
    pub fn check_host_initialization(
        path: &Path,
        directories: &RuntimeDirectoryLayout,
    ) -> Result<()> {
        validate_root(path)?;
        directories.validate()?;
        for ancestor in path.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(meta) => ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "Host path is not a real directory: {}",
                    ancestor.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        ensure!(
            std::fs::symlink_metadata(path.join("layout.json"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "Host layout marker already exists: {}",
            path.join("layout.json").display()
        );
        for name in [
            &directories.instances,
            &directories.cas,
            &directories.components,
            &directories.cache,
            &directories.tmp,
            &directories.run,
            &directories.logs,
        ] {
            let directory = path.join(name);
            match std::fs::symlink_metadata(&directory) {
                Ok(meta) => ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "Host directory conflict: {}",
                    directory.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            if name == &directories.cas || name == &directories.components {
                continue;
            }
            let parent = if name == &directories.instances {
                directory
            } else {
                directory.join(&directories.instances)
            };
            for owned in [&parent, &parent.join("supervisor")] {
                match std::fs::symlink_metadata(owned) {
                    Ok(meta) => {
                        ensure!(
                            meta.is_dir() && !meta.file_type().is_symlink(),
                            "Host directory conflict: {}",
                            owned.display()
                        );
                        if owned == &parent.join("supervisor") {
                            ensure!(
                                std::fs::read_dir(owned)?.next().is_none(),
                                "Unregistered Supervisor content: {}",
                                owned.display()
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub fn initialize_host_with_layout(
        path: &Path,
        installation_id: &str,
        directories: RuntimeDirectoryLayout,
    ) -> Result<Self> {
        Self::check_host_initialization(path, &directories)?;
        Self::initialize_layout(path, installation_id, directories, true)
    }

    pub fn initialize_with_layout(
        path: impl AsRef<Path>,
        installation_id: &str,
        directories: RuntimeDirectoryLayout,
    ) -> Result<Self> {
        Self::initialize_layout(path.as_ref(), installation_id, directories, false)
    }

    fn initialize_layout(
        path: &Path,
        installation_id: &str,
        directories: RuntimeDirectoryLayout,
        shared_host: bool,
    ) -> Result<Self> {
        directories.validate()?;

        validate_root(path)?;
        validate_id(installation_id)?;
        if !path.exists() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).context("create installation root")?;
        }
        let path = path.canonicalize().context("resolve installation root")?;
        validate_root(&path)?;
        let dir = Dir::open_ambient_dir(&path, cap_std::ambient_authority())?;
        ensure!(
            shared_host || dir.entries()?.next().is_none(),
            "Refusing to initialize a nonempty root; inspect or migrate it explicitly"
        );
        let layout = Layout {
            layout_version: LAYOUT_VERSION,
            installation_id: installation_id.to_owned(),
            directories: directories.clone(),
        };
        let mut file = dir.open_with("layout.json", private_create_options())?;
        file.write_all(&serde_json::to_vec_pretty(&layout)?)?;
        file.sync_all()?;
        Ok(Self {
            path,
            installation_id: installation_id.to_owned(),
            dir: Arc::new(dir),
            directories,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        validate_root(path.as_ref())?;
        let path = path
            .as_ref()
            .canonicalize()
            .context("resolve installation root")?;
        validate_root(&path)?;
        let dir = Dir::open_ambient_dir(&path, cap_std::ambient_authority())?;
        let metadata = dir.symlink_metadata("layout.json").with_context(|| {
            format!(
                "inspect runtime layout marker {}",
                path.join("layout.json").display()
            )
        })?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Runtime layout marker must be a regular file, not an alias"
        );
        let layout: Layout = serde_json::from_str(&dir.read_to_string("layout.json")?)?;
        ensure!(
            layout.layout_version == LAYOUT_VERSION,
            "Unsupported runtime layout version {}",
            layout.layout_version
        );
        validate_id(&layout.installation_id)?;
        layout.directories.validate()?;
        Ok(Self {
            path,
            installation_id: layout.installation_id,
            dir: Arc::new(dir),
            directories: layout.directories,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }
    pub fn directory_layout(&self) -> &RuntimeDirectoryLayout {
        &self.directories
    }

    /// Explicit bootstrap operation; reads and ordinary instance planning do not call this.
    pub fn initialize_host_directories(&self) -> Result<()> {
        for directory in [
            &self.directories.instances,
            &self.directories.cas,
            &self.directories.components,
            &self.directories.cache,
            &self.directories.tmp,
            &self.directories.run,
            &self.directories.logs,
        ] {
            self.open_owned_directory(PathBuf::from(directory))?;
        }
        Ok(())
    }
    pub fn instance(&self, id: &str) -> Result<InstancePaths> {
        validate_id(id)?;
        Ok(InstancePaths {
            root: self.clone(),
            relative: PathBuf::from(&self.directories.instances).join(id),
        })
    }

    /// Read-only discovery of configured instances, not service health or an
    /// authorization to delete them. Do not follow aliases into another owner.
    pub fn configured_instances(&self) -> Result<Vec<InstancePaths>> {
        let mut found = Vec::new();
        let Some(instances) = checked_directory(&self.dir, Path::new(&self.directories.instances))?
        else {
            return Ok(found);
        };
        for entry in instances.entries()? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            ensure!(
                !file_type.is_symlink(),
                "Instance discovery rejects symlink entries"
            );
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let id = name
                .to_str()
                .context("Instance directory ID must be UTF-8")?;
            validate_id(id)?;
            let instance = instances.open_dir(&name)?;
            let Some(config) = checked_directory(&instance, Path::new("config"))? else {
                continue;
            };
            match config.symlink_metadata("polar.toml") {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_file() && !metadata.file_type().is_symlink(),
                        "Instance config must be a regular file, not an alias"
                    );
                    found.push(self.instance(id)?);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        found.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(found)
    }

    pub fn storage_space(&self, id: &str) -> Result<StorageSpacePaths> {
        StorageSpacePaths::plan(&self.path, id, &self.directories)
    }

    /// Private persistent CAS metadata directory; never initializes an unknown
    /// root or changes the sharing/layout of the space's payload objects.
    pub fn cas_catalog_directory(&self, space: &str, instance: &str) -> Result<RuntimeDirectory> {
        let file = self.storage_space(space)?.catalog_file(instance)?;
        self.open_owned_directory(
            file.parent()
                .expect("catalog has parent")
                .strip_prefix(&self.path)?
                .to_path_buf(),
        )
    }

    pub fn component(&self, component: RuntimeComponent, id: &str) -> Result<ComponentPaths> {
        validate_id(id)?;
        Ok(ComponentPaths {
            root: self.clone(),
            relative: PathBuf::from(&self.directories.components)
                .join(component.name())
                .join(id),
        })
    }

    fn open_owned_directory(&self, relative: PathBuf) -> Result<RuntimeDirectory> {
        validate_relative(&relative)?;
        let mut builder = DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        self.dir.create_dir_with(&relative, &builder)?;
        Ok(RuntimeDirectory {
            runtime_root: self.path.clone(),
            path: self.path.join(&relative),
            dir: self.dir.open_dir(relative)?,
        })
    }
}

/// Physical sharing is explicit: identical IDs under distinct roots do not
/// identify the same storage. Paths alone grant no read or delete permission.
#[derive(Debug, Clone)]
pub struct StorageSpacePaths {
    path: PathBuf,
}
impl StorageSpacePaths {
    fn plan(root: &Path, id: &str, directories: &RuntimeDirectoryLayout) -> Result<Self> {
        validate_id(id)?;
        Ok(Self {
            path: root.join(&directories.cas).join(id),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn objects(&self) -> PathBuf {
        self.path.join("objects")
    }
    pub fn catalog_file(&self, instance: &str) -> Result<PathBuf> {
        validate_id(instance)?;
        Ok(self.path.join("catalogs").join(instance).join("catalog.db"))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeComponent {
    PolarTmux,
    Desktop,
    NodeOps,
}
impl RuntimeComponent {
    pub fn name(self) -> &'static str {
        match self {
            Self::PolarTmux => "polar-tmux",
            Self::Desktop => "desktop",
            Self::NodeOps => "node-ops",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComponentPaths {
    root: RuntimeRoot,
    relative: PathBuf,
}
impl ComponentPaths {
    pub fn path(&self) -> PathBuf {
        self.root.path.join(&self.relative)
    }
    pub fn directory(&self, module: RuntimeModule, area: RuntimeArea) -> Result<RuntimeDirectory> {
        self.root.open_owned_directory(area.relative(
            &self.relative,
            module,
            &self.root.directories,
        ))
    }
}

/// Central registration of runtime file owners, not an arbitrary namespace.
#[derive(Debug, Clone, Copy)]
pub enum RuntimeModule {
    IdeaCave,
    Kernel,
    Delivery,
    Lgp,
    Blob,
    Voice,
    Inhouse,
    Codex,
    Pi,
    Adam,
    Acp,
    WebBridge,
    Heron,
    Julius,
    Plugins,
    Telegram,
    Maintenance,
    NodeOps,
    Station,
    Desktop,
    ComputeInstance,
}
impl RuntimeModule {
    pub fn name(self) -> &'static str {
        match self {
            Self::IdeaCave => "ideacave",
            Self::Kernel => "kernel",
            Self::Delivery => "delivery",
            Self::Lgp => "lgp",
            Self::Blob => "blob",
            Self::Voice => "voice",
            Self::Inhouse => "inhouse",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Adam => "adam",
            Self::Acp => "acp",
            Self::WebBridge => "web-bridge",
            Self::Heron => "heron",
            Self::Julius => "julius",
            Self::Plugins => "plugins",
            Self::Telegram => "telegram",
            Self::Maintenance => "maintenance",
            Self::NodeOps => "node-ops",
            Self::Station => "station",
            Self::Desktop => "desktop",
            Self::ComputeInstance => "compute_instance",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeArea {
    Config,
    Secrets,
    Data,
    Cache,
    Tmp,
    Run,
    Logs,
}

/// Declared classification, not permission to blindly archive native homes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaBackupPolicy {
    Include,
    Rebuildable,
    Transient,
    Diagnostic,
}

impl RuntimeArea {
    pub fn backup_policy(self) -> AreaBackupPolicy {
        match self {
            Self::Config | Self::Secrets | Self::Data => AreaBackupPolicy::Include,
            Self::Cache => AreaBackupPolicy::Rebuildable,
            Self::Tmp | Self::Run => AreaBackupPolicy::Transient,
            Self::Logs => AreaBackupPolicy::Diagnostic,
        }
    }

    fn relative(
        self,
        owner: &Path,
        module: RuntimeModule,
        directories: &RuntimeDirectoryLayout,
    ) -> PathBuf {
        if self.backup_policy() == AreaBackupPolicy::Include {
            owner.join(self.name()).join(module.name())
        } else {
            PathBuf::from(directories.area(self))
                .join(owner)
                .join(module.name())
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Secrets => "secrets",
            Self::Data => "data",
            Self::Cache => "cache",
            Self::Tmp => "tmp",
            Self::Run => "run",
            Self::Logs => "logs",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstancePathPlan {
    root: PathBuf,
    relative: PathBuf,
    directories: RuntimeDirectoryLayout,
}

/// Shared Polar-scoped working root used by runtime configuration and Git.
pub fn scoped_inhouse_polar_root(
    inhouse_polar_root: &std::path::Path,
    inhouse_polar_id: &str,
) -> std::path::PathBuf {
    inhouse_polar_root.join(inhouse_polar_id)
}
impl InstancePathPlan {
    /// Pure topology planning, including for an installation not created yet.
    /// A returned path grants no IO authority and does not validate a live root.
    pub fn new(root: impl AsRef<Path>, instance: &str) -> Result<Self> {
        Self::with_layout(root, instance, RuntimeDirectoryLayout::default())
    }

    pub fn with_layout(
        root: impl AsRef<Path>,
        instance: &str,
        directories: RuntimeDirectoryLayout,
    ) -> Result<Self> {
        validate_root(root.as_ref())?;
        validate_id(instance)?;
        directories.validate()?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            relative: PathBuf::from(&directories.instances).join(instance),
            directories,
        })
    }

    /// Resolve only the exact config location within an explicitly supplied
    /// root. Never derive a root from a database filename or infer ID from stem.
    pub fn from_config_path(root: impl AsRef<Path>, config: impl AsRef<Path>) -> Result<Self> {
        Self::from_config_path_with_layout(root, config, RuntimeDirectoryLayout::default())
    }

    pub fn from_config_path_with_layout(
        root: impl AsRef<Path>,
        config: impl AsRef<Path>,
        directories: RuntimeDirectoryLayout,
    ) -> Result<Self> {
        validate_root(root.as_ref())?;
        let relative = config
            .as_ref()
            .strip_prefix(root.as_ref())
            .context("Config is outside the bound runtime root")?;
        validate_relative(relative)?;
        let mut components = relative.iter();
        ensure!(
            components.next() == Some(std::ffi::OsStr::new(&directories.instances)),
            "Expected an instance config path"
        );
        let id = components
            .next()
            .and_then(|value| value.to_str())
            .context("Missing instance ID in config path")?;
        let plan = Self::with_layout(root, id, directories)?;
        ensure!(
            config.as_ref() == plan.config_file(),
            "Config does not match the instance's canonical config path"
        );
        Ok(plan)
    }

    pub fn instance_id(&self) -> &str {
        // Every constructor validates an ASCII ID and fixes this relative shape.
        self.relative
            .file_name()
            .and_then(|name| name.to_str())
            .expect("validated instance ID")
    }

    /// Ownership projection only, not a deletion capability. A caller must
    /// still authorize, stop the instance and perform confined filesystem IO.
    pub fn owned_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.path()];
        for area in [
            RuntimeArea::Cache,
            RuntimeArea::Tmp,
            RuntimeArea::Run,
            RuntimeArea::Logs,
        ] {
            roots.push(
                self.root
                    .join(self.directories.area(area))
                    .join(&self.relative),
            );
        }
        roots
    }

    pub fn cas_paths(&self, storage_space: &str) -> Result<CasPaths> {
        Ok(CasPaths {
            local_cas_root: StorageSpacePaths::plan(&self.root, storage_space, &self.directories)?
                .objects(),
            catalog_file: StorageSpacePaths::plan(&self.root, storage_space, &self.directories)?
                .catalog_file(self.instance_id())?,
            hot_cache_root: self
                .root
                .join(&self.directories.cache)
                .join(&self.relative)
                .join("cas"),
            staging_root: self
                .root
                .join(&self.directories.tmp)
                .join(&self.relative)
                .join("cas"),
        })
    }
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.relative)
    }
    pub fn config_file(&self) -> PathBuf {
        self.path().join("config/polar.toml")
    }
    pub fn identity_file(&self) -> PathBuf {
        self.path().join("secrets/polar.identity")
    }
    pub fn core_database_file(&self) -> PathBuf {
        self.path().join("data/admin.db")
    }
    pub fn lgp_database_file(&self) -> PathBuf {
        self.path().join("data/lgp.db")
    }
    pub fn vault_database_file(&self) -> PathBuf {
        self.path().join("secrets/vault.db")
    }
    pub fn backend_home(&self, profile: &str) -> Result<PathBuf> {
        validate_id(profile)?;
        Ok(self.path().join("backends").join(profile))
    }
    pub fn voice_home(&self) -> PathBuf {
        self.path().join("voice")
    }
    pub fn backend_config(&self, profile: &str) -> Result<PathBuf> {
        validate_id(profile)?;
        Ok(self.path().join("config/backends").join(profile))
    }
    pub fn directory_path(&self, module: RuntimeModule, area: RuntimeArea) -> PathBuf {
        self.root
            .join(area.relative(&self.relative, module, &self.directories))
    }

    /// Resolve an IO capability from this explicit plan. Unlike planning, this
    /// requires an existing marked runtime root and checks owned descendants.
    pub fn open_directory(
        &self,
        module: RuntimeModule,
        area: RuntimeArea,
    ) -> Result<RuntimeDirectory> {
        self.bound_root()?
            .instance(self.instance_id())?
            .directory(module, area)
    }

    pub fn open_backend_directory(&self, profile: &str) -> Result<RuntimeDirectory> {
        self.bound_root()?
            .instance(self.instance_id())?
            .backend_directory(profile)
    }

    /// Check the existing marked root and layout without creating any directory.
    pub fn validate_binding(&self) -> Result<()> {
        self.bound_root().map(|_| ())
    }

    fn bound_root(&self) -> Result<RuntimeRoot> {
        let root = RuntimeRoot::open(&self.root)?;
        ensure!(
            root.directory_layout() == &self.directories,
            "Plan differs from installed Host directory layout"
        );
        Ok(root)
    }
}

#[derive(Debug, Clone)]
pub struct InstancePaths {
    root: RuntimeRoot,
    relative: PathBuf,
}
impl InstancePaths {
    /// Registered base directories for host-owned config, databases and secrets.
    pub fn persistent_directory(&self, area: RuntimeArea) -> Result<RuntimeDirectory> {
        ensure!(
            matches!(
                area,
                RuntimeArea::Config | RuntimeArea::Data | RuntimeArea::Secrets
            ),
            "not a persistent base area"
        );
        self.root
            .open_owned_directory(self.relative.join(area.name()))
    }
    pub fn plan(&self) -> InstancePathPlan {
        InstancePathPlan {
            root: self.root.path.clone(),
            relative: self.relative.clone(),
            directories: self.root.directories.clone(),
        }
    }
    pub fn cas_paths(&self, storage_space: &str) -> Result<CasPaths> {
        self.plan().cas_paths(storage_space)
    }
    pub fn path(&self) -> PathBuf {
        self.plan().path()
    }
    pub fn config_file(&self) -> PathBuf {
        self.plan().config_file()
    }
    pub fn identity_file(&self) -> PathBuf {
        self.plan().identity_file()
    }
    pub fn backend_home(&self, profile: &str) -> Result<PathBuf> {
        self.plan().backend_home(profile)
    }
    pub fn voice_home(&self) -> PathBuf {
        self.plan().voice_home()
    }
    pub fn backend_config(&self, profile: &str) -> Result<PathBuf> {
        self.plan().backend_config(profile)
    }
    pub fn directory(&self, module: RuntimeModule, area: RuntimeArea) -> Result<RuntimeDirectory> {
        self.root.open_owned_directory(area.relative(
            &self.relative,
            module,
            &self.root.directories,
        ))
    }

    /// One native home per profile, shared by roles using that profile.
    /// Credential file names and their contents remain native-backend concerns.
    pub fn backend_directory(&self, profile: &str) -> Result<RuntimeDirectory> {
        validate_id(profile)?;
        self.root
            .open_owned_directory(self.relative.join("backends").join(profile))
    }

    pub fn backend_config_directory(&self, profile: &str) -> Result<RuntimeDirectory> {
        validate_id(profile)?;
        self.root
            .open_owned_directory(self.relative.join("config/backends").join(profile))
    }
}

/// Confined I/O for owned files. A native library accepting only paths must be
/// audited separately; obtaining a PathBuf is not a sandbox guarantee.
#[derive(Debug)]
pub struct RuntimeDirectory {
    runtime_root: PathBuf,
    path: PathBuf,
    dir: Dir,
}
impl RuntimeDirectory {
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn create_new(&self, relative: impl AsRef<Path>, contents: &[u8]) -> Result<()> {
        validate_relative(relative.as_ref())?;
        let mut file = self.dir.open_with(relative, private_create_options())?;
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn read(&self, relative: impl AsRef<Path>) -> Result<Vec<u8>> {
        validate_relative(relative.as_ref())?;
        Ok(self.dir.read(relative)?)
    }

    /// Bounded log reads without loading an unbounded runtime file into memory.
    pub fn read_range(
        &self,
        relative: impl AsRef<Path>,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        validate_relative(relative.as_ref())?;
        ensure!(limit <= 65537, "runtime read range exceeds limit");
        let metadata = self.dir.symlink_metadata(relative.as_ref())?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "runtime log must be a regular file"
        );
        let mut file = self.dir.open(relative)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

fn private_create_options() -> &'static OpenOptions {
    static OPTIONS: std::sync::OnceLock<OpenOptions> = std::sync::OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
    })
}

fn checked_directory(parent: &Dir, relative: &Path) -> Result<Option<Dir>> {
    match parent.symlink_metadata(relative) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Instance discovery requires real directories, not aliases"
            );
            Ok(Some(parent.open_dir(relative)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn validate_root(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.parent().is_some(),
        "Runtime root must be an explicit absolute directory, not a filesystem root"
    );
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "Runtime root must not contain parent traversal"
    );
    Ok(())
}
fn validate_relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Owned file name must be relative without traversal"
    );
    Ok(())
}
fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 96
            && !id.ends_with('.')
            && id != "."
            && id != ".."
            && id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_.".contains(&c)),
        "Invalid runtime directory identifier"
    );
    let base = id.split('.').next().unwrap_or_default();
    ensure!(
        !matches!(
            base,
            "con"
                | "prn"
                | "aux"
                | "nul"
                | "com1"
                | "com2"
                | "com3"
                | "com4"
                | "com5"
                | "com6"
                | "com7"
                | "com8"
                | "com9"
                | "lpt1"
                | "lpt2"
                | "lpt3"
                | "lpt4"
                | "lpt5"
                | "lpt6"
                | "lpt7"
                | "lpt8"
                | "lpt9"
        ),
        "Reserved runtime directory identifier"
    );
    Ok(())
}


