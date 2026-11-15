//! Typed local frontend frames and the shared length-prefixed JSON codec.

use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    validate_stable_identifier, ClientApplicationMessage, ClientEnvelope, ExecutionTarget,
    FrontendActionName, FrontendCapability, FrontendCommand, HostApplicationMessage, HostEnvelope,
    HostPlatform, HostResult, MobileTmuxSnapshot, ProtocolValidationError, SessionFeedEventV1,
    SessionFeedSnapshotV1, TerminalBridgeHelloV1, TmuxAttachPlan, WorkspaceRequestEnvelopeV1,
    WorkspaceResponseEnvelopeV1, MAX_APPLICATION_PAYLOAD_BYTES,
};

pub const DESKTOP_CONTROL_CONTRACT_VERSION: u16 = 1;

/// Local admission failures, before a frame has entered the transport queue.
/// Codes are shared with the native C ABI; zero means accepted there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum DesktopSendError {
    NotReady = 1,
    QueueFull = 2,
    Unsupported = 3,
    InvalidRequest = 4,
    Unavailable = 5,
}
impl fmt::Display for DesktopSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotReady => "Station Host is reconnecting",
            Self::QueueFull => "Station Host request queue is full",
            Self::Unsupported => "Station Host did not negotiate this capability",
            Self::InvalidRequest => "Station Host rejected an invalid request",
            Self::Unavailable => "Station Host client is unavailable",
        })
    }
}
impl Error for DesktopSendError {}

fn validate_frontend_identity(
    frontend_instance_id: &str,
    frontend_generation: u64,
) -> Result<(), ProtocolValidationError> {
    validate_stable_identifier(frontend_instance_id, "frontend instance ID")?;
    if frontend_generation == 0 {
        return Err(ProtocolValidationError::InvalidIdentifier(
            "frontend generation",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStateApiV1 {
    ControlCenterV1,
    InboxV1,
    ResourcesV1,
    SettingsV1,
    CodexV1,
    NotificationV1,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostControlVersionsV1 {
    /// Desktop IPC response shape. Increment when a required field changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<u16>,
}
impl HostControlVersionsV1 {
    pub fn negotiated(self) -> Self {
        Self {
            desktop: self.desktop.filter(|v| *v == DESKTOP_CONTROL_CONTRACT_VERSION),
            codex: self
                .codex
                .filter(|v| *v == crate::CODEX_CONTROL_API_VERSION),
            notification: self
                .notification
                .filter(|v| *v == crate::NOTIFICATION_API_VERSION),
        }
    }
    pub fn permits(self, api: ApplicationStateApiV1) -> bool {
        match api {
            ApplicationStateApiV1::CodexV1 => self.codex == Some(crate::CODEX_CONTROL_API_VERSION),
            ApplicationStateApiV1::NotificationV1 => {
                self.notification == Some(crate::NOTIFICATION_API_VERSION)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalClientFrame {
    Register {
        frontend_id: String,
        platform: HostPlatform,
        supported_actions: Vec<FrontendActionName>,
        #[serde(default)]
        capabilities: Vec<FrontendCapability>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frontend_instance_id: Option<String>,
        #[serde(default, skip_serializing_if = "is_zero")]
        frontend_generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_control_versions: Option<HostControlVersionsV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<crate::LocalExtensionOfferV1>,
    },
    Request {
        request: ClientEnvelope,
    },
    ApplicationRequest {
        request: ClientApplicationMessage,
    },
    ExtensionRequest {
        request: crate::LocalExtensionRequestV1,
    },
    ExtensionCancel {
        request: crate::LocalExtensionCancelV1,
    },
    SubscribeSessionFeed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        known_epoch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_revision: Option<u64>,
    },
    /// Upgrade this private same-user local connection from length-prefixed
    /// JSON to the raw terminal bridge framing contract.
    TerminalBridgeV2 {
        hello: crate::TerminalBridgeHelloV2,
    },
    TerminalBridge {
        hello: TerminalBridgeHelloV1,
    },
    FrontendAck {
        dispatch_id: String,
        result: HostResult,
    },
    WorkspaceRelayCompleted {
        dispatch_id: String,
        response: WorkspaceResponseEnvelopeV1,
    },
    Pong {
        nonce: u64,
    },
}

impl LocalClientFrame {
    pub fn validate(&self) -> Result<(), LocalIpcError> {
        match self {
            Self::Register {
                frontend_id,
                supported_actions,
                capabilities,
                frontend_instance_id,
                frontend_generation,
                extensions,
                ..
            } => {
                validate_identifier(frontend_id)?;
                crate::validate_extension_offers(extensions)
                    .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
                if supported_actions.is_empty() {
                    return Err(LocalIpcError::InvalidRegistration);
                }
                let mut unique = supported_actions.clone();
                unique.sort_unstable();
                unique.dedup();
                if unique.len() != supported_actions.len() {
                    return Err(LocalIpcError::InvalidRegistration);
                }
                let mut unique_capabilities = capabilities.clone();
                unique_capabilities.sort_unstable();
                unique_capabilities.dedup();
                if unique_capabilities.len() != capabilities.len() {
                    return Err(LocalIpcError::InvalidRegistration);
                }
                let workspace_rendering =
                    capabilities.contains(&FrontendCapability::WorkspaceRendering);
                if workspace_rendering {
                    let Some(frontend_instance_id) = frontend_instance_id else {
                        return Err(LocalIpcError::InvalidRegistration);
                    };
                    validate_frontend_identity(frontend_instance_id, *frontend_generation)
                        .map_err(|_| LocalIpcError::InvalidRegistration)?;
                } else if frontend_instance_id.is_some() || *frontend_generation != 0 {
                    return Err(LocalIpcError::InvalidRegistration);
                }
            }
            Self::Request { request } => request
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ApplicationRequest { request } => request
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ExtensionRequest { request } => request.validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ExtensionCancel { request } => request.validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::SubscribeSessionFeed {
                known_epoch,
                after_revision,
            } => match (known_epoch, after_revision) {
                (None, None) => {}
                (Some(epoch), Some(revision)) => {
                    validate_identifier(epoch)?;
                    if *revision == 0 {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "Session feed revision must be greater than zero".to_owned(),
                        ));
                    }
                }
                _ => {
                    return Err(LocalIpcError::InvalidEnvelope(
                        "Session feed epoch and revision must be supplied together".to_owned(),
                    ));
                }
            },
            Self::TerminalBridgeV2 { hello } => hello
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::TerminalBridge { hello } => hello
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::FrontendAck {
                dispatch_id,
                result,
            } => {
                validate_identifier(dispatch_id)?;
                match result {
                    HostResult::Accepted => {}
                    HostResult::Error { error } => {
                        error
                            .validate_sanitized()
                            .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
                    }
                    _ => return Err(LocalIpcError::InvalidFrontendAcknowledgement),
                }
            }
            Self::WorkspaceRelayCompleted {
                dispatch_id,
                response,
            } => {
                validate_identifier(dispatch_id)?;
                response
                    .validate()
                    .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
            }
            Self::Pong { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalHostFrame {
    Registered {
        registration_id: String,
        protocol_version: u32,
        host_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_control_versions: Option<HostControlVersionsV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extensions: Vec<crate::LocalExtensionOfferV1>,
    },
    Response {
        response: HostEnvelope,
    },
    ApplicationResponse {
        response: HostApplicationMessage,
    },
    ExtensionResponse {
        response: crate::LocalExtensionResponseV1,
    },
    ExtensionEvent {
        event: crate::LocalExtensionEventV1,
    },
    ApplicationStateChanged {
        api: ApplicationStateApiV1,
        state_revision: u64,
    },
    SessionFeedSnapshot {
        snapshot: SessionFeedSnapshotV1,
    },
    SessionFeedEvent {
        event: SessionFeedEventV1,
    },
    FrontendExecute {
        dispatch_id: String,
        execution_target: ExecutionTarget,
        revision: u64,
        action: FrontendCommand,
        attach_plan: Option<TmuxAttachPlan>,
        snapshot: Option<MobileTmuxSnapshot>,
    },
    WorkspaceRelay {
        dispatch_id: String,
        request: WorkspaceRequestEnvelopeV1,
    },
    Ping {
        nonce: u64,
    },
}

impl LocalHostFrame {
    pub fn validate(&self) -> Result<(), LocalIpcError> {
        match self {
            Self::Registered {
                registration_id,
                host_id,
                extensions,
                ..
            } => {
                validate_identifier(registration_id)?;
                crate::validate_extension_offers(extensions)
                    .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
                validate_identifier(host_id)?;
            }
            Self::Response { response } => response
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ApplicationResponse { response } => response
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ExtensionEvent { event } => event.validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ExtensionResponse { response } => response.validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::ApplicationStateChanged { state_revision, .. } => {
                if *state_revision == 0 {
                    return Err(LocalIpcError::InvalidEnvelope(
                        "application state revision must be greater than zero".to_owned(),
                    ));
                }
            }
            Self::SessionFeedSnapshot { snapshot } => snapshot
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::SessionFeedEvent { event } => event
                .validate()
                .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?,
            Self::FrontendExecute {
                dispatch_id,
                execution_target,
                action,
                attach_plan,
                snapshot,
                ..
            } => {
                validate_identifier(dispatch_id)?;
                execution_target
                    .validate()
                    .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
                match (action, attach_plan, snapshot) {
                    (FrontendCommand::OpenSession { session }, Some(plan), None) => {
                        plan.validate()
                            .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
                        if plan.attached_session_id() != Some(session.as_str()) {
                            return Err(LocalIpcError::InvalidEnvelope(
                                "open_session action and Host attach plan target different sessions"
                                    .to_string(),
                            ));
                        }
                    }
                    (FrontendCommand::OpenSession { .. }, None, None) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "open_session frontend execution requires a Host attach plan"
                                .to_string(),
                        ));
                    }
                    (FrontendCommand::OpenSession { .. }, _, Some(_)) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "open_session frontend execution must not carry a snapshot".to_string(),
                        ));
                    }
                    (FrontendCommand::Refresh, None, Some(_)) => {}
                    (FrontendCommand::Refresh, None, None) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "refresh frontend execution requires a Host snapshot".to_string(),
                        ));
                    }
                    (FrontendCommand::Refresh, Some(_), _) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "refresh frontend execution must not carry an attach plan".to_string(),
                        ));
                    }
                    (_, Some(_), _) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "only open_session frontend execution may carry an attach plan"
                                .to_string(),
                        ));
                    }
                    (_, None, Some(_)) => {
                        return Err(LocalIpcError::InvalidEnvelope(
                            "only refresh frontend execution may carry a snapshot".to_string(),
                        ));
                    }
                    (_, None, None) => {}
                }
            }
            Self::WorkspaceRelay {
                dispatch_id,
                request,
            } => {
                validate_identifier(dispatch_id)?;
                request
                    .validate()
                    .map_err(|error| LocalIpcError::InvalidEnvelope(error.to_string()))?;
            }
            Self::Ping { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum LocalIpcError {
    Json(serde_json::Error),
    PayloadTooLarge,
    FrameLengthMismatch,
    InvalidIdentifier,
    InvalidRegistration,
    InvalidFrontendAcknowledgement,
    InvalidEnvelope(String),
}

impl Clone for LocalIpcError {
    fn clone(&self) -> Self {
        match self {
            Self::Json(error) => Self::InvalidEnvelope(error.to_string()),
            Self::PayloadTooLarge => Self::PayloadTooLarge,
            Self::FrameLengthMismatch => Self::FrameLengthMismatch,
            Self::InvalidIdentifier => Self::InvalidIdentifier,
            Self::InvalidRegistration => Self::InvalidRegistration,
            Self::InvalidFrontendAcknowledgement => Self::InvalidFrontendAcknowledgement,
            Self::InvalidEnvelope(message) => Self::InvalidEnvelope(message.clone()),
        }
    }
}

impl PartialEq for LocalIpcError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for LocalIpcError {}

impl fmt::Display for LocalIpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "local IPC JSON is invalid: {error}"),
            Self::PayloadTooLarge => formatter.write_str("local IPC JSON exceeds 1 MiB"),
            Self::FrameLengthMismatch => {
                formatter.write_str("local IPC frame length does not match its prefix")
            }
            Self::InvalidIdentifier => formatter.write_str("local IPC identifier is invalid"),
            Self::InvalidRegistration => {
                formatter.write_str("local frontend registration is invalid")
            }
            Self::InvalidFrontendAcknowledgement => {
                formatter.write_str("frontend acknowledgement must be accepted or error")
            }
            Self::InvalidEnvelope(message) => {
                write!(formatter, "local IPC envelope is invalid: {message}")
            }
        }
    }
}

impl Error for LocalIpcError {}

impl From<serde_json::Error> for LocalIpcError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn encode_length_prefixed_json<T: Serialize>(value: &T) -> Result<Vec<u8>, LocalIpcError> {
    let json = serde_json::to_vec(value)?;
    if json.len() > MAX_APPLICATION_PAYLOAD_BYTES {
        return Err(LocalIpcError::PayloadTooLarge);
    }
    let length = u32::try_from(json.len()).map_err(|_| LocalIpcError::PayloadTooLarge)?;
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&json);
    Ok(frame)
}

pub fn decode_length_prefixed_json<T: DeserializeOwned>(frame: &[u8]) -> Result<T, LocalIpcError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(LocalIpcError::FrameLengthMismatch)?
        .try_into()
        .expect("checked four-byte prefix");
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_APPLICATION_PAYLOAD_BYTES {
        return Err(LocalIpcError::PayloadTooLarge);
    }
    if frame.len() != length + 4 {
        return Err(LocalIpcError::FrameLengthMismatch);
    }
    Ok(serde_json::from_slice(&frame[4..])?)
}

fn validate_identifier(value: &str) -> Result<(), LocalIpcError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value != value.trim()
    {
        return Err(LocalIpcError::InvalidIdentifier);
    }
    Ok(())
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}
