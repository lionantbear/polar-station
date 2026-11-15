//! OpenSSH argv and POSIX remote-command encoding. Never parses rendered argv.
use std::io;
#[cfg(feature = "runtime")]
use std::{process::Stdio, time::Duration};

#[cfg(feature = "runtime")]
use crate::SshConnectionLease;
use crate::{is_canonical_ssh_control_socket, is_canonical_ssh_destination};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshEndpoint {
    pub(crate) destination: String,
    pub(crate) control_socket: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePty {
    None,
    Required,
}

#[derive(Debug, Clone)]
pub struct SshCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl SshEndpoint {
    pub fn managed(
        destination: impl Into<String>,
        control_socket: impl Into<String>,
    ) -> io::Result<Self> {
        let destination = destination.into();
        let control_socket = control_socket.into();
        if !is_canonical_ssh_destination(&destination)
            || !is_canonical_ssh_control_socket(&control_socket)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid managed SSH endpoint",
            ));
        }
        Ok(Self {
            destination,
            control_socket,
        })
    }
    pub fn destination(&self) -> &str {
        &self.destination
    }
    pub fn control_socket(&self) -> &str {
        &self.control_socket
    }
    /// Trusted Rust-generated remote command. Not a frontend command API.
    pub fn command(&self, remote_command: String, pty: RemotePty) -> SshCommand {
        self.render(remote_command, pty, false)
    }
    /// Frozen wire shape for the retired frontend-execute protocol only.
    /// Current Host execution always uses command().
    pub fn legacy_attach_metadata(&self, remote_command: String) -> SshCommand {
        self.render(remote_command, RemotePty::Required, true)
    }
    fn render(&self, remote_command: String, pty: RemotePty, legacy: bool) -> SshCommand {
        let mut args = vec![
            "-q".into(),
            "-S".into(),
            self.control_socket.clone(),
            "-o".into(),
            "ProxyCommand=/usr/bin/false".into(),
        ];
        if pty == RemotePty::Required {
            args.push("-tt".into());
        }
        if !legacy {
            args.extend([
                "-o".into(),
                "ControlMaster=no".into(),
                "-o".into(),
                "BatchMode=yes".into(),
            ]);
            if pty == RemotePty::None {
                args.push("-T".into());
            }
        }
        args.extend([self.destination.clone(), remote_command]);
        SshCommand {
            program: "ssh".into(),
            args,
        }
    }
}

/// Explicit unmanaged execution is retained for standalone library consumers.
/// Station obtains a managed endpoint from its shared manager instead.
pub fn direct_command(
    destination: &str,
    remote_command: String,
    pty: RemotePty,
) -> io::Result<SshCommand> {
    if !is_canonical_ssh_destination(destination) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH destination",
        ));
    }
    let mut args = Vec::new();
    if pty == RemotePty::Required {
        args.push("-tt".into());
    }
    args.extend([destination.into(), remote_command]);
    Ok(SshCommand {
        program: "ssh".into(),
        args,
    })
}

/// Prepare one SSH PTY for a subsequently supplied trusted generated command.
pub fn prepared_terminal_script(marker: &str) -> io::Result<String> {
    if marker.is_empty()
        || !marker
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_:".contains(&byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid prepared terminal marker",
        ));
    }
    Ok(format!("stty raw -echo || exit 1; printf '{}READY\\n'; IFS= read -r station_attach || exit 1; exec /bin/sh -c \"$station_attach\"", marker))
}

pub fn quote_posix(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        return value.into();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn shell_arguments(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_posix(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "runtime")]
impl SshConnectionLease {
    pub async fn change_forward(&self, cancel: bool, rule: &str) -> io::Result<()> {
        if self.master_exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SSH master exited",
            ));
        }
        let status = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("/usr/bin/ssh")
                .args(self.forward_control_args(cancel, rule))
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH forwarding timed out"))??;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("SSH forwarding failed"))
        }
    }
}


