use super::{LocalTreePublisher, LocalTreeRequest};
use cap_std::{
    ambient_authority,
    fs::{Dir, MetadataExt as _},
};
use polar_cas::{projection::FileOwner, Error, Result};
use std::{
    ffi::OsString,
    fs::File,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

pub(super) fn permissions(file: &File, mode: u32) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
pub(super) fn directory_mode(dir: &Dir, owner: FileOwner, mode: u32) -> Result<()> {
    let metadata = dir.dir_metadata()?;
    if metadata.uid() != owner.uid
        || metadata.gid() != owner.gid
        || metadata.mode() & 0o7777 != mode
    {
        return Err(Error::Integrity);
    }
    Ok(())
}
pub(super) fn open_directory(root: &Dir, path: &Path) -> Result<Dir> {
    let mut dir = root.try_clone()?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(Error::Integrity);
        };
        if !dir.symlink_metadata(name)?.is_dir() {
            return Err(Error::Integrity);
        }
        dir = dir.open_dir(name)?;
    }
    Ok(dir)
}
pub(super) fn sync_directory(dir: &Dir) -> Result<()> {
    dir.open(".")?.sync_all()?;
    Ok(())
}
pub(super) fn require_owner(owner: FileOwner) -> Result<()> {
    if owner.uid != rustix::process::geteuid().as_raw()
        || owner.gid != rustix::process::getegid().as_raw()
    {
        return Err(Error::Invalid(
            "projection owner must match the caller execution identity".into(),
        ));
    }
    Ok(())
}

pub(super) struct Target {
    pub parent: Dir,
    pub parent_path: PathBuf,
    pub name: OsString,
}
impl Target {
    pub fn open(
        publisher: &LocalTreePublisher,
        request: &LocalTreeRequest,
        create: bool,
    ) -> Result<Self> {
        let owner = request.profile.owner;
        // Inspection does not create files or assign ownership. A Supervisor
        // may read an authorized tree owned by another uid; inspect still
        // verifies every actual owner/mode/inode against the requested profile.
        if create {
            require_owner(owner)?;
        }
        let key = Path::new(&request.target.key);
        let parent_key = key.parent().ok_or(Error::Integrity)?;
        let mut parent = publisher.dir.try_clone()?;
        for component in parent_key.components() {
            let Component::Normal(name) = component else {
                return Err(Error::Integrity);
            };
            if create {
                match parent.create_dir(name) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
                sync_directory(&parent)?;
            }
            crate::admission::files::check_directory(&parent.symlink_metadata(name)?)?;
            parent = parent.open_dir(name)?;
        }
        let parent_path = publisher.root.join(parent_key);
        if parent_path.canonicalize()? != parent_path {
            return Err(Error::Integrity);
        }
        let by_path = std::fs::metadata(&parent_path)?;
        let by_fd = parent.dir_metadata()?;
        if by_path.dev() != by_fd.dev() || by_path.ino() != by_fd.ino() {
            return Err(Error::Integrity);
        }
        Ok(Self {
            parent,
            parent_path,
            name: key.file_name().ok_or(Error::Integrity)?.into(),
        })
    }
    pub fn path(&self) -> PathBuf {
        self.parent_path.join(&self.name)
    }
    pub fn sync(&self) -> Result<()> {
        sync_directory(&self.parent)
    }
}

pub(super) struct Stage {
    temporary: Option<tempfile::TempDir>,
    pub dir: Dir,
    directories: Vec<PathBuf>,
    committed: bool,
}
impl Stage {
    pub fn new(parent: &Path) -> Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix(".cas-projection-")
            .tempdir_in(parent)?;
        let dir = Dir::open_ambient_dir(temporary.path(), ambient_authority())?;
        Ok(Self {
            temporary: Some(temporary),
            dir,
            directories: vec![],
            committed: false,
        })
    }
    pub fn path(&self) -> &Path {
        self.temporary.as_ref().expect("owned stage").path()
    }
    pub fn create_directory(&mut self, relative: &Path) -> Result<()> {
        self.dir.create_dir(relative)?;
        self.directories.push(relative.into());
        Ok(())
    }
    pub fn finish(&self) -> Result<()> {
        for path in self.directories.iter().rev() {
            let dir = open_directory(&self.dir, path)?;
            permissions(&dir.open(".")?.into_std(), 0o555)?;
            sync_directory(&dir)?;
        }
        permissions(&self.dir.open(".")?.into_std(), 0o555)?;
        sync_directory(&self.dir)
    }
    pub fn commit(&mut self, target: &Target) -> Result<bool> {
        let name = self.path().file_name().ok_or(Error::Integrity)?;
        match rename_no_replace(&target.parent, name, &target.name) {
            Ok(()) => {
                self.committed = true;
                Ok(true)
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let Some(temporary) = self.temporary.take() else {
            return;
        };
        if self.committed {
            let _ = temporary.keep();
            return;
        }
        // Only undo modes on a proven still-private owned tree, never on a
        // bundle that may have become visible after an uncertain rename.
        let private = std::fs::symlink_metadata(temporary.path())
            .ok()
            .zip(self.dir.dir_metadata().ok())
            .is_some_and(|(path, fd)| {
                path.is_dir() && path.dev() == fd.dev() && path.ino() == fd.ino()
            });
        if !private {
            let _ = temporary.keep();
            return;
        }
        if let Ok(file) = self.dir.open(".") {
            let _ = permissions(&file.into_std(), 0o700);
        }
        for path in &self.directories {
            if let Ok(dir) = open_directory(&self.dir, path) {
                if let Ok(file) = dir.open(".") {
                    let _ = permissions(&file.into_std(), 0o700);
                }
            }
        }
        // TempDir removes only this operation's private namespace; readonly
        // linked CAS files are unlinked here, never chmodded or deleted there.
    }
}
pub(super) fn require_atomic_rename() -> Result<()> {
    if cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple"
    )) {
        Ok(())
    } else {
        Err(Error::UnsupportedCapability)
    }
}
fn rename_no_replace(parent: &Dir, from: &std::ffi::OsStr, to: &std::ffi::OsStr) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    {
        rustix::fs::renameat_with(parent, from, parent, to, rustix::fs::RenameFlags::NOREPLACE)
            .map_err(std::io::Error::from)?;
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        let _ = (parent, from, to);
        Err(Error::UnsupportedCapability)
    }
}
