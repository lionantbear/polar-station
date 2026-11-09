//! Control Center v1 wire DTOs.
//!
//! Product decisions are projected by `polar-tmux-workspace`. These types only
//! carry the resulting native-renderer contract across the local IPC boundary.

use std::{collections::BTreeSet, net::Ipv4Addr};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    validate_display_label, validate_stable_identifier, DisplayMode, ProtocolValidationError,
    PROTOCOL_VERSION,
};

pub const CONTROL_CENTER_API_VERSION: u16 = 1;
pub const WORKSPACE_STRUCTURE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCenterApiV1 {
    ControlCenterV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlCenterRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: ControlCenterApiV1,
    pub request_id: String,
    pub frontend_session_id: String,
    pub expected_control_revision: Option<u64>,
    pub command: ControlCenterCommandV1,
}

impl ControlCenterRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        validate_stable_identifier(&self.frontend_session_id, "frontend session ID")?;
        match (&self.command, self.expected_control_revision) {
            (ControlCenterCommandV1::SubmitIntent { .. }, None) => {
                return Err(ProtocolValidationError::MissingExpectedRevision);
            }
            (ControlCenterCommandV1::GetProjection | ControlCenterCommandV1::SessionEditor { .. }, Some(_)) => {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
            _ => {}
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCenterCommandV1 {
    GetProjection,
    SessionEditor { request: crate::SessionEditorRequestV1 },
    SubmitIntent { intent: ControlCenterIntentV1 },
}

impl ControlCenterCommandV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::GetProjection => Ok(()),
            Self::SessionEditor { request } => request.validate(),
            Self::SubmitIntent { intent } => intent.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStructureCandidateV1 {
    pub base_structure_revision: u64,
    pub base_structure_fingerprint: String,
    pub structure: WorkspaceStructureSnapshotV1,
}

impl WorkspaceStructureCandidateV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.base_structure_revision == u64::MAX
            || self.structure.structure_revision != self.base_structure_revision + 1
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        validate_optional_structure_fingerprint(
            &self.base_structure_fingerprint,
            self.base_structure_revision,
        )?;
        self.structure.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStructureSnapshotV1 {
    pub schema_version: u16,
    pub workspace_id: String,
    pub workspace_name: String,
    pub structure_revision: u64,
    pub structure_fingerprint: String,
    pub selected_tab_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_pane_id: Option<String>,
    pub tabs: Vec<WorkspaceStructureTabV1>,
}

impl WorkspaceStructureSnapshotV1 {
    pub fn compute_structure_fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash_component(&mut hash, "polar-station-workspace-structure-v1");
        hash_component(&mut hash, &self.workspace_id);
        hash_component(&mut hash, &self.workspace_name);
        hash_component(&mut hash, &self.selected_tab_id);
        hash_optional_component(&mut hash, self.selected_pane_id.as_deref());
        hash_u64(&mut hash, self.tabs.len() as u64);
        for tab in &self.tabs {
            hash_component(&mut hash, &tab.id);
            hash_component(&mut hash, &tab.title);
            hash_component(
                &mut hash,
                match tab.display_mode {
                    DisplayMode::Monitor => "monitor",
                    DisplayMode::Focus => "focus",
                },
            );
            hash_optional_component(&mut hash, tab.main_pane_id.as_deref());
            hash_string_list(&mut hash, &tab.canonical_pane_ids);
            hash_string_list(&mut hash, &tab.visual_pane_ids);
            hash_u64(&mut hash, tab.panes.len() as u64);
            for pane in &tab.panes {
                hash_component(&mut hash, &pane.id);
                if let Some(content) = &pane.content {
                    content.hash(&mut hash);
                } else {
                    // Preserve every legacy terminal-only structure fingerprint.
                    hash_component(&mut hash, pane.execution_target_id.as_deref().unwrap_or(""));
                    hash_component(&mut hash, pane.tmux_session_id.as_deref().unwrap_or(""));
                }
            }
        }
        format!("{:x}", hash.finalize())
    }

    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.schema_version != WORKSPACE_STRUCTURE_SCHEMA_VERSION {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        validate_stable_identifier(&self.workspace_id, "Workspace ID")?;
        validate_display_label(&self.workspace_name, "Workspace name")?;
        validate_stable_identifier(&self.selected_tab_id, "selected tab ID")?;
        validate_structure_fingerprint(&self.structure_fingerprint)?;
        if self.structure_fingerprint != self.compute_structure_fingerprint()
            || self.tabs.is_empty()
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }

        let mut tab_ids = BTreeSet::new();
        let mut workspace_pane_ids = BTreeSet::new();
        let mut session_bindings = BTreeSet::new();
        for tab in &self.tabs {
            tab.validate()?;
            if !tab_ids.insert(tab.id.as_str()) {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
            for pane in &tab.panes {
                let duplicate_terminal = match (&pane.execution_target_id, &pane.tmux_session_id) {
                    (Some(target), Some(session)) => !session_bindings.insert((target.as_str(), session.as_str())),
                    _ => false,
                };
                if !workspace_pane_ids.insert(pane.id.as_str()) || duplicate_terminal {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
            }
        }
        if !tab_ids.contains(self.selected_tab_id.as_str()) {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        if let Some(selected_pane_id) = &self.selected_pane_id {
            validate_stable_identifier(selected_pane_id, "selected pane ID")?;
            if !workspace_pane_ids.contains(selected_pane_id.as_str()) {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStructureTabV1 {
    pub id: String,
    pub title: String,
    pub display_mode: DisplayMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_pane_id: Option<String>,
    pub canonical_pane_ids: Vec<String>,
    pub visual_pane_ids: Vec<String>,
    pub panes: Vec<WorkspaceStructurePaneV1>,
}

impl WorkspaceStructureTabV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "tab ID")?;
        validate_display_label(&self.title, "tab title")?;
        if self.panes.len() > 9
            || self.canonical_pane_ids.len() != self.panes.len()
            || self.visual_pane_ids.len() != self.panes.len()
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        for pane in &self.panes {
            pane.validate()?;
        }
        if self
            .panes
            .iter()
            .map(|pane| pane.id.as_str())
            .ne(self.canonical_pane_ids.iter().map(String::as_str))
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        let canonical = validate_unique_identifier_list(&self.canonical_pane_ids, "Pane ID")?;
        let visual = validate_unique_identifier_list(&self.visual_pane_ids, "Pane ID")?;
        if canonical != visual {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        match (self.display_mode, self.main_pane_id.as_deref()) {
            (DisplayMode::Monitor, None) => {}
            (DisplayMode::Focus, Some(main_pane_id)) if canonical.contains(main_pane_id) => {
                validate_stable_identifier(main_pane_id, "MAIN Pane ID")?;
            }
            _ => return Err(ProtocolValidationError::InvalidControlCenterCommand),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStructurePaneV1 {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<StationPaneContentV1>,
}

/// Persisted view identity, never an authorization grant. Terminal panes use
/// their existing fields; a semantic pane MUST NOT also bind a tmux session.
/// This scalar DTO intentionally does not depend on any Polar SDK crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StationPaneContentV1 {
    LgpText { target_id: String, node_id: String, owner: String, thread: String, actor: String },
    IdeaTree { view_id: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationPaneContentKindV1 { LgpText, IdeaTree }
impl StationPaneContentV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::LgpText { target_id, node_id, owner, thread, actor } => {
                for value in [target_id, node_id, owner, thread, actor] { validate_stable_identifier(value, "content identity")?; }
            }
            Self::IdeaTree { view_id } => validate_stable_identifier(view_id, "Idea Tree view identity")?,
        }
        Ok(())
    }
    fn hash(&self, hash: &mut Sha256) {
        hash_component(hash, "polar-station-pane-content-v1");
        match self {
            Self::LgpText { target_id, node_id, owner, thread, actor } => {
                hash_component(hash, "lgp_text");
                for value in [target_id, node_id, owner, thread, actor] { hash_component(hash, value); }
            }
            Self::IdeaTree { view_id } => { hash_component(hash, "idea_tree"); hash_component(hash, view_id); }
        }
    }
}

impl WorkspaceStructurePaneV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "Pane ID")?;
        match (&self.execution_target_id, &self.tmux_session_id, &self.content) {
            (Some(target), Some(session), None) => {
                validate_stable_identifier(target, "execution target ID")?;
                validate_stable_identifier(session, "tmux Session ID")
            }
            (None, None, Some(content)) => content.validate(),
            _ => Err(ProtocolValidationError::InvalidControlCenterCommand),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionKindV1 {
    Shell,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderV1 { Codex, Claude }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStartModeV1 { New, Resume }

/// Only Session and target operations cross the Host control boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCenterIntentV1 {
    RefreshSessions,
    CheckConnection { target_id: String, polar: bool },
    UpdateConnection { expected_revision: String, change: crate::ConnectionChangeV1 },
    SetSshTarget {
        target_id: String,
        target_revision: u64,
        destination: String,
        #[serde(deserialize_with = "deserialize_present_optional_route_profile")]
        route_profile: Option<SshRouteProfileV1>,
    },
    QuickCreateSession {
        execution_target_id: String,
        source_session_id: Option<String>,
        kind: QuickSessionKindV1,
    },
    CreateCodexSession {
        execution_target_id: String,
        endpoint_id: String,
        name: String,
        working_directory: Option<String>,
        #[serde(default)]
        yolo: bool,
    },
    /// Owns the original agent identity; only Claude's current CLI fallback
    /// creates a tmux operation plane as part of this action.
    CreateAgentSession {
        execution_target_id: String,
        provider: AgentProviderV1,
        mode: AgentStartModeV1,
        name: String,
        working_directory: Option<String>,
        endpoint_id: Option<String>,
        session_id: Option<String>,
        #[serde(default)]
        session_ids: Vec<String>,
        #[serde(default)]
        yolo: bool,
        /// Optional Codex CLI on the controller Host, connected through its
        /// local/private forward to the original Thread. No second Agent.
        #[serde(default)]
        open_local_cli: bool,
        #[serde(default)]
        open_terminal: bool,
    },
    CreateSession {
        execution_target_id: String,
        name: String,
        working_directory: Option<String>,
        command: Option<String>,
        #[serde(default)]
        auto_restart: bool,
    },
    UseExistingSession {
        execution_target_id: String,
        tmux_session_id: String,
        replacement: SessionReplacementV1,
        confirmation_id: String,
        retire: bool,
    },
    RestartSession {
        execution_target_id: String,
        tmux_session_id: String,
    },
    EndAgentSession { owner_host_id: String, endpoint_id: String, thread_id: String, confirmation_id: String },
    RenameAgentSession { owner_host_id: String, endpoint_id: String, thread_id: String, name: String },
    RenameSession {
        execution_target_id: String,
        tmux_session_id: String,
        name: String,
    },
    KillSession {
        execution_target_id: String,
        tmux_session_id: String,
        confirmation_id: String,
    },
    AcknowledgeSessionSummary {
        execution_target_id: String,
        tmux_session_id: String,
        summary_revision: u64,
    },
    MarkAttentionNotificationHandled {
        execution_target_id: String,
        tmux_session_id: String,
        summary_revision: u64,
    },
}

impl ControlCenterIntentV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::RefreshSessions => Ok(()),
            Self::CheckConnection { target_id, .. } => validate_stable_identifier(target_id, "connection ID"),
            Self::UpdateConnection { expected_revision, .. } => {
                validate_stable_identifier(expected_revision, "connections revision")
            }
            Self::UseExistingSession {
                execution_target_id,
                tmux_session_id,
                replacement,
                confirmation_id,
                ..
            } => {
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                validate_stable_identifier(tmux_session_id, "tmux Session ID")?;
                validate_stable_identifier(confirmation_id, "confirmation ID")?;
                replacement.validate()?;
                if replacement.execution_target_id != *execution_target_id
                    || replacement.tmux_session_id == *tmux_session_id
                {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
                Ok(())
            }
            Self::SetSshTarget {
                target_id,
                destination,
                route_profile,
                ..
            } => {
                validate_stable_identifier(target_id, "execution target ID")?;
                if !crate::is_canonical_ssh_destination(destination) {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
                if let Some(route_profile) = route_profile {
                    route_profile.validate()?;
                }
                Ok(())
            }
            Self::QuickCreateSession {
                execution_target_id,
                source_session_id,
                ..
            } => {
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                if let Some(source) = source_session_id {
                    validate_stable_identifier(source, "source Session ID")?;
                }
                Ok(())
            }
            Self::CreateCodexSession { execution_target_id, endpoint_id, name, working_directory, .. } => {
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                validate_stable_identifier(endpoint_id, "Codex endpoint")?;
                validate_display_label(name, "tmux session name")?;
                validate_optional_session_input(working_directory, "working directory", 4096)
            }
            Self::CreateAgentSession { execution_target_id, provider, mode, name, working_directory, endpoint_id, session_id, session_ids, yolo, open_local_cli, .. } => {
                if !session_ids.is_empty() {
                    if session_ids.len() > 9 || *provider != AgentProviderV1::Codex || *mode != AgentStartModeV1::Resume { return Err(ProtocolValidationError::InvalidControlCenterCommand); }
                    let mut unique = std::collections::BTreeSet::new();
                    for id in session_ids { validate_stable_identifier(id, "thread ID")?; if !unique.insert(id) { return Err(ProtocolValidationError::InvalidControlCenterCommand); } }
                }
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                if !(*provider == AgentProviderV1::Codex && *mode == AgentStartModeV1::Resume) {
                    validate_display_label(name, "agent session name")?;
                }
                validate_optional_session_input(working_directory, "working directory", 4096)?;
                match (provider, mode) {
                    (AgentProviderV1::Codex, AgentStartModeV1::New) => {
                        if session_id.is_some() { return Err(ProtocolValidationError::InvalidControlCenterCommand); }
                    }
                    (AgentProviderV1::Codex, AgentStartModeV1::Resume) => {
                        validate_stable_identifier(session_id.as_deref().ok_or(ProtocolValidationError::InvalidControlCenterCommand)?, "Codex Thread ID")?;
                    }
                    (AgentProviderV1::Claude, AgentStartModeV1::New) => {
                        if session_id.is_some() || endpoint_id.is_some() || *yolo || *open_local_cli { return Err(ProtocolValidationError::InvalidControlCenterCommand); }
                    }
                    (AgentProviderV1::Claude, AgentStartModeV1::Resume) => {
                        if endpoint_id.is_some() || *yolo || *open_local_cli || session_id.as_deref().and_then(|s| uuid::Uuid::parse_str(s).ok()).is_none() {
                            return Err(ProtocolValidationError::InvalidControlCenterCommand);
                        }
                    }
                }
                if *provider == AgentProviderV1::Codex {
                    validate_stable_identifier(endpoint_id.as_deref().ok_or(ProtocolValidationError::InvalidControlCenterCommand)?, "Codex endpoint")?;
                }
                Ok(())
            }
            Self::CreateSession {
                execution_target_id,
                name,
                working_directory,
                command,
                ..
            } => {
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                validate_display_label(name, "tmux session name")?;
                validate_optional_session_input(working_directory, "working directory", 4096)?;
                validate_optional_session_input(command, "session command", 16 * 1024)
            }
            Self::EndAgentSession { owner_host_id, endpoint_id, thread_id, confirmation_id } => {
                validate_stable_identifier(owner_host_id, "owner Host")?;
                validate_stable_identifier(confirmation_id, "confirmation")?;
                crate::CodexCommandV1::OpenThread { endpoint_id:endpoint_id.clone(), thread_id:thread_id.clone() }.validate()
            }
            Self::RenameAgentSession { owner_host_id, endpoint_id, thread_id, name } => {
                validate_stable_identifier(owner_host_id, "owner Host ID")?;
                crate::CodexCommandV1::RenameThread { endpoint_id: endpoint_id.clone(), thread_id: thread_id.clone(), name: name.clone() }.validate()
            }
            Self::RenameSession {
                execution_target_id,
                tmux_session_id,
                name,
            } => {
                validate_session_identity(execution_target_id, tmux_session_id)?;
                validate_display_label(name, "tmux session name")
            }
            Self::KillSession {
                execution_target_id,
                tmux_session_id,
                confirmation_id,
            } => {
                validate_session_identity(execution_target_id, tmux_session_id)?;
                validate_stable_identifier(confirmation_id, "confirmation ID")
            }
            Self::RestartSession {
                execution_target_id,
                tmux_session_id,
            } => validate_session_identity(execution_target_id, tmux_session_id),
            Self::AcknowledgeSessionSummary {
                execution_target_id,
                tmux_session_id,
                summary_revision,
            }
            | Self::MarkAttentionNotificationHandled {
                execution_target_id,
                tmux_session_id,
                summary_revision,
            } => {
                validate_session_identity(execution_target_id, tmux_session_id)?;
                if *summary_revision == 0 {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
                Ok(())
            }
        }
    }
}

fn validate_session_identity(target: &str, session: &str) -> Result<(), ProtocolValidationError> {
    validate_stable_identifier(target, "execution target ID")?;
    validate_stable_identifier(session, "tmux session ID")
}

/// Complete Host-owned route information for one SSH execution target.
///
/// The native clients edit and submit this value as part of the target record;
/// they must not reconstruct it from a separate local configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshRouteProfileV1 {
    pub lan_address: String,
    pub lan_port: u16,
    pub wan_host: String,
    pub wan_port: u16,
    pub doh_providers: Vec<String>,
}

fn deserialize_present_optional_route_profile<'de, D>(
    deserializer: D,
) -> Result<Option<SshRouteProfileV1>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SshRouteProfileV1>::deserialize(deserializer)
}

impl SshRouteProfileV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.lan_address.parse::<Ipv4Addr>().is_err()
            || self.lan_port == 0
            || !valid_route_host(&self.wan_host)
            || self.wan_port == 0
            || self.doh_providers.is_empty()
            || self.doh_providers.len() > 2
            || self
                .doh_providers
                .iter()
                .any(|provider| !matches!(provider.as_str(), "google" | "cloudflare"))
            || self.doh_providers.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        Ok(())
    }
}

fn valid_route_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn validate_optional_session_input(
    value: &Option<String>,
    _label: &'static str,
    max_len: usize,
) -> Result<(), ProtocolValidationError> {
    if value.as_ref().is_some_and(|value| {
        value.is_empty() || value.len() > max_len || value.chars().any(char::is_control)
    }) {
        return Err(ProtocolValidationError::InvalidControlCenterCommand);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCenterResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: ControlCenterApiV1,
    pub request_id: String,
    pub control_revision: u64,
    pub result: ControlCenterResultV1,
}

impl ControlCenterResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        match &self.result {
            ControlCenterResultV1::SessionEditor { result } => result.validate(),
            ControlCenterResultV1::SessionBatchCreated { projection, items } => {
                for item in items { if let Some(session) = &item.session { session.validate()?; }
                    if let Some(error) = &item.error { error.validate()?; } }
                projection.validate()
            },
            ControlCenterResultV1::Projection { projection }
            | ControlCenterResultV1::IntentAccepted { projection } => projection.validate(),
            ControlCenterResultV1::SessionCreated {
                projection,
                session,
            } => {
                session.validate()?;
                projection.validate()
            }
            ControlCenterResultV1::Error { error, projection } => {
                error.validate()?;
                if let Some(projection) = projection {
                    projection.validate()?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlCenterResultV1 {
    SessionBatchCreated { projection: ControlCenterProjectionV1, items: Vec<SessionCreationItemV1> },
    SessionEditor { result: crate::SessionEditorResultV1 },
    SessionCreated {
        projection: ControlCenterProjectionV1,
        session: SessionIdentityV1,
    },
    Projection {
        projection: ControlCenterProjectionV1,
    },
    IntentAccepted {
        projection: ControlCenterProjectionV1,
    },
    Error {
        error: ControlCenterErrorV1,
        projection: Option<ControlCenterProjectionV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCreationItemV1 {
    pub thread_id: String,
    pub session: Option<SessionIdentityV1>,
    pub error: Option<ControlCenterErrorV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCenterErrorV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<AgentSessionResumeV1>,
    pub code: String,
    pub message: String,
}

impl ControlCenterErrorV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.code, "Control Center error code")?;
        // Errors carry actionable explanations, not short display labels.
        // Applying the 128-byte label limit here disconnects IPC on failures.
        if self.message.trim().is_empty() || self.message.len() > 8192
            || self.message.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
            return Err(ProtocolValidationError::InvalidIdentifier("Control Center error message"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlCenterProjectionV1 {
    #[serde(default)]
    pub agent_sessions: Vec<AgentSessionCatalogRowV1>,
    pub schema_version: u16,
    pub control_revision: u64,
    #[serde(default)]
    pub connections: crate::ConnectionsProjectionV1,
    #[serde(default)]
    pub session_groups: Vec<SessionTargetGroupV1>,
    #[serde(default)]
    pub codex_endpoints: Vec<polar_station_settings::CodexEndpointSettingsV1>,
    #[serde(default)]
    pub codex_endpoint_states: std::collections::BTreeMap<String, String>,
    pub header: ControlHeaderProjectionV1,
    pub sessions: SessionLibraryProjectionV1,
    pub bottom_bar: ControlBottomBarProjectionV1,
    #[serde(default)]
    pub projected_actions: Vec<ControlActionProjectionV1>,
    pub auxiliary_surfaces: AuxiliarySurfacesProjectionV1,
    pub banner: Option<ControlBannerProjectionV1>,
    #[serde(default)]
    pub attention_badge: AttentionBadgeProjectionV1,
}

impl ControlCenterProjectionV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.schema_version != CONTROL_CENTER_API_VERSION {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        self.header.validate()?;
        self.sessions.validate()?;
        self.bottom_bar.validate()?;
        for action in &self.projected_actions {
            action.validate()?;
        }
        self.auxiliary_surfaces.validate()?;
        if let Some(banner) = &self.banner {
            banner.validate()?;
        }
        self.attention_badge.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionResumeV1 {
    pub execution_target_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionCatalogRowV1 {
    #[serde(default)]
    pub terminals: Vec<SessionIdentityV1>,
    #[serde(default)]
    pub metadata: crate::CodexThreadMetadataV1,
    pub execution_target_id: String,
    pub owner_host_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub display_name: String,
    pub status: String,
    pub loaded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTargetGroupV1 {
    pub id: String,
    pub title: String,
    pub state: SessionTargetStateV1,
    pub message: Option<String>,
    pub stale: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTargetStateV1 { Loading, Ready, Empty, Offline, MissingTmux, Failed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlHeaderProjectionV1 {
    pub connection: ControlConnectionProjectionV1,
    pub ssh_target_editor: WorkspaceSshTargetEditorProjectionV1,
    pub rename_action: ControlActionProjectionV1,
    pub add_tab: ControlActionProjectionV1,
    pub refresh: ControlActionProjectionV1,
    #[serde(default)]
    pub workspace_menu_detach_action: Option<ControlActionProjectionV1>,
    pub lifecycle_action: Option<ControlActionProjectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSshTargetEditorProjectionV1 {
    pub title: String,
    pub target_id: String,
    pub target_revision: u64,
    pub destination: String,
    pub route_profile: Option<SshRouteProfileV1>,
    pub destination_label: String,
    pub destination_placeholder: String,
    pub cancel_label: String,
    pub save_label: String,
}

impl WorkspaceSshTargetEditorProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_display_label(&self.title, "SSH target editor title")?;
        validate_stable_identifier(&self.target_id, "execution target ID")?;
        if !self.destination.is_empty() && !crate::is_canonical_ssh_destination(&self.destination) {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        if let Some(route_profile) = &self.route_profile {
            route_profile.validate()?;
        }
        validate_display_label(&self.destination_label, "SSH destination label")?;
        validate_display_label(&self.destination_placeholder, "SSH destination placeholder")?;
        validate_display_label(&self.cancel_label, "SSH target cancel label")?;
        validate_display_label(&self.save_label, "SSH target save label")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlConnectionProjectionV1 {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<ControlConnectionRouteV1>,
    pub status: ControlConnectionStatusV1,
    pub accessibility_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlConnectionRouteV1 {
    Lan,
    Wan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlConnectionStatusV1 {
    #[serde(alias = "local")]
    Inactive,
    Connecting,
    Connected,
    Partial,
    Recovering,
    Offline,
}

impl ControlConnectionProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_display_label(&self.label, "connection label")?;
        validate_display_label(&self.accessibility_label, "connection accessibility label")?;
        Ok(())
    }
}

impl ControlHeaderProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.connection.validate()?;
        self.ssh_target_editor.validate()?;
        self.rename_action.validate()?;
        self.add_tab.validate()?;
        self.refresh.validate()?;
        if let Some(action) = &self.workspace_menu_detach_action {
            action.validate()?;
        }
        if let Some(action) = &self.lifecycle_action {
            action.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLibraryProjectionV1 {
    pub visible: bool,
    pub title: String,
    pub interaction_hint: String,
    pub refresh_action: ControlActionProjectionV1,
    pub close_action: ControlActionProjectionV1,
    pub sections: Vec<SessionSectionProjectionV1>,
    pub remote_targets: Vec<SessionTargetProjectionV1>,
    pub rows: Vec<SessionRowProjectionV1>,
    pub selected: Option<SessionIdentityV1>,
    #[serde(default = "default_local_execution_target")]
    pub local_execution_target_id: String,
    #[serde(default)]
    pub remote_execution_target_id: Option<String>,
    pub local_new_shell: ControlActionProjectionV1,
    pub local_new_command: ControlActionProjectionV1,
    pub remote_new_session: ControlActionProjectionV1,
}

impl SessionLibraryProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_display_label(&self.title, "Sessions title")?;
        validate_display_label(&self.interaction_hint, "Sessions interaction hint")?;
        self.refresh_action.validate()?;
        self.close_action.validate()?;
        for section in &self.sections {
            section.validate()?;
        }
        for target in &self.remote_targets {
            target.validate()?;
        }
        for row in &self.rows {
            row.validate()?;
        }
        if let Some(selected) = &self.selected {
            selected.validate()?;
            if !self.rows.iter().any(|row| row.identity == *selected) {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
        }
        validate_stable_identifier(&self.local_execution_target_id, "local execution target ID")?;
        if let Some(target) = &self.remote_execution_target_id {
            validate_stable_identifier(target, "remote execution target ID")?;
        }
        self.local_new_shell.validate()?;
        self.local_new_command.validate()?;
        self.remote_new_session.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSectionProjectionV1 {
    pub section: ControlSessionSectionV1,
    pub title: String,
    pub state: SessionSectionStateV1,
    pub message: Option<String>,
    pub row_count: usize,
}

impl SessionSectionProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_display_label(&self.title, "Session section title")?;
        if let Some(message) = &self.message {
            validate_display_label(message, "Session section message")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSectionStateV1 {
    Loading,
    Populated,
    Empty,
    TargetMissing,
    Error,
}

fn validate_optional_structure_fingerprint(
    value: &str,
    revision: u64,
) -> Result<(), ProtocolValidationError> {
    if revision == 0 && value.is_empty() {
        return Ok(());
    }
    validate_structure_fingerprint(value)
}

fn validate_structure_fingerprint(value: &str) -> Result<(), ProtocolValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ProtocolValidationError::InvalidControlCenterCommand);
    }
    Ok(())
}

fn validate_unique_identifier_list<'a>(
    values: &'a [String],
    description: &'static str,
) -> Result<BTreeSet<&'a str>, ProtocolValidationError> {
    let mut unique = BTreeSet::new();
    for value in values {
        validate_stable_identifier(value, description)?;
        if !unique.insert(value.as_str()) {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
    }
    Ok(unique)
}

fn hash_component(hash: &mut Sha256, value: &str) {
    hash_u64(hash, value.len() as u64);
    hash.update(value.as_bytes());
}

fn hash_optional_component(hash: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash_component(hash, value);
        }
        None => hash.update([0]),
    }
}

fn hash_string_list(hash: &mut Sha256, values: &[String]) {
    hash_u64(hash, values.len() as u64);
    for value in values {
        hash_component(hash, value);
    }
}

fn hash_u64(hash: &mut Sha256, value: u64) {
    hash.update(value.to_be_bytes());
}

fn default_local_execution_target() -> String {
    crate::LOCAL_EXECUTION_TARGET.to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTargetProjectionV1 {
    pub id: String,
    pub label: String,
}

impl SessionTargetProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "Session target ID")?;
        validate_display_label(&self.label, "Session target label")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentityV1 {
    pub execution_target_id: String,
    pub tmux_session_id: String,
}

impl SessionIdentityV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.execution_target_id, "execution target ID")?;
        validate_stable_identifier(&self.tmux_session_id, "tmux session ID")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReplacementV1 {
    pub execution_target_id: String,
    pub tmux_session_id: String,
    pub server_generation: String,
    pub created_unix_seconds: i64,
}

impl SessionReplacementV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.execution_target_id, "execution target ID")?;
        validate_stable_identifier(&self.tmux_session_id, "tmux Session ID")?;
        if self.server_generation.is_empty()
            || self.server_generation.len() > 1024
            || self.created_unix_seconds < 0
        {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRowProjectionV1 {
    pub identity: SessionIdentityV1,
    #[serde(default)]
    pub session_kind: SessionKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_agent_id: Option<String>,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_label: Option<String>,
    pub target_label: String,
    pub section: ControlSessionSectionV1,
    pub attached_clients: u32,
    pub transport_status: ControlTransportStatusV1,
    pub selected: bool,
    pub assigned_to_selected_tab: bool,
    pub assignment_action: ControlActionProjectionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_actions: Option<SessionAssignmentActionsV1>,
    pub rename_action: ControlActionProjectionV1,
    pub kill_action: ControlActionProjectionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_action: Option<ControlActionProjectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<SessionReplacementV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_action: Option<ControlActionProjectionV1>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKindV1 {
    #[default]
    Terminal,
    CodexPlane,
    ClaudePlane,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAssignmentActionsV1 {
    pub add: ControlActionProjectionV1,
    pub remove: ControlActionProjectionV1,
    pub tab_full: ControlActionProjectionV1,
    pub assigned_elsewhere: ControlActionProjectionV1,
    pub no_managed_tab: ControlActionProjectionV1,
}

impl SessionAssignmentActionsV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.add.validate()?;
        self.remove.validate()?;
        self.tab_full.validate()?;
        self.assigned_elsewhere.validate()?;
        self.no_managed_tab.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSessionSectionV1 {
    Local,
    Remote,
}

impl SessionRowProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.identity.validate()?;
        validate_display_label(&self.display_name, "tmux session display name")?;
        if let Some(detail_label) = &self.detail_label {
            validate_display_label(detail_label, "tmux session detail label")?;
        }
        self.assignment_action.validate()?;
        if let Some(actions) = &self.assignment_actions {
            actions.validate()?;
        }
        self.rename_action.validate()?;
        if let Some(action) = &self.restart_action {
            action.validate()?;
        }
        if let Some(replacement) = &self.replacement {
            replacement.validate()?;
        }
        if let Some(action) = &self.replacement_action {
            action.validate()?;
        }
        if self.replacement.is_some() != self.replacement_action.is_some() {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        self.kill_action.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlTransportStatusV1 {
    Online,
    Offline,
    Recovering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionAttentionStateV1 {
    #[default]
    Idle,
    Unknown,
    Working,
    ReviewReady,
    NeedsAttention,
    ApprovalRequired,
    DecisionRequired,
    PermissionRequired,
    Failed,
}

impl SessionAttentionStateV1 {
    pub const fn is_red(self) -> bool {
        matches!(
            self,
            Self::ApprovalRequired
                | Self::DecisionRequired
                | Self::PermissionRequired
                | Self::Failed
        )
    }

    pub const fn is_orange(self) -> bool {
        matches!(self, Self::ReviewReady | Self::NeedsAttention)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionAttentionSourceV1 {
    #[default]
    Unspecified,
    TmuxActivity,
    TmuxBell,
    PaneTitle,
    AgentHook,
    AiClassification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionAttentionConfidenceV1 {
    Direct,
    #[default]
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMonitorHealthV1 {
    Connected,
    #[default]
    Degraded,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionSummaryFreshnessV1 {
    Analyzing,
    Fresh,
    Stale,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAttentionProjectionV1 {
    pub execution_target_id: String,
    pub tmux_session_id: String,
    pub summary_revision: u64,
    #[serde(default)]
    pub summary_sections: Vec<String>,
    pub state: SessionAttentionStateV1,
    pub freshness: SessionSummaryFreshnessV1,
    #[serde(default)]
    pub primary_source: SessionAttentionSourceV1,
    #[serde(default)]
    pub confidence: SessionAttentionConfidenceV1,
    #[serde(default)]
    pub monitor_health: SessionMonitorHealthV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_since_ms: Option<u64>,
    pub unread: bool,
    pub notification_pending: bool,
}

impl Default for SessionAttentionProjectionV1 {
    fn default() -> Self {
        Self {
            execution_target_id: "local".to_owned(),
            tmux_session_id: "unknown".to_owned(),
            summary_revision: 0,
            summary_sections: Vec::new(),
            state: SessionAttentionStateV1::Idle,
            freshness: SessionSummaryFreshnessV1::Unavailable,
            primary_source: SessionAttentionSourceV1::Unspecified,
            confidence: SessionAttentionConfidenceV1::Inferred,
            monitor_health: SessionMonitorHealthV1::Degraded,
            monitor_detail: None,
            source_event_id: None,
            attention_since_ms: None,
            unread: false,
            notification_pending: false,
        }
    }
}

impl SessionAttentionProjectionV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.execution_target_id, "execution target ID")?;
        validate_stable_identifier(&self.tmux_session_id, "tmux session ID")?;
        if self.summary_sections.len() > 3 {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        for section in &self.summary_sections {
            validate_display_label(section, "summary section")?;
        }
        if let Some(detail) = &self.monitor_detail {
            validate_display_label(detail, "monitor detail")?;
        }
        if let Some(source_event_id) = &self.source_event_id {
            validate_stable_identifier(source_event_id, "attention source event ID")?;
        }
        if self.summary_revision == 0 && !self.summary_sections.is_empty() {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionBadgeProjectionV1 {
    pub unresolved_red: u32,
    pub unread_orange: u32,
    pub highest_state: SessionAttentionStateV1,
}

impl Default for AttentionBadgeProjectionV1 {
    fn default() -> Self {
        Self {
            unresolved_red: 0,
            unread_orange: 0,
            highest_state: SessionAttentionStateV1::Idle,
        }
    }
}

impl AttentionBadgeProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.unresolved_red == 0 && self.unread_orange == 0 && self.highest_state.is_red() {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlBottomBarProjectionV1 {
    pub actions: Vec<ControlActionProjectionV1>,
    pub more_actions: Vec<ControlActionProjectionV1>,
}

impl ControlBottomBarProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        // This is an action catalog, not a native toolbar/menu specification.
        let mut ids = BTreeSet::new();
        for action in self.actions.iter().chain(&self.more_actions) {
            if !ids.insert(action.id.as_str()) {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
        }
        for action in &self.actions {
            action.validate()?;
        }
        for action in &self.more_actions {
            action.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionRoleV1 {
    Primary,
    Secondary,
    Toggle,
    Recovery,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlConfirmationV1 {
    pub id: String,
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub destructive: bool,
}

impl ControlConfirmationV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "confirmation ID")?;
        validate_display_label(&self.title, "confirmation title")?;
        // A confirmation contains explanatory prose and interpolated Session
        // and machine names, not a 128-byte button/row label. Keep the body
        // bounded and display-safe without rejecting valid generated names.
        if self.message.is_empty()
            || self.message.len() > 4096
            || self.message != self.message.trim()
            || self.message.chars().any(char::is_control)
        {
            return Err(ProtocolValidationError::InvalidIdentifier("confirmation message"));
        }
        validate_display_label(&self.confirm_label, "confirmation button label")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlActionProjectionV1 {
    pub id: String,
    pub label: String,
    pub role: ControlActionRoleV1,
    pub enabled: bool,
    pub selected: bool,
    pub disabled_reason: Option<String>,
    pub confirmation: Option<ControlConfirmationV1>,
    pub accessibility_label: String,
}

impl ControlActionProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "Control Center action ID")?;
        validate_display_label(&self.label, "Control Center action label")?;
        validate_display_label(
            &self.accessibility_label,
            "Control Center accessibility label",
        )?;
        match (&self.disabled_reason, self.enabled) {
            (Some(reason), false) => {
                validate_display_label(reason, "Control Center disabled reason")?
            }
            (None, false) | (Some(_), true) => {
                return Err(ProtocolValidationError::InvalidControlCenterCommand);
            }
            (None, true) => {}
        }
        if let Some(confirmation) = &self.confirmation {
            confirmation.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliarySurfacesProjectionV1 {
    pub resources: AuxiliarySurfaceProjectionV1,
    pub iphone: AuxiliarySurfaceProjectionV1,
}

impl AuxiliarySurfacesProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        self.resources.validate()?;
        self.iphone.validate()?;
        if self.resources.id != "resources" || self.iphone.id != "iphone" {
            return Err(ProtocolValidationError::InvalidControlCenterCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliarySurfaceProjectionV1 {
    pub id: String,
    pub title: String,
    pub state: AuxiliarySurfaceStateV1,
    pub message: String,
}

impl AuxiliarySurfaceProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_stable_identifier(&self.id, "auxiliary surface ID")?;
        validate_display_label(&self.title, "auxiliary surface title")?;
        validate_display_label(&self.message, "auxiliary surface message")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliarySurfaceStateV1 {
    Available,
    Unavailable,
    Unpaired,
    Paired,
}

impl AuxiliarySurfaceStateV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Unpaired => "unpaired",
            Self::Paired => "paired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlBannerProjectionV1 {
    pub level: ControlBannerLevelV1,
    pub message: String,
}

impl ControlBannerProjectionV1 {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_display_label(&self.message, "Control Center banner")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlBannerLevelV1 {
    Information,
    Warning,
    Error,
}






