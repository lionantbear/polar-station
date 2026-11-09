use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxErrorKind {
    InvalidInput,
    ExecutableMissing,
    NoServer,
    TargetNotFound,
    UnsupportedTarget,
    TargetUnavailable,
    SshMasterUnavailable,
    CommandFailed,
    SessionAlreadyExists,
    ParseFailed,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxError {
    kind: TmuxErrorKind,
    message: String,
}

impl TmuxError {
    pub fn kind(&self) -> TmuxErrorKind {
        self.kind
    }

    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::InvalidInput, message)
    }

    pub(crate) fn executable_missing(executable: &str) -> Self {
        Self::new(
            TmuxErrorKind::ExecutableMissing,
            format!("tmux executable is unavailable: {executable}"),
        )
    }

    pub(crate) fn no_server(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::NoServer, message)
    }

    pub(crate) fn target_not_found(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::TargetNotFound, message)
    }

    pub(crate) fn unsupported_target(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::UnsupportedTarget, message)
    }

    pub(crate) fn target_unavailable(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::TargetUnavailable, message)
    }

    pub(crate) fn ssh_master_unavailable(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::SshMasterUnavailable, message)
    }

    pub(crate) fn command_failed(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::CommandFailed, message)
    }

    pub(crate) fn session_already_exists() -> Self {
        Self::new(
            TmuxErrorKind::SessionAlreadyExists,
            "a tmux session with this name already exists",
        )
    }

    pub(crate) fn parse_failed(message: impl Into<String>) -> Self {
        Self::new(TmuxErrorKind::ParseFailed, message)
    }

    pub(crate) fn timeout(milliseconds: u128) -> Self {
        Self::new(
            TmuxErrorKind::Timeout,
            format!("tmux command timed out after {milliseconds} ms"),
        )
    }

    fn new(kind: TmuxErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for TmuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TmuxError {}

pub type TmuxResult<T> = Result<T, TmuxError>;
