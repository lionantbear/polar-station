use super::{FileMode, FileOwner};
use cap_std::{ambient_authority, fs::Dir};
use polar_cas::{
    verification::{ContentVerifier, VerifiedContent},
    ContentRef, Error, ObjectId, Result,
};
use std::{
    ffi::OsString,
    fs::{File, Metadata},
    io::{Read, Seek, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    mtime: i64,
    mtime_nsec: i64,
}
impl FileStamp {
    pub(super) fn executable(&self) -> bool {
        self.mode & 0o111 != 0
    }
    pub(crate) fn of(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        }
    }
}
pub(super) fn absolute_parent(path: &Path) -> Result<(&Path, &std::ffi::OsStr)> {
    if !path.is_absolute() {
        return Err(Error::Invalid("absolute local file path required".into()));
    }
    let parent = path.parent().ok_or(Error::Integrity)?;
    let name = path.file_name().ok_or(Error::Integrity)?;
    if parent.canonicalize()? != parent {
        return Err(Error::Invalid(
            "local file parent must not contain symlinks".into(),
        ));
    }
    Ok((parent, name))
}
pub(super) fn open_absolute(path: &Path) -> Result<(Dir, OsString, File)> {
    let (parent, name) = absolute_parent(path)?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    open_relative(&dir, Path::new(name))
}
pub(crate) fn open_relative(root: &Dir, key: &Path) -> Result<(Dir, OsString, File)> {
    let mut dir = root.try_clone()?;
    let mut components = key.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(Error::Invalid("relative local key required".into()));
        };
        if components.peek().is_some() {
            // Check and open the same directory atomically. lstat followed by
            // open_dir would allow an intervening in-root symlink substitution.
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let access = rustix::fs::OFlags::PATH;
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let access = rustix::fs::OFlags::RDONLY;
            let fd = rustix::fs::openat(
                &dir,
                name,
                access
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            dir = Dir::from_std_file(File::from(fd));
        } else {
            let fd = rustix::fs::openat(
                &dir,
                name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(Error::Integrity);
            }
            return Ok((dir, name.to_owned(), file));
        }
    }
    Err(Error::Invalid("empty local key".into()))
}
/// Read-only callers do not need a cloned root fd or a retained parent handle.
/// Resolve constraints apply atomically to the complete path, including its leaf.
pub(crate) fn open_read_file(root: &Dir, key: &Path) -> Result<(File, Metadata)> {
    let file = open_read_handle(root, key, false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::Integrity);
    }
    Ok((file, metadata))
}

/// Open under the same path constraints, without auditing an already admitted
/// object's metadata. Cached-only resolution never falls back to disk lookup.
/// WouldBlock means the caller must retry on its existing blocking worker.
pub(crate) fn open_read_handle(root: &Dir, key: &Path, cached_only: bool) -> Result<File> {
    if key.as_os_str().is_empty() || key.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::Invalid("relative local key required".into()));
    }
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};
        match openat2(
            root,
            key,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | if cached_only {
                    ResolveFlags::CACHED
                } else {
                    ResolveFlags::empty()
                },
        ) {
            Ok(fd) => return Ok(File::from(fd)),
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL)
                if cached_only =>
            {
                return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock).into())
            }
            // Only absence of the syscall permits fallback; a path/security
            // error must never be retried with weaker resolution constraints.
            Err(rustix::io::Errno::NOSYS) => {}
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    }
    if cached_only {
        return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock).into());
    }
    Ok(open_relative(root, key)?.2)
}


pub(crate) fn check_directory(metadata: &cap_std::fs::Metadata) -> Result<()> {
    if !metadata.is_dir() {
        return Err(Error::Integrity);
    }
    Ok(())
}
pub(crate) fn check_file(
    file: &File,
    size: u64,
    mode: FileMode,
    owner: FileOwner,
) -> Result<Metadata> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() != size
        || metadata.mode() & 0o7777 != mode.bits()
        || metadata.uid() != owner.uid
        || metadata.gid() != owner.gid
    {
        return Err(Error::Integrity);
    }
    Ok(metadata)
}
pub(super) fn verify_file(
    file: &mut File,
    expected: &ContentRef,
    located: &ObjectId,
    mut output: Option<&mut File>,
) -> Result<VerifiedContent> {
    let before = FileStamp::of(&file.metadata()?);
    file.rewind()?;
    let mut verifier = ContentVerifier::default();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or(Error::TooLarge)?;
        if total > expected.size_bytes {
            return Err(Error::Integrity);
        }
        verifier.update(&buffer[..count])?;
        if let Some(target) = output.as_mut() {
            target.write_all(&buffer[..count])?;
        }
    }
    let verified = verifier.finish();
    if total != expected.size_bytes
        || !verified.ids().contains(&expected.id)
        || !verified.ids().contains(located)
        || before != FileStamp::of(&file.metadata()?)
    {
        return Err(Error::Integrity);
    }
    Ok(verified)
}
pub(super) fn set_private_metadata(file: &File, mode: FileMode, owner: FileOwner) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(Error::Integrity);
    }
    if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
        rustix::fs::fchown(
            file,
            Some(rustix::process::Uid::from_raw(owner.uid)),
            Some(rustix::process::Gid::from_raw(owner.gid)),
        )
        .map_err(std::io::Error::from)?;
    }
    file.set_permissions(std::fs::Permissions::from_mode(mode.bits()))?;
    Ok(())
}
pub(super) fn try_reflink(source: &File, destination: &File) -> Result<bool> {
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        not(any(target_arch = "sparc", target_arch = "sparc64"))
    ))]
    {
        match rustix::fs::ioctl_ficlone(destination, source) {
            Ok(()) => Ok(true),
            Err(
                rustix::io::Errno::XDEV
                | rustix::io::Errno::OPNOTSUPP
                | rustix::io::Errno::NOTTY
                | rustix::io::Errno::INVAL
                | rustix::io::Errno::NOSYS,
            ) => Ok(false),
            Err(error) => Err(std::io::Error::from(error).into()),
        }
    }
    #[cfg(not(all(
        any(target_os = "linux", target_os = "android"),
        not(any(target_arch = "sparc", target_arch = "sparc64"))
    )))]
    {
        let _ = (source, destination);
        Ok(false)
    }
}


