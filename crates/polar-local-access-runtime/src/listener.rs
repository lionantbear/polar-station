//! Owned Unix listener. Its public traversal directory contains no secrets.
use crate::server::PolarAccessSocketServer;
use anyhow::{ensure, Context, Result};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::watch,
    task::JoinSet,
};

pub fn prepare_directory(path: &Path, owner: u32) -> Result<()> {
    ensure!(
        path.is_absolute() && path.as_os_str().len() <= 100,
        "invalid local chat socket path"
    );
    let parent = path.parent().context("chat socket parent missing")?;
    ensure!(
        parent.parent().is_some(),
        "chat socket requires a dedicated directory"
    );
    match std::fs::create_dir(parent) {
        Ok(()) => {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755))?;
            if owner != unsafe { libc::geteuid() } {
                let raw = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes())?;
                ensure!(
                    unsafe { libc::chown(raw.as_ptr(), owner, u32::MAX) } == 0,
                    "cannot assign chat directory to service account"
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(parent)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == owner && metadata.mode() & 0o022 == 0,
        "chat directory must be owned and writable only by service account"
    );
    ensure!(
        parent.canonicalize()? == parent,
        "chat directory cannot contain symlinks"
    );
    Ok(())
}

pub struct LocalChatListener {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    _lock: File,
}
impl LocalChatListener {
    pub async fn bind(path: &Path) -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        prepare_directory(path, uid)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        ensure!(lock.metadata()?.uid() == uid, "chat lock owner mismatch");
        lock.try_lock()
            .context("chat socket already owned by another process")?;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            ensure!(
                meta.file_type().is_socket() && meta.uid() == uid,
                "refuse to replace foreign chat path"
            );
            match UnixStream::connect(path).await {
                Ok(_) => anyhow::bail!("chat socket is live"),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(path)?
                }
                Err(error) => return Err(error.into()),
            }
        }
        let listener = UnixListener::bind(path)?;
        let meta = std::fs::symlink_metadata(path)?;
        let owned = Self {
            listener,
            path: path.into(),
            identity: (meta.dev(), meta.ino()),
            _lock: lock,
        };
        // All users may connect, but every operation requires the Vault grant.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
        Ok(owned)
    }

    pub async fn run(
        self,
        server: Arc<PolarAccessSocketServer>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut connections = JoinSet::new();
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                accepted = self.listener.accept(), if connections.len() < 64 => {
                    let (stream, _) = accepted?;
                    let server = server.clone();
                    connections.spawn(async move { server.serve(tokio::io::BufReader::new(stream)).await });
                },
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}
impl Drop for LocalChatListener {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}


