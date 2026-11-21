//! Transport-independent Pane content commands and receipts.

use anyhow::Result;
use polar_station_protocol::{
    PaneContentApiV1, PaneContentBindingV1, PaneContentCommandV1, PaneContentRequestV1,
    PaneContentResponseV1, PaneContentResultV1, ProtocolError, TerminalBridgeFrameV1,
    PROTOCOL_VERSION,
};
use polar_tmux_types::{ContentChannel, ContentChannelAction, PendingContentTransition};

pub fn bridge_frame_for_action(
    action: &ContentChannelAction,
    binding_generation: u64,
) -> Option<TerminalBridgeFrameV1> {
    match action {
        ContentChannelAction::Attach { binding }
        | ContentChannelAction::RestartChild { binding }
        | ContentChannelAction::ReplaceChild { to: binding, .. }
        | ContentChannelAction::SwitchClient { to: binding, .. } => {
            Some(TerminalBridgeFrameV1::Bind {
                binding: binding.clone(),
                binding_generation,
            })
        }
        ContentChannelAction::Detach { .. } => {
            Some(TerminalBridgeFrameV1::Detach { binding_generation })
        }
        ContentChannelAction::Retire { .. } => Some(TerminalBridgeFrameV1::Retire),
        ContentChannelAction::KeepBinding { .. } => {
            Some(TerminalBridgeFrameV1::RetainBinding { binding_generation })
        }
        ContentChannelAction::KeepDetached => None,
    }
}

pub fn commit_response(
    channel: &mut ContentChannel,
    transition: PendingContentTransition,
    identity: ResponseIdentity,
) -> PaneContentResponseV1 {
    if channel.commit(transition).is_err() {
        return identity.rejected(
            "content_state_changed",
            "Pane content state changed before the operation completed",
            true,
        );
    }
    match channel.binding() {
        Some(binding) => identity.ready(binding.clone()),
        None => identity.detached(),
    }
}

pub fn recovery_request(request: &PaneContentRequestV1, channel: &ContentChannel) -> Result<bool> {
    if !matches!(
        request.command,
        PaneContentCommandV1::ObserveRecovery
            | PaneContentCommandV1::Focus { .. }
            | PaneContentCommandV1::TerminalControl { .. }
            | PaneContentCommandV1::ProbeTerminalControl
            | PaneContentCommandV1::ProbeRecovery
            | PaneContentCommandV1::DismissRecovery
    ) {
        return Ok(false);
    }
    request.validate()?;
    if matches!(
        request.command,
        PaneContentCommandV1::TerminalControl { .. }
    ) {
        anyhow::ensure!(
            request.content_revision == channel.content_revision().max(1),
            "stale terminal control binding"
        );
    }
    anyhow::ensure!(
        request.channel_id == channel.channel_id()
            && request.channel_generation == channel.channel_generation(),
        "stale recovery channel"
    );
    Ok(true)
}

pub fn recovery_response(
    request: &PaneContentRequestV1,
    view: polar_station_protocol::TerminalRecoveryViewV1,
) -> PaneContentResponseV1 {
    PaneContentResponseV1 {
        protocol_version: PROTOCOL_VERSION,
        api: request.api,
        request_id: request.request_id.clone(),
        channel_id: request.channel_id.clone(),
        channel_generation: request.channel_generation,
        content_revision: if matches!(request.command, PaneContentCommandV1::ObserveRecovery) {
            view.content_generation
        } else {
            request.content_revision
        },
        result: PaneContentResultV1::Recovery {
            view: Box::new(view),
        },
    }
}

pub fn sanitized_operation_message(error: &anyhow::Error) -> String {
    let raw = error.to_string().replace(['\n', '\r', '\t'], " ");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "Pane content operation failed".to_owned()
    } else {
        trimmed.chars().take(512).collect()
    }
}

pub struct ResponseIdentity {
    request_id: String,
    channel_id: String,
    channel_generation: u64,
    content_revision: u64,
}

impl From<&PaneContentRequestV1> for ResponseIdentity {
    fn from(request: &PaneContentRequestV1) -> Self {
        Self {
            request_id: request.request_id.clone(),
            channel_id: request.channel_id.clone(),
            channel_generation: request.channel_generation,
            content_revision: request.content_revision,
        }
    }
}

impl ResponseIdentity {
    fn response(self, result: PaneContentResultV1) -> PaneContentResponseV1 {
        PaneContentResponseV1 {
            protocol_version: PROTOCOL_VERSION,
            api: PaneContentApiV1::PaneContentV1,
            request_id: self.request_id,
            channel_id: self.channel_id,
            channel_generation: self.channel_generation,
            content_revision: self.content_revision,
            result,
        }
    }

    fn ready(self, binding: PaneContentBindingV1) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Accepted { binding })
    }

    fn detached(self) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Detached)
    }

    pub fn rejected(self, code: &str, message: &str, retryable: bool) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Rejected {
            error: ProtocolError::new(code, message, retryable),
        })
    }
}
