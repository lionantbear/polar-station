pub(crate) mod socket_readiness;

use std::{
    env,
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::fs::OpenOptionsExt,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use polar_tmux_protocol::{
    decode_terminal_bridge_frame_v1, encode_terminal_bridge_frame_v1, TerminalBridgeFrameV1,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub struct UserFileLock {
    _file: File,
}

pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("POLAR_TMUX_IPC_SOCKET") {
        return Ok(PathBuf::from(path));
    }

    #[cfg(target_os = "linux")]
    {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is required unless POLAR_TMUX_IPC_SOCKET is set")?;
        Ok(PathBuf::from(runtime).join("polar-tmux/host.sock"))
    }

    #[cfg(target_os = "macos")]
    {
        let runtime = env::var_os("TMPDIR")
            .context("TMPDIR is required unless POLAR_TMUX_IPC_SOCKET is set")?;
        Ok(PathBuf::from(runtime).join("polar-tmux/host.sock"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        bail!("this Unix platform has no Polar Tmux local IPC path contract")
    }
}

pub fn bind(path: &Path) -> Result<UnixListener> {
    let parent = path
        .parent()
        .context("local IPC socket path has no parent directory")?;
    prepare_private_directory(parent)?;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
                bail!(
                    "refusing to replace non-owned or non-socket local IPC path {}",
                    path.display()
                );
            }
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    bail!(
                        "refusing to replace active local IPC socket {}",
                        path.display()
                    )
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to probe local IPC socket {}", path.display())
                    });
                }
            }
            std::fs::remove_file(path)
                .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect local IPC path {}", path.display()));
        }
    }

    let listener = UnixListener::bind(path)
        .with_context(|| format!("failed to bind local IPC socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect local IPC socket {}", path.display()))?;
    Ok(listener)
}

pub fn acquire_host_instance_lock(socket_path: &Path) -> Result<UserFileLock> {
    let parent = socket_path
        .parent()
        .context("local IPC socket path has no parent directory")?;
    prepare_private_directory(parent)?;
    let file_name = socket_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("local IPC socket path has no UTF-8 file name")?;
    let path = parent.join(format!("{file_name}.instance.lock"));
    let file = open_private_lock_file(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(UserFileLock { _file: file }),
        Err(TryLockError::WouldBlock) => {
            bail!("another Polar Tmux Host already owns {}", path.display())
        }
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("failed to lock {}", path.display()))
        }
    }
}

pub fn acquire_identity_creation_lock() -> Result<UserFileLock> {
    let path = identity_lock_path_with(|name| env::var_os(name))?;
    acquire_identity_creation_lock_at(&path)
}

#[cfg(feature = "telegram-channel")]
pub(crate) fn telegram_lock_socket(bot_id: i64) -> Result<PathBuf> {
    anyhow::ensure!(bot_id > 0, "invalid Telegram bot identity");
    let identity = identity_lock_path_with(|name| env::var_os(name))?;
    Ok(identity
        .parent()
        .context("missing Host runtime directory")?
        .join(format!("telegram-{bot_id}")))
}

fn identity_lock_path_with(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    let runtime = lookup("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is required for the Host identity lock")?;
    #[cfg(target_os = "macos")]
    let runtime = lookup("TMPDIR").context("TMPDIR is required for the Host identity lock")?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = lookup;
        bail!("this Unix platform has no Polar Tmux identity lock path contract");
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Ok(PathBuf::from(runtime).join("polar-tmux/identity.lock"))
}

fn acquire_identity_creation_lock_at(path: &Path) -> Result<UserFileLock> {
    let parent = path
        .parent()
        .context("Host identity lock path has no parent directory")?;
    prepare_private_directory(parent)?;
    let file = open_private_lock_file(path)?;
    file.lock()
        .with_context(|| format!("failed to lock {}", path.display()))?;
    Ok(UserFileLock { _file: file })
}

pub fn verify_peer(stream: &UnixStream) -> Result<()> {
    let credential = stream
        .peer_cred()
        .context("failed to read local IPC peer credentials")?;
    if credential.uid() != effective_uid() {
        bail!("local IPC peer UID does not match the Host UID");
    }
    Ok(())
}

pub fn verify_server_socket(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect local IPC socket {}", path.display()))?;
    if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
        bail!(
            "local IPC server must be a Unix socket owned by the current user: {}",
            path.display()
        );
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "local IPC server permissions are not private: {}",
            path.display()
        );
    }
    Ok(())
}

pub use polar_local_access_runtime::codec::{read_json_frame, write_json_frame};


pub async fn read_terminal_bridge_frame<R>(reader: &mut R) -> Result<Option<TerminalBridgeFrameV1>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 5];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error).context("failed to read terminal bridge frame header"),
    }
    let length = u32::from_be_bytes(
        header[1..]
            .try_into()
            .expect("terminal bridge header has a four-byte length"),
    ) as usize;
    if length > MAX_FRAME_BYTES {
        bail!("terminal bridge frame length exceeds {MAX_FRAME_BYTES}");
    }
    let mut encoded = Vec::with_capacity(header.len() + length);
    encoded.extend_from_slice(&header);
    encoded.resize(header.len() + length, 0);
    reader
        .read_exact(&mut encoded[header.len()..])
        .await
        .context("failed to read terminal bridge frame payload")?;
    decode_terminal_bridge_frame_v1(&encoded)
        .context("invalid terminal bridge frame")
        .map(Some)
}

pub async fn write_terminal_bridge_frame<W>(
    writer: &mut W,
    frame: &TerminalBridgeFrameV1,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded =
        encode_terminal_bridge_frame_v1(frame).context("invalid terminal bridge frame")?;
    writer
        .write_all(&encoded)
        .await
        .context("failed to write terminal bridge frame")?;
    writer
        .flush()
        .await
        .context("failed to flush terminal bridge frame")
}

fn require_owned_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect local IPC directory {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.uid() != effective_uid() {
        bail!(
            "local IPC directory must be a real directory owned by the current user: {}",
            path.display()
        );
    }
    Ok(())
}

pub(crate) fn prepare_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create local IPC directory {}", path.display()))?;
    require_owned_directory(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect local IPC directory {}", path.display()))
}

fn open_private_lock_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("failed to open lock file {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect lock file {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.uid() != effective_uid() {
        bail!(
            "lock file must be a regular file owned by the current user: {}",
            path.display()
        );
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect lock file {}", path.display()))?;
    Ok(file)
}

/// Current Host OS user. This is a process fact, not an authorization grant.
/// Callers must still verify the accepted socket peer with verify_peer.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments, does not dereference memory, and is
    // thread-safe on supported Unix platforms.
    unsafe { libc::geteuid() }
}


