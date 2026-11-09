//! Leaf tmux execution/content DTOs. No Polar identity, SDK, store or UI dependency.
//! Types moved from polar-tmux-protocol; that crate re-exports the SAME types.
use std::fmt;
use serde::{Deserialize, Serialize};
mod pane_content;
pub use pane_content::*;
mod recovery;
pub use recovery::*;
mod terminal_control;
pub use terminal_control::*;
mod terminal_size;
pub use terminal_size::*;
pub const PROTOCOL_VERSION: u32 = 1;
pub const LOCAL_EXECUTION_TARGET: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionTarget(String);

impl ExecutionTarget {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolValidationError> {
        let target = Self(value.into());
        target.validate()?;
        Ok(target)
    }

    pub fn local() -> Self {
        Self(LOCAL_EXECUTION_TARGET.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_local(&self) -> bool {
        self.0 == LOCAL_EXECUTION_TARGET
    }

    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.0, "execution target")
    }
}

impl Default for ExecutionTarget {
    fn default() -> Self {
        Self::local()
    }
}

impl fmt::Display for ExecutionTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<&str> for ExecutionTarget {
    type Error = ProtocolValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionTargetKind {
    Local,
    Ssh,
    Wsl,
}

/// Public target projection. Connection destinations and credentials are
/// intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTargetDescriptor {
    pub id: ExecutionTarget,
    pub display_name: String,
    pub kind: ExecutionTargetKind,
}

impl ExecutionTargetDescriptor {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.id.validate()?;
        validate_display_label(&self.display_name, "execution target display name")
    }
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxSession {
    pub id: String,
    pub name: String,
    pub window_count: u32,
    pub attached: bool,
    pub created_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxWindow {
    pub id: String,
    pub session_id: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub pane_count: u32,
    pub layout: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxPane {
    pub id: String,
    pub window_id: String,
    pub index: u32,
    pub title: String,
    pub current_command: String,
    pub current_path: String,
    pub active: bool,
    pub dead: bool,
    pub pid: Option<u32>,
    pub tty: String,
}

/// Pane topology that is safe to collect for privacy-preserving clients.
///
/// Unlike [`TmuxPane`], this type cannot carry the current process, working
/// directory, PID, or TTY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxTopologyPane {
    pub id: String,
    pub window_id: String,
    pub index: u32,
    pub title: String,
    pub active: bool,
    pub dead: bool,
}

impl From<&TmuxPane> for TmuxTopologyPane {
    fn from(pane: &TmuxPane) -> Self {
        Self {
            id: pane.id.clone(),
            window_id: pane.window_id.clone(),
            index: pane.index,
            title: pane.title.clone(),
            active: pane.active,
            dead: pane.dead,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxClientInfo {
    pub name: String,
    pub tty: String,
    pub session_id: String,
    pub pid: Option<u32>,
    pub created_unix_seconds: Option<i64>,
    #[serde(default)]
    pub last_session_unix_seconds: Option<i64>,
    #[serde(default)]
    pub last_session: String,
    pub readonly: bool,
    pub control_mode: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxSnapshot {
    pub sessions: Vec<TmuxSession>,
    pub windows: Vec<TmuxWindow>,
    pub panes: Vec<TmuxPane>,
    #[serde(default)]
    pub clients: Vec<TmuxClientInfo>,
}

/// Session/window/pane topology without tmux client, process, TTY, or other
/// client-adjacent facts. Privacy-preserving Host paths obtain this directly
/// instead of collecting a full snapshot and dropping fields afterward.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxTopologySnapshot {
    pub sessions: Vec<TmuxSession>,
    pub windows: Vec<TmuxWindow>,
    pub panes: Vec<TmuxTopologyPane>,
}

impl From<&TmuxSnapshot> for TmuxTopologySnapshot {
    fn from(snapshot: &TmuxSnapshot) -> Self {
        Self {
            sessions: snapshot.sessions.clone(),
            windows: snapshot.windows.clone(),
            panes: snapshot.panes.iter().map(TmuxTopologyPane::from).collect(),
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeDirection {
    Left,
    Right,
    Up,
    Down,
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxAttachPlan {
    pub program: String,
    pub args: Vec<String>,
}

/// Identity of the Host-owned SSH transport generation used to prepare a
/// remote Pane attach. Native clients treat this as an opaque recovery token;
/// only the Rust Host compares it when waiting for a replacement transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTransportGenerationV1 {
    pub supervisor_instance_id: u64,
    pub generation: u64,
}

impl SshTransportGenerationV1 {
    pub fn supervisor_instance_id(&self) -> u64 {
        self.supervisor_instance_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.supervisor_instance_id == 0 || self.generation == 0 {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "transport generation",
            ));
        }
        Ok(())
    }
}

/// Exact environment prefix used for tmux commands executed through SSH.
///
/// Non-interactive macOS SSH shells do not necessarily load Homebrew into
/// `PATH`. Keeping this assignment fixed and protocol-visible makes discovery
/// and attach plans deterministic without evaluating a remote shell profile.
macro_rules! canonical_tmux_execution_path {
    () => {
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    };
}

/// Fixed trusted executable search path for canonical tmux child processes.
///
/// Native application environments are intentionally not part of the tmux
/// execution contract. In particular, a macOS GUI Surface does not inherit a
/// Homebrew-aware shell profile.
pub const TMUX_EXECUTION_PATH: &str = canonical_tmux_execution_path!();

pub const REMOTE_TMUX_PATH_ASSIGNMENT: &str = concat!("PATH=", canonical_tmux_execution_path!());

impl TmuxAttachPlan {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self.program.as_str() {
            "tmux" if exact_tmux_attach_session(&self.args).is_some() => Ok(()),
            "ssh" => {
                let [quiet, socket_flag, control_socket, option_flag, fail_closed, pty, destination, remote_command] =
                    self.args.as_slice()
                else {
                    return Err(ProtocolValidationError::InvalidAttachPlan);
                };
                if quiet != "-q"
                    || socket_flag != "-S"
                    || !is_canonical_ssh_control_socket(control_socket)
                    || option_flag != "-o"
                    || fail_closed != "ProxyCommand=/usr/bin/false"
                    || pty != "-tt"
                    || !is_canonical_ssh_destination(destination)
                    || exact_remote_tmux_attach_session(remote_command).is_none()
                {
                    return Err(ProtocolValidationError::InvalidAttachPlan);
                }
                Ok(())
            }
            _ => Err(ProtocolValidationError::InvalidAttachPlan),
        }
    }

    pub fn attached_session_id(&self) -> Option<&str> {
        match self.program.as_str() {
            "tmux" => exact_tmux_attach_session(&self.args),
            "ssh" => self
                .args
                .get(7)
                .and_then(|command| exact_remote_tmux_attach_session(command)),
            _ => None,
        }
    }
}

pub fn is_canonical_tmux_socket_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=96).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn is_canonical_ssh_destination(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=255).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes.iter().any(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'_' | b'-' | b'@' | b':' | b'%' | b'+' | b'[' | b']'
                )
        })
}

/// Validate the Unix-domain socket identity used by a Host-owned OpenSSH
/// ControlMaster. The bounded absolute grammar keeps the path representable in
/// `sockaddr_un` and prevents a frontend from substituting a relative path.
pub fn is_canonical_ssh_control_socket(value: &str) -> bool {
    let bytes = value.as_bytes();
    (2..=103).contains(&bytes.len())
        && bytes[0] == b'/'
        && !value.ends_with('/')
        && !value.contains("//")
        && !value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
}

fn exact_tmux_attach_session(args: &[String]) -> Option<&str> {
    match args {
        [attach, target_flag, session] => {
            (attach == "attach-session" && target_flag == "-t" && is_stable_session_id(session))
                .then_some(session)
        }
        [socket_flag, socket, attach, target_flag, session] => (socket_flag == "-L"
            && is_canonical_tmux_socket_name(socket)
            && attach == "attach-session"
            && target_flag == "-t"
            && is_stable_session_id(session))
        .then_some(session),
        _ => None,
    }
}

fn exact_remote_tmux_attach_session(command: &str) -> Option<&str> {
    let words = command.split(' ').collect::<Vec<_>>();
    let command_words = match words.as_slice() {
        [path_assignment, remaining @ ..] if *path_assignment == REMOTE_TMUX_PATH_ASSIGNMENT => {
            remaining
        }
        legacy => legacy,
    };
    let session = match command_words {
        ["tmux", "attach-session", "-t", session] => *session,
        ["tmux", "-L", socket, "attach-session", "-t", session]
            if is_canonical_tmux_socket_name(socket) =>
        {
            *session
        }
        _ => return None,
    };
    let session = session
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))?;
    is_stable_session_id(session).then_some(session)
}

fn is_stable_session_id(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('$') else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}


pub use polar_local_access_contract::{ProtocolError, ProtocolValidationError, validate_stable_identifier, validate_display_label};
