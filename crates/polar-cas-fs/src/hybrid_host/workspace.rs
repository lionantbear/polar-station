//! Explicit persistent workspace identity and standalone-EROFS activation.
//! No Drop implementation removes upper/work or clears inode protections.
use super::{mount_image, owned_mount, HybridWorkspace};
use crate::host::PreparedImage;
use polar_cas::{
    workspace::{WorkspaceInode, WorkspaceStorageRecord},
    Error, Result,
};
use rustix::fs::{mkdirat, openat, Mode, OFlags};
use std::{
    fs::File,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && name != "."
        && name != ".."
}
fn open_dir(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    Ok(File::from(
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn parent_dir(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::Invalid("absolute workspace parent required".into()));
    }
    let mut fd = File::open("/")?;
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => fd = open_dir(&fd, n)?,
            _ => {
                return Err(Error::Invalid(
                    "workspace parent must be canonical and symlink-free".into(),
                ))
            }
        }
    }
    if fd.metadata()?.mode() & 0o077 != 0 {
        return Err(Error::Invalid("workspace parent must be private".into()));
    }
    Ok(fd)
}
fn stamp(file: &File, directory: bool) -> Result<WorkspaceInode> {
    let m = file.metadata()?;
    if m.is_dir() != directory
        || (!directory && (!m.is_file() || m.nlink() != 1))
        || m.mode() & 0o077 != 0
    {
        return Err(Error::Invalid("unsafe workspace inode".into()));
    }
    Ok(WorkspaceInode {
        device: m.dev(),
        inode: m.ino(),
        owner: m.uid(),
        mode: m.mode(),
    })
}
pub(super) fn record(
    root_fd: &File,
    upper_fd: &File,
    work_fd: &File,
    lease_fd: &File,
) -> Result<WorkspaceStorageRecord> {
    let root = stamp(root_fd, true)?;
    let upper = stamp(upper_fd, true)?;
    let work = stamp(work_fd, true)?;
    let lease = stamp(lease_fd, false)?;
    if root.device != upper.device
        || upper.device != work.device
        || lease.device != root.device
        || upper.inode == work.inode
        || root.inode == upper.inode
        || root.inode == work.inode
        || root.owner != upper.owner
        || root.owner != work.owner
        || root.owner != lease.owner
    {
        return Err(Error::Integrity);
    }
    Ok(WorkspaceStorageRecord {
        schema: "polar.cas.workspace-storage.v1".into(),
        root,
        upper,
        work,
        lease,
    })
}
impl HybridWorkspace {
    /// Deterministic name is selected by the owner. Existing paths are NEVER
    /// adopted as a new workspace, including incomplete creations.
    pub fn create_named(parent: &Path, name: &str) -> Result<Self> {
        if !name_valid(name) {
            return Err(Error::Invalid("workspace name".into()));
        }
        let dir = parent_dir(parent)?;
        mkdirat(&dir, name, Mode::RWXU).map_err(std::io::Error::from)?;
        let root_fd = open_dir(&dir, name.as_ref())?;
        for child in ["upper", "work"] {
            mkdirat(&root_fd, child, Mode::RWXU).map_err(std::io::Error::from)?;
        }
        let lease = File::from(
            openat(
                &root_fd,
                "lease",
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        lease
            .try_lock()
            .map_err(|e| Error::Backend(format!("workspace lease: {e}")))?;
        let upper = open_dir(&root_fd, "upper".as_ref())?;
        let work = open_dir(&root_fd, "work".as_ref())?;
        upper.sync_all()?;
        work.sync_all()?;
        lease.sync_all()?;
        root_fd.sync_all()?;
        dir.sync_all()?;
        let identity = record(&root_fd, &upper, &work, &lease)?;
        let value = Self {
            root: parent.join(name),
            root_fd,
            upper,
            work,
            _lock: lease,
            identity,
        };
        value.storage_record()?;
        Ok(value)
    }
    pub fn storage_record(&self) -> Result<WorkspaceStorageRecord> {
        let current = record(&self.root_fd, &self.upper, &self.work, &self._lock)?;
        if current != self.identity {
            return Err(Error::Integrity);
        }
        Ok(current)
    }
    /// Requires exact root/upper/work/lease identity. No CREATE flags, no
    /// chmod, no replacing missing children. Lock is acquired before comparison.
    pub fn reopen(parent: &Path, name: &str, expected: &WorkspaceStorageRecord) -> Result<Self> {
        if !name_valid(name) || expected.schema != "polar.cas.workspace-storage.v1" {
            return Err(Error::Invalid("workspace recovery record".into()));
        }
        let dir = parent_dir(parent)?;
        let root_fd = open_dir(&dir, name.as_ref())?;
        let lease = File::from(
            openat(
                &root_fd,
                "lease",
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        lease
            .try_lock()
            .map_err(|e| Error::Backend(format!("workspace lease: {e}")))?;
        let upper = open_dir(&root_fd, "upper".as_ref())?;
        let work = open_dir(&root_fd, "work".as_ref())?;
        let result = Self {
            root: parent.join(name),
            root_fd,
            upper,
            work,
            _lock: lease,
            identity: expected.clone(),
        };
        if result.storage_record()? != *expected {
            return Err(Error::Integrity);
        }
        result.revalidate()?;
        Ok(result)
    }
    pub fn revalidate(&self) -> Result<()> {
        self.storage_record()?;
        let parent = self
            .root
            .parent()
            .ok_or_else(|| Error::Invalid("workspace parent".into()))?;
        let name = self
            .root
            .file_name()
            .ok_or_else(|| Error::Invalid("workspace name".into()))?;
        let root = open_dir(&parent_dir(parent)?, name)?;
        if stamp(&root, true)? != stamp(&self.root_fd, true)?
            || stamp(&open_dir(&root, "upper".as_ref())?, true)? != stamp(&self.upper, true)?
            || stamp(&open_dir(&root, "work".as_ref())?, true)? != stamp(&self.work, true)?
        {
            return Err(Error::Integrity);
        }
        let lease = File::from(
            openat(
                &root,
                "lease",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        if stamp(&lease, false)? != stamp(&self._lock, false)? {
            return Err(Error::Integrity);
        }
        Ok(())
    }
    /// The owning runtime may pass a borrowed root to its worker. Retain this
    /// whole owner until all users have released derived FDs. Never publish upper.
    pub fn activate_filesystem(
        self,
        image: PreparedImage,
        allow_exec: bool,
    ) -> Result<WorkspaceMount> {
        self.revalidate()?;
        let lower = mount_image(&image, &[], allow_exec)?;
        let (mut fd, mut errno) = (-1, 0);
        // SAFETY: exact immutable lower and exclusive checked upper/work remain
        // owned below; the C operation returns one detached mount and never a path.
        let rc = unsafe {
            polar_cas_fs_sys::pcfs_overlay_mount_detached(
                lower.as_raw_fd(),
                std::ptr::null(),
                0,
                self.upper.as_raw_fd(),
                self.work.as_raw_fd(),
                u32::from(allow_exec),
                &mut fd,
                &mut errno,
            )
        };
        let mount = owned_mount(rc, fd, errno)?;
        Ok(WorkspaceMount {
            mount,
            _lower: lower,
            _image: image,
            workspace: self,
            readonly: false,
        })
    }
}
/// Close merged mount before lower/image/workspace lease (declaration order).
/// Root must not be cloned into an independently writable mount. Host is the
/// trusted owner of all users; this is not an isolation boundary against root.
pub struct WorkspaceMount {
    mount: File,
    _lower: File,
    _image: PreparedImage,
    workspace: HybridWorkspace,
    readonly: bool,
}
impl WorkspaceMount {
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.mount.as_fd()
    }
    pub fn storage_record(&self) -> Result<WorkspaceStorageRecord> {
        self.workspace.storage_record()
    }
    pub fn storage_root(&self) -> &Path {
        self.workspace.storage_root()
    }
    pub fn revalidate(&self) -> Result<()> {
        self.workspace.revalidate()
    }
    /// Freeze the SAME mount (not a readonly clone of a writable mount).
    /// Open writers cause EBUSY. Do not automatically undo readonly on failure.
    pub fn pause_writes(&mut self) -> Result<()> {
        self.revalidate()?;
        let mut errno = 0;
        // SAFETY: owned mount handle remains live and cannot be changed through
        // this exclusive owner during the syscall. No caller boolean creates pause.
        let rc = unsafe {
            polar_cas_fs_sys::pcfs_overlay_readonly_v1(self.mount.as_raw_fd(), &mut errno)
        };
        if rc != polar_cas_fs_sys::PCFS_OK {
            return Err(Error::Backend(format!(
                "readonly workspace status {rc}, errno {errno}"
            )));
        }
        self.readonly = true;
        Ok(())
    }
    /// Only expose a capture root after the kernel transition and sync. This
    /// returned FD remains subordinate to this owner's immutable-lower lease.
    pub fn capture_root(&self) -> Result<File> {
        if !self.readonly {
            return Err(Error::Invalid("workspace is not paused".into()));
        }
        self.revalidate()?;
        open_dir(&self.mount, ".".as_ref())
    }
}
