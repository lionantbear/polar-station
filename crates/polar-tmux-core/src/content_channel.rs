//! Transactional state for one resident terminal content channel.
//!
//! The channel deliberately has no Workspace, Tab, Pane-order, focus, or
//! layout knowledge. It decides only which content operation is required to
//! make one physical terminal channel show one tmux Session.

use polar_tmux_types::{
    PaneContentBindingV1, PaneContentCommandV1, PaneContentRequestV1, ProtocolValidationError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentChannelPhase {
    Detached,
    Bound,
    Changing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentChannelAction {
    Attach {
        binding: PaneContentBindingV1,
    },
    SwitchClient {
        from: PaneContentBindingV1,
        to: PaneContentBindingV1,
    },
    ReplaceChild {
        from: PaneContentBindingV1,
        to: PaneContentBindingV1,
    },
    RestartChild {
        binding: PaneContentBindingV1,
    },
    Detach {
        from: PaneContentBindingV1,
    },
    Retire {
        from: Option<PaneContentBindingV1>,
    },
    KeepBinding {
        binding: PaneContentBindingV1,
    },
    KeepDetached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentChannelError {
    InvalidRequest(ProtocolValidationError),
    InvalidChannelIdentity,
    WrongChannel,
    StaleGeneration,
    StaleRevision,
    Busy,
    InvalidTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingContentTransition {
    channel_id: String,
    channel_generation: u64,
    previous_revision: u64,
    content_revision: u64,
    previous_binding: Option<PaneContentBindingV1>,
    next_binding: Option<PaneContentBindingV1>,
    action: ContentChannelAction,
}

impl PendingContentTransition {
    pub fn action(&self) -> &ContentChannelAction {
        &self.action
    }

    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentChannel {
    channel_id: String,
    channel_generation: u64,
    content_revision: u64,
    binding: Option<PaneContentBindingV1>,
    phase: ContentChannelPhase,
}

impl ContentChannel {
    pub fn new(
        channel_id: impl Into<String>,
        channel_generation: u64,
    ) -> Result<Self, ContentChannelError> {
        Self::from_parts(channel_id.into(), channel_generation, 0, None)
    }

    pub fn bound(
        channel_id: impl Into<String>,
        channel_generation: u64,
        content_revision: u64,
        binding: PaneContentBindingV1,
    ) -> Result<Self, ContentChannelError> {
        binding
            .validate()
            .map_err(ContentChannelError::InvalidRequest)?;
        if content_revision == 0 {
            return Err(ContentChannelError::InvalidChannelIdentity);
        }
        Self::from_parts(
            channel_id.into(),
            channel_generation,
            content_revision,
            Some(binding),
        )
    }

    fn from_parts(
        channel_id: String,
        channel_generation: u64,
        content_revision: u64,
        binding: Option<PaneContentBindingV1>,
    ) -> Result<Self, ContentChannelError> {
        if channel_id.is_empty()
            || channel_id.len() > 256
            || channel_id != channel_id.trim()
            || channel_id.chars().any(char::is_control)
            || channel_generation == 0
        {
            return Err(ContentChannelError::InvalidChannelIdentity);
        }
        let phase = if binding.is_some() {
            ContentChannelPhase::Bound
        } else {
            ContentChannelPhase::Detached
        };
        Ok(Self {
            channel_id,
            channel_generation,
            content_revision,
            binding,
            phase,
        })
    }

    pub fn begin(
        &mut self,
        request: &PaneContentRequestV1,
    ) -> Result<PendingContentTransition, ContentChannelError> {
        request
            .validate()
            .map_err(ContentChannelError::InvalidRequest)?;
        if request.channel_id != self.channel_id {
            return Err(ContentChannelError::WrongChannel);
        }
        if request.channel_generation != self.channel_generation {
            return Err(ContentChannelError::StaleGeneration);
        }
        if request.content_revision <= self.content_revision {
            return Err(ContentChannelError::StaleRevision);
        }
        if self.phase == ContentChannelPhase::Changing {
            return Err(ContentChannelError::Busy);
        }

        let previous_binding = self.binding.clone();
        let (next_binding, action) = transition_for(&request.command, previous_binding.as_ref());
        self.phase = ContentChannelPhase::Changing;
        Ok(PendingContentTransition {
            channel_id: self.channel_id.clone(),
            channel_generation: self.channel_generation,
            previous_revision: self.content_revision,
            content_revision: request.content_revision,
            previous_binding,
            next_binding,
            action,
        })
    }

    pub fn commit(
        &mut self,
        transition: PendingContentTransition,
    ) -> Result<(), ContentChannelError> {
        self.validate_pending(&transition)?;
        self.binding = transition.next_binding;
        self.content_revision = transition.content_revision;
        self.phase = if self.binding.is_some() {
            ContentChannelPhase::Bound
        } else {
            ContentChannelPhase::Detached
        };
        Ok(())
    }

    pub fn reject(
        &mut self,
        transition: PendingContentTransition,
    ) -> Result<(), ContentChannelError> {
        self.validate_pending(&transition)?;
        self.binding = transition.previous_binding;
        self.phase = if self.binding.is_some() {
            ContentChannelPhase::Bound
        } else {
            ContentChannelPhase::Detached
        };
        Ok(())
    }

    fn validate_pending(
        &self,
        transition: &PendingContentTransition,
    ) -> Result<(), ContentChannelError> {
        if self.phase != ContentChannelPhase::Changing
            || transition.channel_id != self.channel_id
            || transition.channel_generation != self.channel_generation
            || transition.previous_revision != self.content_revision
            || transition.previous_binding != self.binding
        {
            return Err(ContentChannelError::InvalidTransition);
        }
        Ok(())
    }

    pub fn binding(&self) -> Option<&PaneContentBindingV1> {
        self.binding.as_ref()
    }

    pub fn phase(&self) -> ContentChannelPhase {
        self.phase
    }

    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    pub fn channel_generation(&self) -> u64 {
        self.channel_generation
    }
}

fn transition_for(
    command: &PaneContentCommandV1,
    current: Option<&PaneContentBindingV1>,
) -> (Option<PaneContentBindingV1>, ContentChannelAction) {
    match command {
        PaneContentCommandV1::Bind { binding } => match current {
            None => (
                Some(binding.clone()),
                ContentChannelAction::Attach {
                    binding: binding.clone(),
                },
            ),
            Some(existing) if existing == binding => (
                Some(binding.clone()),
                ContentChannelAction::KeepBinding {
                    binding: binding.clone(),
                },
            ),
            Some(existing) if existing.execution_target == binding.execution_target => (
                Some(binding.clone()),
                ContentChannelAction::SwitchClient {
                    from: existing.clone(),
                    to: binding.clone(),
                },
            ),
            Some(existing) => (
                Some(binding.clone()),
                ContentChannelAction::ReplaceChild {
                    from: existing.clone(),
                    to: binding.clone(),
                },
            ),
        },
        PaneContentCommandV1::Reconnect { binding } => (
            Some(binding.clone()),
            match current {
                Some(_) => ContentChannelAction::RestartChild {
                    binding: binding.clone(),
                },
                None => ContentChannelAction::Attach {
                    binding: binding.clone(),
                },
            },
        ),
        PaneContentCommandV1::Detach => match current {
            Some(existing) => (
                None,
                ContentChannelAction::Detach {
                    from: existing.clone(),
                },
            ),
            None => (None, ContentChannelAction::KeepDetached),
        },
        // Recovery observation/control never mutates binding or revision.
        // The pane-host handles these before entering a content transition.
        PaneContentCommandV1::ObserveRecovery
        | PaneContentCommandV1::UnobserveRecovery
        | PaneContentCommandV1::Focus { .. }
        | PaneContentCommandV1::TerminalControl { .. }
        | PaneContentCommandV1::ProbeTerminalControl
        | PaneContentCommandV1::ProbeRecovery
        | PaneContentCommandV1::DismissRecovery => (
            current.cloned(),
            match current {
                Some(binding) => ContentChannelAction::KeepBinding {
                    binding: binding.clone(),
                },
                None => ContentChannelAction::KeepDetached,
            },
        ),
        PaneContentCommandV1::Retire => (
            None,
            ContentChannelAction::Retire {
                from: current.cloned(),
            },
        ),
    }
}
