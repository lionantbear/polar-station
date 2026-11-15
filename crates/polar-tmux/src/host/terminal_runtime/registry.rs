use polar_tmux_protocol::PaneContentBindingV1;

use std::{
    collections::HashMap,
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::{oneshot, watch, Mutex};

#[derive(Clone, Default)]
pub(crate) struct TerminalConsumerRegistry {
    active: Arc<Mutex<HashMap<String, ActiveTerminalConsumer>>>,
    selections: Arc<Mutex<HashMap<String, (u64, String)>>>,
    pub(crate) lifetimes: super::TerminalLifetimes,
}

struct ActiveTerminalConsumer {
    generation: u64,
    binding: Option<PaneContentBindingV1>,
    revoke: oneshot::Sender<()>,
    selected: watch::Sender<bool>,
    control_epoch: Arc<AtomicU64>,
    native_instance: Option<String>,
    native_sequence: u64,
}

#[derive(Debug)]
pub(crate) struct TerminalConsumerRegistration {
    revoked: oneshot::Receiver<()>,
    pub(super) selected: watch::Receiver<bool>,
}

impl TerminalConsumerRegistration {
    pub(crate) async fn revoked(&mut self) -> bool {
        (&mut self.revoked).await.is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalConsumerRegistrationError {
    StaleGeneration,
}

impl fmt::Display for TerminalConsumerRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration => formatter
                .write_str("terminal Consumer generation is not newer than the active connection"),
        }
    }
}

impl std::error::Error for TerminalConsumerRegistrationError {}

impl TerminalConsumerRegistry {
    pub(crate) async fn register(
        &self,
        consumer_id: &str,
        generation: u64,
        binding: Option<PaneContentBindingV1>,
    ) -> std::result::Result<TerminalConsumerRegistration, TerminalConsumerRegistrationError> {
        let mut active = self.active.lock().await;
        if active
            .get(consumer_id)
            .is_some_and(|entry| entry.generation >= generation)
        {
            return Err(TerminalConsumerRegistrationError::StaleGeneration);
        }
        let (native_instance, native_sequence) = if let Some(previous) = active.remove(consumer_id)
        {
            previous.control_epoch.fetch_add(1, Ordering::AcqRel);
            let _ = previous.revoke.send(());
            (previous.native_instance, previous.native_sequence)
        } else {
            (None, 0)
        };
        let (revoke, revoked) = oneshot::channel();
        let (selected, selection) = watch::channel(false);
        active.insert(
            consumer_id.to_owned(),
            ActiveTerminalConsumer {
                generation,
                binding,
                revoke,
                selected,
                control_epoch: Arc::new(AtomicU64::new(1)),
                native_instance,
                native_sequence,
            },
        );
        Ok(TerminalConsumerRegistration {
            revoked,
            selected: selection,
        })
    }

    /// Validate native decisions across separate Pane bridges. This registry
    /// never chooses a Pane, stores layouts, or arbitrates external clients.
    pub(super) async fn select(
        &self,
        instance: &str,
        sequence: u64,
        consumer: &str,
        generation: u64,
        focused: bool,
    ) -> Option<polar_tmux_core::TmuxControlEpoch> {
        let mut active = self.active.lock().await;
        let current = active
            .get(consumer)
            .filter(|entry| entry.generation == generation)?;
        // Intent order belongs to each Consumer; selection order spans Panes.
        // A later old-Pane blur may arrive before the new-Pane selection.
        if current.native_instance.as_deref() == Some(instance)
            && current.native_sequence > sequence
        {
            return None;
        }
        let new_intent = current.native_instance.as_deref() != Some(instance)
            || current.native_sequence != sequence;
        let selected = current.selected.clone();
        let epoch = current.control_epoch.clone();
        let mut selections = self.selections.lock().await;
        let previous = selections.get(instance);
        if focused
            && previous.is_some_and(|(old, owner)| {
                *old > sequence || (*old == sequence && owner != consumer)
            })
        {
            return None;
        }
        if let Some((_, old)) = previous.filter(|_| focused) {
            if old != consumer {
                if let Some(entry) = active.get(old) {
                    entry.control_epoch.fetch_add(1, Ordering::AcqRel);
                    entry.selected.send_if_modified(|value| {
                        let changed = *value;
                        *value = false;
                        changed
                    });
                }
            }
        }
        if new_intent {
            epoch.fetch_add(1, Ordering::AcqRel);
        }
        selected.send_if_modified(|value| {
            let changed = *value != focused;
            *value = focused;
            changed
        });
        let entry = active.get_mut(consumer)?;
        if entry.native_instance.as_deref() != Some(instance) {
            entry.native_instance = Some(instance.into());
        }
        entry.native_sequence = sequence;
        if focused {
            selections.insert(instance.into(), (sequence, consumer.into()));
        } else if previous.is_some_and(|(_, owner)| owner != consumer) {
            // Record this Consumer's blur to reject its delayed own focus,
            // without cancelling the selected Pane or advancing its sequence.
            return None;
        }
        Some(polar_tmux_core::TmuxControlEpoch {
            expected: epoch.load(Ordering::Acquire),
            current: epoch,
        })
    }

    pub(crate) async fn update_binding(
        &self,
        consumer_id: &str,
        generation: u64,
        binding: Option<PaneContentBindingV1>,
    ) {
        if let Some(active) = self.active.lock().await.get_mut(consumer_id) {
            if active.generation == generation {
                active.binding = binding;
            }
        }
    }

    

    pub(crate) async fn unregister(&self, consumer_id: &str, generation: u64) {
        let mut active = self.active.lock().await;
        if active
            .get(consumer_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            let removed = active.remove(consumer_id).expect("current consumer exists");
            removed.control_epoch.fetch_add(1, Ordering::AcqRel);
            if let Some(instance) = removed.native_instance {
                if !active
                    .values()
                    .any(|entry| entry.native_instance.as_ref() == Some(&instance))
                {
                    self.selections.lock().await.remove(&instance);
                }
            }
        }
    }

    
}


