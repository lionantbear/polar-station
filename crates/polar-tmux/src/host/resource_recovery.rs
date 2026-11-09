//! One recovery entry point. Each resource owns its effects; catalog/status
//! locks never cover external I/O. watch retains the newest desired state.
use super::{
    application_state::ApplicationStatePublisher, resource_provider::SystemResourceProvider,
    resource_service::append_resource_log,
};
use crate::ssh_connection_manager::{SshConnectionManager, SshConnectionStatus};
use polar_tmux_protocol::{ApplicationStateApiV1, ResourceLogEventV1, ResourceLogLevelV1};
use polar_tmux_resources::{
    specs, ResourceCatalog, ResourceId, ResourceRuntimeState, ResourceRuntimeStatus, ResourceSpec,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::{watch, Mutex, Semaphore};
use tracing::Instrument;

type Statuses = Arc<Mutex<BTreeMap<ResourceId, ResourceRuntimeStatus>>>;
#[derive(Clone)]
struct Publication {
    statuses: Statuses,
    logs: Logs,
    events: ApplicationStatePublisher,
    source: watch::Receiver<Input>,
    pending: Pending,
}

type Pending = Arc<std::sync::Mutex<BTreeSet<(ResourceId, bool)>>>;
type Logs = Arc<Mutex<BTreeMap<ResourceId, VecDeque<ResourceLogEventV1>>>>;

#[derive(Clone, Default, PartialEq, Eq)]
struct Commands {
    restart: u64,
    mirror: u64,
}
#[derive(Clone)]
struct Input {
    catalog: ResourceCatalog,
    commands: BTreeMap<ResourceId, Commands>,
    shutdown: bool,
}
#[derive(Clone, PartialEq, Eq)]
struct Desired {
    spec: Option<ResourceSpec>,
    revision: u64,
    generation: Option<u64>,
    commands: Commands,
}

pub(crate) struct ResourceRecovery {
    provider: SystemResourceProvider,
    ssh: SshConnectionManager,
    input: watch::Sender<Input>,
    done: watch::Sender<bool>,
    started: AtomicBool,
    statuses: Statuses,
    logs: Logs,
    events: ApplicationStatePublisher,
    pending: Pending,
}
impl ResourceRecovery {
    pub(crate) fn new(
        provider: SystemResourceProvider,
        ssh: SshConnectionManager,
        catalog: ResourceCatalog,
        statuses: Statuses,
        logs: Logs,
        events: ApplicationStatePublisher,
    ) -> Self {
        Self {
            provider,
            ssh,
            input: watch::channel(Input {
                catalog,
                commands: BTreeMap::new(),
                shutdown: false,
            })
            .0,
            done: watch::channel(false).0,
            started: AtomicBool::new(false),
            statuses,
            logs,
            events,
            pending: Arc::default(),
        }
    }
    pub(crate) fn apply(&self, catalog: ResourceCatalog) {
        let retained: std::collections::BTreeSet<_> =
            specs(&catalog).iter().map(|spec| spec.id()).collect();
        self.pending
            .lock()
            .expect("Resource pending intents")
            .retain(|(id, _)| retained.contains(id));
        self.input.send_modify(|input| {
            input.catalog = catalog;
            input.commands.retain(|id, _| retained.contains(id));
        });
    }
    pub(crate) fn command(&self, id: ResourceId, mirror: bool) {
        if !self
            .pending
            .lock()
            .expect("Resource pending intents")
            .insert((id, mirror))
        {
            return;
        }
        self.input.send_modify(|input| {
            let commands = input.commands.entry(id).or_default();
            if mirror {
                commands.mirror += 1;
            } else {
                commands.restart += 1;
            }
        });
    }
    pub(crate) async fn shutdown(&self) {
        if !self.started.load(Ordering::Acquire) {
            return;
        }
        let mut done = self.done.subscribe();
        self.input.send_modify(|input| input.shutdown = true);
        let _ = done.wait_for(|value| *value).await;
    }
    pub(crate) fn start(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        // Subscribe before initial work: no lost initial generation notification.
        let mut changes = self.ssh.subscribe_status_changes();
        let mut input = self.input.subscribe();
        let ssh = self.ssh.clone();
        let factory = self.provider.fork();
        let statuses = self.statuses.clone();
        let logs = self.logs.clone();
        let events = self.events.clone();
        let done = self.done.clone();
        let pending = self.pending.clone();
        Some(tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(4));
            let mut workers = BTreeMap::<ResourceId, watch::Sender<Desired>>::new();
            let mut tasks = tokio::task::JoinSet::new();
            let mut ensured_targets = BTreeSet::new();
            loop {
                let snapshot = input.borrow_and_update().clone();
                let desired_specs = if snapshot.shutdown {
                    Vec::new()
                } else {
                    specs(&snapshot.catalog)
                };
                for spec in desired_specs.iter() {
                    let id = spec.id();
                    if spec.enabled() && ensured_targets.insert(spec.target_id().to_owned()) {
                        factory.ensure_connection(spec.target_id());
                    }
                    let generation =
                        ssh.current_status_change(spec.target_id())
                            .and_then(|status| {
                                (status.status == SshConnectionStatus::Connected)
                                    .then_some(status.generation)
                                    .flatten()
                            });
                    let desired = Desired {
                        spec: Some(spec.clone()),
                        revision: snapshot.catalog.revision,
                        generation,
                        commands: snapshot.commands.get(&id).cloned().unwrap_or_default(),
                    };
                    if let Some(sender) = workers.get(&id) {
                        sender.send_if_modified(|old| {
                            if *old == desired {
                                false
                            } else {
                                *old = desired;
                                true
                            }
                        });
                    } else {
                        let (sender, receiver) = watch::channel(desired);
                        workers.insert(id, sender);
                        let provider = factory.fork();
                        let permits = permits.clone();
                        let publication = Publication {
                            statuses: statuses.clone(),
                            logs: logs.clone(),
                            events: events.clone(),
                            source: input.clone(),
                            pending: pending.clone(),
                        };
                        tasks.spawn(async move {
                            run_resource(id, provider, receiver, permits, publication).await;
                            id
                        });
                    }
                }
                // Keep retired entries until cleanup completes; they remain available
                // for an explicit retry instead of losing blocked mount ownership.
                for (id, sender) in &workers {
                    if !desired_specs.iter().any(|spec| spec.id() == *id) {
                        sender.send_if_modified(|old| {
                            if old.spec.is_none() {
                                false
                            } else {
                                old.spec = None;
                                old.revision = snapshot.catalog.revision;
                                true
                            }
                        });
                    }
                }
                if snapshot.shutdown {
                    break;
                }
                tokio::select! {
                    result = input.changed() => if result.is_err() { break; },
                    result = changes.recv() => if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; },
                    result = tasks.join_next(), if !tasks.is_empty() => {
                        match result { Some(Ok(id)) => { workers.remove(&id); }, Some(Err(error)) => tracing::error!(%error, "Resource worker failed"), None => {} }
                    }
                }
            }
            drop(workers);
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    tracing::error!(%error, "Resource cleanup worker failed");
                }
            }
            done.send_replace(true);
        }))
    }
}

async fn run_resource<P: ResourceEffects>(
    id: ResourceId,
    mut provider: P,
    mut desired: watch::Receiver<Desired>,
    permits: Arc<Semaphore>,
    publication: Publication,
) {
    let Publication {
        statuses,
        logs,
        events,
        source,
        pending,
    } = publication;
    let mut previous = None;
    let mut handled = Commands::default();
    let mut ordinal = 0u64;
    let mut child_exit = false;
    let mut old_generation = 0;
    loop {
        let queued = Instant::now();
        let permit = permits
            .acquire()
            .await
            .expect("Resource semaphore lives with worker");
        let work = desired.borrow_and_update().clone();
        ordinal += 1;
        let operation = format!("{id}-{ordinal}");
        let span = tracing::info_span!(target:"polar_station_operation", "resource_reconcile", component="Resources",
            resource_id=id.to_string(), operation_id=operation, catalog_revision=work.revision,
            old_generation, new_generation=work.generation.unwrap_or(0), generation=work.generation.unwrap_or(0), wait_us=queued.elapsed().as_micros() as u64, outcome="cancelled");
        let next = async {
            if child_exit {
                provider.process_exit().await;
            }
            provider
                .update(
                    id,
                    previous.as_ref(),
                    work.spec.clone(),
                    work.generation,
                    work.commands.restart != handled.restart,
                    work.commands.mirror != handled.mirror,
                )
                .await
        }
        .instrument(span.clone())
        .await;
        old_generation = next.generation;
        previous = work.spec.clone();
        child_exit = false;
        let watch_child = next.state == ResourceRuntimeState::Running;
        let current_lease = !watch_child || provider.lease_current().await;
        if *desired.borrow() != work
            || !current_lease
            || !source_matches(&source.borrow(), &work, id)
        {
            span.record("outcome", "superseded");
            provider.discard_obsolete(id).instrument(span.clone()).await;
            previous = None;
            drop(permit);
            if *desired.borrow() != work {
                continue;
            }
            // The SSH notification may still be queued at the coordinator.
        } else {
            let level = if next.state == ResourceRuntimeState::NeedsAttention {
                ResourceLogLevelV1::Error
            } else {
                ResourceLogLevelV1::Info
            };
            span.record(
                "outcome",
                if level == ResourceLogLevelV1::Error {
                    "failed"
                } else {
                    "ok"
                },
            );
            // Fence publication while holding the watch read guard, without awaits.
            let mut current = statuses.lock().await;
            let accepted = {
                let latest = desired.borrow();
                if *latest != work || !source_matches(&source.borrow(), &work, id) {
                    false
                } else {
                    if work.spec.is_some() {
                        current.insert(id, next.clone());
                    } else {
                        current.remove(&id);
                    }
                    true
                }
            };
            drop(current);
            if accepted {
                {
                    let mut pending = pending.lock().expect("Resource pending intents");
                    if handled.restart != work.commands.restart {
                        pending.remove(&(id, false));
                    }
                    if handled.mirror != work.commands.mirror {
                        pending.remove(&(id, true));
                    }
                }
                handled = work.commands.clone();
                append_resource_log(
                    &logs,
                    id,
                    level,
                    format!(
                        "op={ordinal} revision={} generation={} {}",
                        work.revision, next.generation, next.message
                    ),
                )
                .await;
                events.publish(ApplicationStateApiV1::ResourcesV1);
                tracing::info!(target:"polar_station_operation", component="Resources", event="resource_status_published",
                    resource_id=id.to_string(), operation_id=operation, generation=next.generation, catalog_revision=work.revision);
            }
            drop(permit);
            if work.spec.is_none() && next.state == ResourceRuntimeState::Stopped {
                break;
            }
        }
        drop(span); // Operation duration excludes the event-driven idle wait.
        tokio::select! {
            result = desired.changed() => if result.is_err() {
                let _permit = permits.acquire().await.expect("Resource semaphore");
                provider.discard_obsolete(id).await;
                break;
            },
            _ = provider.child_exited(), if watch_child => { child_exit = true; }
        }
    }
}

fn source_matches(input: &Input, work: &Desired, id: ResourceId) -> bool {
    if input.shutdown {
        return work.spec.is_none();
    }
    input.catalog.revision == work.revision
        && specs(&input.catalog)
            .into_iter()
            .find(|spec| spec.id() == id)
            == work.spec
        && input.commands.get(&id).cloned().unwrap_or_default() == work.commands
}

// Effect seam for race/fault tests. Domain decisions remain in resources;
// production delegates directly to the existing platform provider.
trait ResourceEffects: Send + 'static {
    fn update<'a>(
        &'a mut self,
        id: ResourceId,
        previous: Option<&'a ResourceSpec>,
        desired: Option<ResourceSpec>,
        generation: Option<u64>,
        restart: bool,
        mirror: bool,
    ) -> futures_util::future::BoxFuture<'a, ResourceRuntimeStatus>;
    fn discard_obsolete(&mut self, id: ResourceId) -> futures_util::future::BoxFuture<'_, ()>;
    fn child_exited(&mut self) -> futures_util::future::BoxFuture<'_, ()>;
    fn process_exit(&mut self) -> futures_util::future::BoxFuture<'_, ()>;
    fn lease_current(&self) -> futures_util::future::BoxFuture<'_, bool>;
}
impl ResourceEffects for SystemResourceProvider {
    fn update<'a>(
        &'a mut self,
        id: ResourceId,
        previous: Option<&'a ResourceSpec>,
        desired: Option<ResourceSpec>,
        generation: Option<u64>,
        restart: bool,
        mirror: bool,
    ) -> futures_util::future::BoxFuture<'a, ResourceRuntimeStatus> {
        Box::pin(SystemResourceProvider::update(
            self, id, previous, desired, generation, restart, mirror,
        ))
    }
    fn discard_obsolete(&mut self, id: ResourceId) -> futures_util::future::BoxFuture<'_, ()> {
        Box::pin(SystemResourceProvider::discard_obsolete(self, id))
    }
    fn child_exited(&mut self) -> futures_util::future::BoxFuture<'_, ()> {
        Box::pin(SystemResourceProvider::child_exited(self))
    }
    fn process_exit(&mut self) -> futures_util::future::BoxFuture<'_, ()> {
        Box::pin(SystemResourceProvider::process_exit(self))
    }
    fn lease_current(&self) -> futures_util::future::BoxFuture<'_, bool> {
        Box::pin(SystemResourceProvider::lease_current(self))
    }
}


