//! Stable DTOs and cryptographic wire primitives shared by Polar Tmux Hosts
//! and native clients.
//!
//! This crate intentionally models tmux using tmux's own nouns: server,
//! session, window, pane, and client. It owns no process, socket, discovery,
//! or secure-store implementation.

mod terminal_control;
pub use terminal_control::*;
pub use polar_tmux_types::*;

mod connections;
pub use connections::*;
mod codex_control;
mod control_center;
mod message_connector;
mod notification_relay;
pub use codex_control::*;
pub use message_connector::*;
pub use notification_relay::*;
mod crypto;
mod extensions;
pub use extensions::*;
mod inbox;
mod local_ipc;
mod mobile;
mod resources;
mod session_runtime;
mod session_editor;
pub use session_editor::*;
mod settings;
mod workspace;

use std::fmt;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub use control_center::{
    SessionCreationItemV1, AgentSessionResumeV1, AgentSessionCatalogRowV1, AgentProviderV1, AgentStartModeV1, SessionKindV1,
    AttentionBadgeProjectionV1, AuxiliarySurfaceProjectionV1, AuxiliarySurfaceStateV1,
    AuxiliarySurfacesProjectionV1, ControlActionProjectionV1, ControlActionRoleV1,
    ControlBannerLevelV1, ControlBannerProjectionV1, ControlBottomBarProjectionV1,
    ControlCenterApiV1, ControlCenterCommandV1, ControlCenterErrorV1, ControlCenterIntentV1,
    ControlCenterProjectionV1, ControlCenterRequestEnvelopeV1, ControlCenterResponseEnvelopeV1,
    ControlCenterResultV1, ControlConfirmationV1, ControlConnectionProjectionV1,
    ControlConnectionRouteV1, ControlConnectionStatusV1, ControlHeaderProjectionV1,
    ControlSessionSectionV1, ControlTransportStatusV1, QuickSessionKindV1,
    SessionAssignmentActionsV1, SessionAttentionConfidenceV1, SessionAttentionProjectionV1,
    SessionAttentionSourceV1, SessionAttentionStateV1, SessionIdentityV1,
    SessionLibraryProjectionV1, SessionMonitorHealthV1, SessionReplacementV1,
    SessionTargetGroupV1, SessionTargetStateV1, SessionRowProjectionV1, SessionSectionProjectionV1, SessionSectionStateV1,
    SessionSummaryFreshnessV1, SessionTargetProjectionV1, SshRouteProfileV1,
    WorkspaceSshTargetEditorProjectionV1, WorkspaceStructureCandidateV1, StationPaneContentV1, StationPaneContentKindV1, WorkspaceStructurePaneV1,
    WorkspaceStructureSnapshotV1, WorkspaceStructureTabV1, CONTROL_CENTER_API_VERSION,
    WORKSPACE_STRUCTURE_SCHEMA_VERSION,
};
pub use crypto::{
    derive_production_session_keys, legacy_client_proof, production_pairing_proof,
    production_pairing_proof_base64, verify_production_pairing_proof, DirectionalFrameCiphers,
    EncryptedFrameError, EncryptedFrameReader, EncryptedFrameWriter, FrameDirection,
    PairingCryptoError, PairingProofDirection, PairingTranscript, SessionKeys,
};
pub use inbox::{
    AiInvocationProjectionV1, DeliveryModeV1, DeliveryReceiptStateV1, InboxApiV1,
    InboxAttentionConfidenceV1, InboxAttentionSourceV1, InboxCommandV1, InboxDraftStatusV1,
    InboxMessageIdV1, InboxMessageProjectionV1, InboxPageV1, InboxOverviewV1, InboxOverviewRowV1, InboxRequestEnvelopeV1,
    InboxResponseEnvelopeV1, InboxResultV1, TerminalOriginHandleV1, INBOX_API_VERSION,
};
pub use local_ipc::{
    decode_length_prefixed_json, encode_length_prefixed_json, ApplicationStateApiV1,
    DESKTOP_CONTROL_CONTRACT_VERSION, DesktopSendError, HostControlVersionsV1, LocalClientFrame, LocalHostFrame, LocalIpcError,
};
pub use mobile::{
    MobileCommandPolicy, MobileTmuxPane, MobileTmuxSession, MobileTmuxSnapshot, MobileTmuxWindow,
};

pub use polar_station_settings::{
    AppearanceSettingsV1, AttentionSettingsV1, SessionSummarySettingsV1, SettingsSectionUpdateV1,
    SettingsV1,
};
pub use resources::{
    ResourceLogEventV1, ResourceLogLevelV1, ResourceLogTailV1, ResourcesApiV1, ResourcesCommandV1,
    ResourcesRequestEnvelopeV1, ResourcesResponseEnvelopeV1, ResourcesResultV1,
    RESOURCES_API_VERSION, RESOURCE_LOG_MAX_BYTES, RESOURCE_LOG_MAX_EVENTS,
};
pub use session_runtime::{
    decode_terminal_bridge_frame_v1, encode_terminal_bridge_frame_v1, SessionFeedChangeV1,
    SessionFeedEventV1, SessionFeedSnapshotV1, SessionKeyV1, SessionLifecycleV1,
    SessionProjectionV1, SessionRuntimeApiV1, TerminalBridgeFrameError, TerminalBridgeFrameV1,
    TerminalBridgeHelloV1, TerminalConsumerLifecycleV1, TerminalConsumerStatusV1,
    TerminalRecoveryPhaseV1, TerminalRecoveryViewV1, TerminalSizeV1, SESSION_RUNTIME_API_VERSION,
};
pub use settings::{
    CodexNotifyConfigStateV1, SettingsApiV1, SettingsCommandV1, SettingsRequestEnvelopeV1,
    SettingsResponseEnvelopeV1, SettingsResultV1, SETTINGS_API_VERSION,
};
pub use workspace::{
    CanonicalLayoutProjectionV1, ClientApplicationMessage, CompanionManagedPaneProjectionV1,
    CompanionWorkspaceCatalogProjectionV1, CompanionWorkspaceProjectionV1,
    CompanionWorkspaceTabProjectionV1, HostApplicationMessage, LayoutRoleV1, LayoutSlotV1,
    ManagedPaneProjectionV1, PendingImpactV1, RationalV1, RuntimeHealthV1, TransportStatusV1,
    WorkspaceApiV1, WorkspaceCatalogProjectionV1, WorkspaceCommandV1, WorkspaceProjectionV1,
    WorkspaceRequestEnvelopeV1, WorkspaceResponseEnvelopeV1, WorkspaceResultV1, WorkspaceScopeV1,
    WorkspaceTabProjectionV1, WORKSPACE_API_VERSION,
};
pub type SanitizedTmuxSession = MobileTmuxSession;
pub type SanitizedTmuxWindow = MobileTmuxWindow;
pub type SanitizedTmuxPane = MobileTmuxPane;
pub type SanitizedTmuxSnapshot = MobileTmuxSnapshot;

pub const MAX_APPLICATION_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CompanionPairingPayloadV1 {
    pub schema: String,
    pub host_id: String,
    pub pairing_code: String,
    pub connected_devices: u32,
    pub listener_ready: bool,
}

impl CompanionPairingPayloadV1 {
    pub fn new(
        host_id: impl Into<String>,
        pairing_code: impl Into<String>,
        connected_devices: u32,
        listener_ready: bool,
    ) -> Result<Self, ProtocolValidationError> {
        let payload = Self {
            schema: "polar-tmux-pairing-payload-v1".to_owned(),
            host_id: host_id.into(),
            pairing_code: pairing_code.into(),
            connected_devices,
            listener_ready,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.schema != "polar-tmux-pairing-payload-v1" {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "pairing payload schema",
            ));
        }
        validate_stable_identifier(&self.host_id, "Host ID")?;
        let prefix = format!("polar-tmux://pair?v=1&host_id={}&key=", self.host_id);
        if !self.pairing_code.starts_with(&prefix)
            || self.pairing_code.len() <= prefix.len()
            || self.pairing_code.len() > 1024
            || self.pairing_code.chars().any(char::is_control)
        {
            return Err(ProtocolValidationError::InvalidIdentifier("pairing code"));
        }
        Ok(())
    }
}

impl fmt::Debug for CompanionPairingPayloadV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompanionPairingPayloadV1")
            .field("schema", &self.schema)
            .field("host_id", &self.host_id)
            .field("pairing_code", &"<redacted>")
            .field("connected_devices", &self.connected_devices)
            .field("listener_ready", &self.listener_ready)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPlatform {
    Macos,
    Linux,
    Windows,
    Unknown,
}

impl HostPlatform {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Self::Macos,
            "linux" => Self::Linux,
            "windows" => Self::Windows,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDescriptor {
    pub protocol_version: u32,
    pub host_id: String,
    pub host_name: String,
    pub platform: HostPlatform,
    pub host_version: String,
    pub capabilities: HostCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCapabilities {
    pub tmux_read: bool,
    pub tmux_write: bool,
    pub frontend_control: bool,
    pub remote_tmux: bool,
    pub wsl_tmux: bool,
    #[serde(default)]
    pub workspace_api_versions: Vec<u16>,
    #[serde(default)]
    pub resources_api_versions: Vec<u16>,
    #[serde(default)]
    pub presentation_api_versions: Vec<u16>,
    #[serde(default)]
    pub session_runtime_api_versions: Vec<u16>,
    #[serde(default)]
    pub ssh_multiplex_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub execution_target: ExecutionTarget,
    pub expected_revision: Option<u64>,
    pub command: ClientCommand,
}

impl ClientEnvelope {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        self.execution_target.validate()?;
        if self.command.is_mutation() && self.expected_revision.is_none() {
            return Err(ProtocolValidationError::MissingExpectedRevision);
        }
        if let ClientCommand::AwaitAttachRecovery {
            after_transport_generation,
            ..
        } = &self.command
        {
            after_transport_generation.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Ping,
    GetHost,
    GetCompanionPairing,
    ListExecutionTargets,
    GetSnapshot,
    ListSessions,
    ListWindows {
        session: String,
    },
    ListPanes {
        target: String,
    },
    ListClients,
    CreateSession {
        name: String,
        working_directory: Option<String>,
        command: Option<String>,
    },
    KillSession {
        target: String,
    },
    CreateWindow {
        session: String,
        name: Option<String>,
        working_directory: Option<String>,
        command: Option<String>,
    },
    KillWindow {
        target: String,
    },
    RenameWindow {
        target: String,
        name: String,
    },
    SplitPane {
        target: String,
        horizontal: bool,
        working_directory: Option<String>,
        command: Option<String>,
    },
    KillPane {
        target: String,
    },
    SelectPane {
        target: String,
    },
    ResizePane {
        target: String,
        direction: ResizeDirection,
        amount: u16,
    },
    SendKeys {
        target: String,
        keys: Vec<String>,
    },
    SendText {
        target: String,
        text: String,
        press_enter: bool,
    },
    SwitchClient {
        client: String,
        session: String,
    },
    CapturePane {
        target: String,
        history: bool,
    },
    PrepareAttach {
        session: String,
    },
    AwaitAttachRecovery {
        session: String,
        after_transport_generation: SshTransportGenerationV1,
    },
    Frontend {
        action: FrontendCommand,
    },
}

impl ClientCommand {
    pub fn is_tmux_mutation(&self) -> bool {
        matches!(
            self,
            Self::CreateSession { .. }
                | Self::KillSession { .. }
                | Self::CreateWindow { .. }
                | Self::KillWindow { .. }
                | Self::RenameWindow { .. }
                | Self::SplitPane { .. }
                | Self::KillPane { .. }
                | Self::SelectPane { .. }
                | Self::ResizePane { .. }
                | Self::SendKeys { .. }
                | Self::SendText { .. }
                | Self::SwitchClient { .. }
        )
    }

    pub fn is_mutation(&self) -> bool {
        self.is_tmux_mutation() || matches!(self, Self::Frontend { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontendActionName {
    Activate,
    OpenSession,
    FocusPane,
    SetDisplayMode,
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontendCapability {
    RefreshSnapshotV1,
    WorkspaceRendering,
    ApplicationStateEventsV1,
    SessionFeedV1,
    WorkspaceRelayV1,
    SessionLifecycleV2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendCommand {
    Activate,
    OpenSession { session: String },
    FocusPane { pane: String },
    SetDisplayMode { mode: DisplayMode },
    Refresh,
}

impl FrontendCommand {
    pub fn action_name(&self) -> FrontendActionName {
        match self {
            Self::Activate => FrontendActionName::Activate,
            Self::OpenSession { .. } => FrontendActionName::OpenSession,
            Self::FocusPane { .. } => FrontendActionName::FocusPane,
            Self::SetDisplayMode { .. } => FrontendActionName::SetDisplayMode,
            Self::Refresh => FrontendActionName::Refresh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    Monitor,
    Focus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub execution_target: ExecutionTarget,
    pub revision: u64,
    pub result: HostResult,
}

impl HostEnvelope {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        self.execution_target.validate()?;
        match &self.result {
            HostResult::AttachPlan {
                plan,
                transport_generation,
            } => {
                plan.validate()?;
                if let Some(generation) = transport_generation {
                    generation.validate()?;
                }
            }
            HostResult::CompanionPairing { pairing } => pairing.validate()?,
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontendCompletionStatus {
    Applied,
    FrontendFailed,
    TmuxAppliedFrontendFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendCompletion {
    pub status: FrontendCompletionStatus,
    pub frontend_error: Option<ProtocolError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostResult {
    Pong,
    Host {
        host: HostDescriptor,
    },
    CompanionPairing {
        pairing: CompanionPairingPayloadV1,
    },
    ExecutionTargets {
        targets: Vec<ExecutionTargetDescriptor>,
    },
    Snapshot {
        snapshot: TmuxSnapshot,
    },
    MobileSnapshot {
        snapshot: MobileTmuxSnapshot,
    },
    SanitizedSnapshot {
        snapshot: SanitizedTmuxSnapshot,
    },
    Sessions {
        sessions: Vec<TmuxSession>,
    },
    MobileSessions {
        sessions: Vec<MobileTmuxSession>,
    },
    SanitizedSessions {
        sessions: Vec<SanitizedTmuxSession>,
    },
    Windows {
        windows: Vec<TmuxWindow>,
    },
    MobileWindows {
        windows: Vec<MobileTmuxWindow>,
    },
    SanitizedWindows {
        windows: Vec<SanitizedTmuxWindow>,
    },
    Panes {
        panes: Vec<TmuxPane>,
    },
    MobilePanes {
        panes: Vec<MobileTmuxPane>,
    },
    SanitizedPanes {
        panes: Vec<SanitizedTmuxPane>,
    },
    Clients {
        clients: Vec<TmuxClientInfo>,
    },
    SessionCreated {
        session: TmuxSession,
    },
    MobileSessionCreated {
        session: MobileTmuxSession,
    },
    SanitizedSessionCreated {
        session: SanitizedTmuxSession,
    },
    WindowCreated {
        window: TmuxWindow,
    },
    WindowRenamed {
        window: TmuxWindow,
    },
    PaneCreated {
        pane: TmuxPane,
    },
    AttachPlan {
        plan: TmuxAttachPlan,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport_generation: Option<SshTransportGenerationV1>,
    },
    FrontendCompleted {
        completion: FrontendCompletion,
        snapshot: MobileTmuxSnapshot,
    },
    Text {
        text: String,
    },
    Accepted,
    Error {
        error: ProtocolError,
    },
}

/// Text-JSON messages sent by the Companion during production pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingClientMessage {
    HostPeerHello {
        protocol_version: u32,
        host_id: String,
        source_host_id: String,
        scope: HostPeerScopeV1,
        client_nonce: String,
    },
    ClientHello {
        protocol_version: u32,
        host_id: String,
        device_id: String,
        client_nonce: String,
    },
    ClientProof {
        challenge_id: String,
        proof: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPeerScopeV1 {
    ExecutionController,
    NotificationSender,
}
impl HostPeerScopeV1 {
    pub fn credential_account(
        self,
        source_host_id: &str,
    ) -> Result<String, ProtocolValidationError> {
        validate_stable_identifier(source_host_id, "source Host")?;
        let role = match self {
            Self::ExecutionController => "execution-controller",
            Self::NotificationSender => "notification-sender",
        };
        Ok(format!("{role}:{source_host_id}"))
    }
}

/// Text-JSON messages sent by the Host during production pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingHostMessage {
    HostChallenge {
        challenge_id: String,
        host_id: String,
        server_nonce: String,
        expires_unix_seconds: i64,
        host_proof: String,
    },
    HostAccepted {
        session_id: String,
        host_proof: String,
    },
    Error {
        error: ProtocolError,
    },
}

impl PairingClientMessage {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::HostPeerHello {
                protocol_version,
                host_id,
                source_host_id,
                client_nonce,
                ..
            } => Self::ClientHello {
                protocol_version: *protocol_version,
                host_id: host_id.clone(),
                device_id: source_host_id.clone(),
                client_nonce: client_nonce.clone(),
            }
            .validate(),
            Self::ClientHello {
                protocol_version,
                host_id,
                device_id,
                client_nonce,
            } => {
                if *protocol_version != PROTOCOL_VERSION {
                    return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                        *protocol_version,
                    ));
                }
                validate_stable_identifier(host_id, "Host ID")?;
                validate_stable_identifier(device_id, "device ID")?;
                validate_nonce_base64(client_nonce)
            }
            Self::ClientProof {
                challenge_id,
                proof,
            } => {
                validate_stable_identifier(challenge_id, "challenge ID")?;
                validate_proof_base64(proof)
            }
        }
    }
}

impl PairingHostMessage {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::HostChallenge {
                challenge_id,
                host_id,
                server_nonce,
                host_proof,
                ..
            } => {
                validate_stable_identifier(challenge_id, "challenge ID")?;
                validate_stable_identifier(host_id, "Host ID")?;
                validate_nonce_base64(server_nonce)?;
                validate_proof_base64(host_proof)
            }
            Self::HostAccepted {
                session_id,
                host_proof,
            } => {
                validate_stable_identifier(session_id, "session ID")?;
                validate_proof_base64(host_proof)
            }
            Self::Error { error } => error.validate_sanitized(),
        }
    }
}

fn validate_nonce_base64(value: &str) -> Result<(), ProtocolValidationError> {
    validate_canonical_base64_32(value, "nonce")
}

fn validate_proof_base64(value: &str) -> Result<(), ProtocolValidationError> {
    validate_canonical_base64_32(value, "proof")
}

fn validate_canonical_base64_32(
    value: &str,
    field: &'static str,
) -> Result<(), ProtocolValidationError> {
    let decoded = BASE64_STANDARD
        .decode(value)
        .map_err(|_| ProtocolValidationError::InvalidBase64(field))?;
    if decoded.len() != 32 || BASE64_STANDARD.encode(decoded) != value {
        return Err(ProtocolValidationError::InvalidBase64(field));
    }
    Ok(())
}



pub mod polar_access_transport;
pub use polar_access_transport::*;
