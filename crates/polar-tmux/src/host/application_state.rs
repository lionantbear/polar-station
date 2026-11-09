use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use polar_tmux_protocol::{ApplicationStateApiV1, LocalHostFrame};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigurationKind {
    #[cfg(feature = "codex-control")]
    Codex,
    Connectors,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeliveryChannel {
    Telegram,
    Imessage,
    Connector,
}
#[derive(Clone, Debug)]
pub(crate) enum HostChange {
    Projection {
        api: ApplicationStateApiV1,
        state_revision: u64,
    },
    #[cfg(feature = "codex-control")]
    Configuration {
        kind: ConfigurationKind,
        revision: u64,
    },
    #[cfg(feature = "codex-control")]
    Work(DeliveryChannel),
    #[cfg(feature = "codex-control")]
    Recovery,
}
pub(crate) struct ProjectionReceiver(broadcast::Receiver<HostChange>);
impl HostChange {
    fn into_projection(self) -> Option<LocalHostFrame> {
        match self {
            Self::Projection {
                api,
                state_revision,
            } => Some(LocalHostFrame::ApplicationStateChanged {
                api,
                state_revision,
            }),
            #[cfg(feature = "codex-control")]
            _ => None,
        }
    }
}
impl ProjectionReceiver {
    pub(crate) async fn recv(&mut self) -> Result<LocalHostFrame, broadcast::error::RecvError> {
        loop {
            if let Some(frame) = self.0.recv().await?.into_projection() {
                return Ok(frame);
            }
        }
    }
    
}
#[cfg(feature = "codex-control")]
pub(crate) struct RecoveryAfterMutation<'a>(&'a ApplicationStatePublisher);
#[cfg(feature = "codex-control")]
impl Drop for RecoveryAfterMutation<'_> {
    fn drop(&mut self) {
        self.0.recovery();
    }
}
const APPLICATION_STATE_EVENT_CAPACITY: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct ApplicationStatePublisher {
    next_revision: Arc<AtomicU64>,
    sender: broadcast::Sender<HostChange>,
}

impl ApplicationStatePublisher {
    pub(crate) fn new() -> Self {
        let (sender, _) = broadcast::channel(APPLICATION_STATE_EVENT_CAPACITY);
        Self {
            next_revision: Arc::new(AtomicU64::new(1)),
            sender,
        }
    }

    pub(crate) fn subscribe(&self) -> ProjectionReceiver {
        ProjectionReceiver(self.sender.subscribe())
    }
    #[cfg(feature = "codex-control")]
    pub(crate) fn subscribe_work(&self) -> broadcast::Receiver<HostChange> {
        self.sender.subscribe()
    }
    #[cfg(feature = "codex-control")]
    pub(crate) fn recovery_after_mutation(&self) -> RecoveryAfterMutation<'_> {
        RecoveryAfterMutation(self)
    }
    #[cfg(feature = "codex-control")]
    pub(crate) fn recovery(&self) {
        let _ = self.sender.send(HostChange::Recovery);
    }
    pub(crate) fn configuration(&self, kind: ConfigurationKind, revision: u64) {
        #[cfg(feature = "codex-control")]
        let _ = self
            .sender
            .send(HostChange::Configuration { kind, revision });
        #[cfg(not(feature = "codex-control"))]
        let _ = (kind, revision);
    }
    pub(crate) fn work(&self, channel: DeliveryChannel) {
        #[cfg(feature = "codex-control")]
        let _ = self.sender.send(HostChange::Work(channel));
        #[cfg(not(feature = "codex-control"))]
        let _ = channel;
    }

    pub(crate) fn publish(&self, api: ApplicationStateApiV1) -> u64 {
        let state_revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let _ = self.sender.send(HostChange::Projection {
            api,
            state_revision,
        });
        state_revision
    }
}

impl Default for ApplicationStatePublisher {
    fn default() -> Self {
        Self::new()
    }
}



/// Work consumers never interpret native projection invalidation as durable work.
/// Lag is returned to the owner for one bounded ledger reconciliation.
#[cfg(feature = "codex-control")]
pub(crate) async fn wait_for_work(
    receiver: &mut broadcast::Receiver<HostChange>,
    channel: DeliveryChannel,
) -> Result<(), broadcast::error::RecvError> {
    loop {
        match receiver.recv().await? {
            HostChange::Work(actual) if actual == channel => return Ok(()),
            HostChange::Configuration { .. } => return Ok(()),
            _ => {}
        }
    }
}



#[cfg(feature = "codex-control")]
pub(crate) async fn wait_configuration(
    receiver: &mut broadcast::Receiver<HostChange>,
) -> Result<(ConfigurationKind, u64), broadcast::error::RecvError> {
    loop {
        if let HostChange::Configuration { kind, revision } = receiver.recv().await? {
            return Ok((kind, revision));
        }
    }
}

#[cfg(feature = "codex-control")]
pub(crate) async fn wait_observer_change(
    receiver: &mut broadcast::Receiver<HostChange>,
) -> Result<HostChange, broadcast::error::RecvError> {
    loop {
        let event = receiver.recv().await?;
        if matches!(
            event,
            HostChange::Configuration { .. } | HostChange::Recovery
        ) {
            return Ok(event);
        }
    }
}
