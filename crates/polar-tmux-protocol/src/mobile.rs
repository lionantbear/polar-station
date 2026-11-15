//! Privacy-preserving tmux projections and the Companion command allowlist.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ClientCommand, FrontendCommand, TmuxPane, TmuxSession, TmuxSnapshot, TmuxTopologyPane,
    TmuxTopologySnapshot, TmuxWindow,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileTmuxSession {
    pub id: String,
    pub name: String,
    pub window_count: u32,
    pub attached: bool,
}

impl From<&TmuxSession> for MobileTmuxSession {
    fn from(session: &TmuxSession) -> Self {
        Self {
            id: sanitize_label(&session.id),
            name: sanitize_label(&session.name),
            window_count: session.window_count,
            attached: session.attached,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileTmuxWindow {
    pub id: String,
    pub session_id: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub pane_count: u32,
    pub layout: String,
}

impl From<&TmuxWindow> for MobileTmuxWindow {
    fn from(window: &TmuxWindow) -> Self {
        Self {
            id: sanitize_label(&window.id),
            session_id: sanitize_label(&window.session_id),
            index: window.index,
            name: sanitize_label(&window.name),
            active: window.active,
            pane_count: window.pane_count,
            layout: sanitize_layout(&window.layout),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileTmuxPane {
    pub id: String,
    pub window_id: String,
    pub index: u32,
    pub title: String,
    pub active: bool,
    pub dead: bool,
}

impl From<&TmuxPane> for MobileTmuxPane {
    fn from(pane: &TmuxPane) -> Self {
        Self {
            id: sanitize_label(&pane.id),
            window_id: sanitize_label(&pane.window_id),
            index: pane.index,
            title: sanitize_label(&pane.title),
            active: pane.active,
            dead: pane.dead,
        }
    }
}

impl From<&TmuxTopologyPane> for MobileTmuxPane {
    fn from(pane: &TmuxTopologyPane) -> Self {
        Self {
            id: sanitize_label(&pane.id),
            window_id: sanitize_label(&pane.window_id),
            index: pane.index,
            title: sanitize_label(&pane.title),
            active: pane.active,
            dead: pane.dead,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileTmuxSnapshot {
    pub sessions: Vec<MobileTmuxSession>,
    pub windows: Vec<MobileTmuxWindow>,
    pub panes: Vec<MobileTmuxPane>,
}

impl From<&TmuxSnapshot> for MobileTmuxSnapshot {
    fn from(snapshot: &TmuxSnapshot) -> Self {
        Self {
            sessions: snapshot
                .sessions
                .iter()
                .map(MobileTmuxSession::from)
                .collect(),
            windows: snapshot
                .windows
                .iter()
                .map(MobileTmuxWindow::from)
                .collect(),
            panes: snapshot.panes.iter().map(MobileTmuxPane::from).collect(),
        }
    }
}

impl From<&TmuxTopologySnapshot> for MobileTmuxSnapshot {
    fn from(snapshot: &TmuxTopologySnapshot) -> Self {
        Self {
            sessions: snapshot
                .sessions
                .iter()
                .map(MobileTmuxSession::from)
                .collect(),
            windows: snapshot
                .windows
                .iter()
                .map(MobileTmuxWindow::from)
                .collect(),
            panes: snapshot.panes.iter().map(MobileTmuxPane::from).collect(),
        }
    }
}

pub struct MobileCommandPolicy;

impl MobileCommandPolicy {
    pub fn validate(command: &ClientCommand) -> Result<(), MobilePolicyError> {
        match command {
            ClientCommand::Ping
            | ClientCommand::GetHost
            | ClientCommand::ListExecutionTargets
            | ClientCommand::GetSnapshot
            | ClientCommand::ListSessions => Ok(()),
            ClientCommand::ListWindows { session } => validate_tmux_reference(session, '$'),
            ClientCommand::ListPanes { target } => {
                if validate_tmux_reference(target, '$').is_ok()
                    || validate_tmux_reference(target, '@').is_ok()
                {
                    Ok(())
                } else {
                    Err(MobilePolicyError::InvalidTmuxReference)
                }
            }
            ClientCommand::CreateSession {
                name,
                working_directory,
                command,
            } if working_directory.is_none() && command.is_none() => validate_session_name(name),
            ClientCommand::Frontend { action } => validate_frontend_action(action),
            _ => Err(MobilePolicyError::CommandNotAllowed),
        }
    }
}

fn validate_frontend_action(action: &FrontendCommand) -> Result<(), MobilePolicyError> {
    match action {
        FrontendCommand::Activate | FrontendCommand::Refresh => Ok(()),
        FrontendCommand::OpenSession { session } => validate_tmux_reference(session, '$'),
        FrontendCommand::FocusPane { pane } => validate_tmux_reference(pane, '%'),
        FrontendCommand::SetDisplayMode { .. } => Err(MobilePolicyError::CommandNotAllowed),
    }
}

fn validate_tmux_reference(value: &str, prefix: char) -> Result<(), MobilePolicyError> {
    let mut characters = value.chars();
    if characters.next() != Some(prefix) {
        return Err(MobilePolicyError::InvalidTmuxReference);
    }
    let suffix: String = characters.collect();
    if suffix.is_empty() || suffix.len() > 20 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(MobilePolicyError::InvalidTmuxReference);
    }
    Ok(())
}

fn validate_session_name(value: &str) -> Result<(), MobilePolicyError> {
    if value.is_empty()
        || value.len() > 64
        || value != value.trim()
        || !value
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | ' '))
    {
        return Err(MobilePolicyError::InvalidSessionName);
    }
    Ok(())
}

fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

fn sanitize_layout(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, ',' | 'x' | '[' | ']' | '{' | '}' | '-')
        })
        .take(1024)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobilePolicyError {
    CommandNotAllowed,
    InvalidTmuxReference,
    InvalidSessionName,
}

impl fmt::Display for MobilePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::CommandNotAllowed => "command is not in the Companion allowlist",
            Self::InvalidTmuxReference => "tmux reference is not a stable object ID",
            Self::InvalidSessionName => "session name violates the Companion name contract",
        };
        formatter.write_str(message)
    }
}

impl Error for MobilePolicyError {}
