//! Session observation orchestration. Host owns the returned task; this module
//! owns refresh admission, transport events, recovery and projection dependencies.
use super::SessionLifecycle;
use crate::{
    host::{
        control_center_service::ControlCenterService, session_monitor::SessionMonitorSupervisor,
        session_registry::SessionRegistry,
    },
    ssh_connection_manager::{SshConnectionManager, SshConnectionStatus},
    target_registry::TargetRegistry,
};
use anyhow::{Context, Result};
use polar_tmux_protocol::{ExecutionTarget, SessionLifecycleV1};
use std::{sync::Arc, time::Duration};
use tracing::{warn, Instrument};

pub(crate) struct RefreshRuntime {
    pub(crate) targets: TargetRegistry,
    pub(crate) session_lifecycle: Arc<SessionLifecycle>,
    pub(crate) ssh_connections: SshConnectionManager,
    pub(crate) sessions: SessionRegistry,
    pub(crate) session_monitors: SessionMonitorSupervisor,
    pub(crate) control_center: Arc<ControlCenterService>,
}

pub(crate) fn start(state: Arc<RefreshRuntime>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut workers = tokio::task::JoinSet::new();
        let mut created = state.session_lifecycle.subscribe_refresh_requests();
        let mut status_changes = state.ssh_connections.subscribe_status_changes();
        for descriptor in state.targets.descriptors() {
            schedule_session_refresh(&mut workers, &state, descriptor.id, true);
        }

        loop {
            let change = match tokio::select! {
                change = status_changes.recv() => change,
                _ = workers.join_next(), if !workers.is_empty() => { continue; },
                event = created.recv() => {
                    if let Ok(request) = event {
                        match request {
                            crate::host::session_lifecycle::SessionRefreshRequest::Observe(target) => {
                                if let Ok(target) = ExecutionTarget::new(target.to_string()) {
                                    schedule_session_refresh(&mut workers, &state, target, false);
                                }
                            }
                            crate::host::session_lifecycle::SessionRefreshRequest::Project(target) => {
                                if let Ok(target) = ExecutionTarget::new(target.to_string()) {
                                    state.control_center.refresh_target_projection(&target, "client_count");
                                }
                            }
                        }
                    } else if matches!(event, Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) {
                        for descriptor in state.targets.descriptors() {
                            if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(descriptor.id.to_string()) {
                                state.session_lifecycle.request_refresh(target);
                            }
                            schedule_session_refresh(&mut workers, &state, descriptor.id, true);
                        }
                    }
                    continue;
                }

            } {
                Ok(change) => change,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    for descriptor in state.targets.descriptors() {
                        if !descriptor.id.is_local() {
                            if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(
                                descriptor.id.to_string(),
                            ) {
                                state.session_lifecycle.request_refresh(target);
                            }
                            schedule_session_refresh(&mut workers, &state, descriptor.id, true);
                        }
                    }
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let Ok(target) = ExecutionTarget::new(change.target_id.clone()) else {
                continue;
            };
            let Some(change) = state.ssh_connections.current_status_change(target.as_str()) else {
                continue;
            };
            let (lifecycle, generation) = match change.status {
                SshConnectionStatus::Connecting => (SessionLifecycleV1::Connecting, None),
                SshConnectionStatus::Recovering => {
                    let generation = state
                        .ssh_connections
                        .transport_generation(target.as_str())
                        .await;
                    (SessionLifecycleV1::Recovering, generation)
                }
                SshConnectionStatus::Offline => (SessionLifecycleV1::Unavailable, None),
                SshConnectionStatus::Connected => {
                    let generation = state
                        .ssh_connections
                        .transport_generation(target.as_str())
                        .await;
                    (SessionLifecycleV1::Recovering, generation)
                }
            };
            if let Err(error) = state
                .sessions
                .mark_target_lifecycle(&target, lifecycle, generation)
                .await
            {
                warn!(execution_target_id = %target, %error, "Session lifecycle event was rejected");
            }
            if change.status != SshConnectionStatus::Connected {
                state
                    .session_lifecycle
                    .mark_unavailable(target.as_str())
                    .await;
                state.session_monitors.stop_target(&target).await;
            }
            state.control_center.refresh_target_projection(&target, "transport");
            if change.status == SshConnectionStatus::Connected {
                // The active worker already awaits this lease and validates its
                // generation after observation. Do not queue its own scan twice.
                if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(target.to_string())
                {
                    state.session_lifecycle.request_refresh_if_idle(target);
                }
            }
        }
    })
}

fn schedule_session_refresh(
    workers: &mut tokio::task::JoinSet<()>,
    state: &Arc<RefreshRuntime>,
    target: ExecutionTarget,
    initial: bool,
) {
    let admitted = if initial {
        state.session_lifecycle.begin_refresh(target.as_str())
    } else {
        state
            .session_lifecycle
            .begin_queued_refresh(target.as_str())
    };
    if admitted {
        let state = state.clone();
        workers.spawn(async move {
            refresh_session_registry_target(&state, &target).await;
        });
    }
}

async fn refresh_session_registry_target(state: &RefreshRuntime, target_id: &ExecutionTarget) {
    let owner = &state.session_lifecycle;
    let mut requests = owner.subscribe_refresh_requests();
    let mut transports = state.ssh_connections.subscribe_status_changes();
    let mut retry = 0usize;
    let retry_delays = [1_u64, 2, 5, 10, 30];
    loop {
        let result: Result<()> = async {
            let target = state
                .targets
                .get(target_id.as_str())
                .context("Session target missing")?;
            let gate = target.operation_gate();
            let client = target.managed_client(&state.ssh_connections).await?;
            let generation = state
                .ssh_connections
                .transport_generation(target_id.as_str())
                .await;
            let monitors = owner
                .refresh_target(
                    &client,
                    target_id,
                    &target.descriptor().display_name,
                    &state.sessions,
                    generation.clone(),
                    &state.ssh_connections,
                    &gate,
                )
                .await?;
            // A transport replacement invalidates even an otherwise valid tmux observation.
            anyhow::ensure!(
                generation
                    == state
                        .ssh_connections
                        .transport_generation(target_id.as_str())
                        .await,
                "Session transport changed during discovery"
            );
            // Valid facts are ready for presentation now. Monitor startup may
            // require further remote round trips and must not delay their view.
            state.control_center.refresh_target_projection(target_id, "observation");
            state
                .session_monitors
                .reconcile_target(
                    target_id,
                    client,
                    monitors,
                    generation,
                    state.sessions.clone(),
                    Some(target.clone()),
                )
                .instrument(tracing::info_span!(target: "polar_station_operation", "session_monitors_reconcile", component="Sessions", target_id=target_id.as_str()))
                .await?;
            Ok(())
        }
        .await;
        let failed = result.is_err();
        if let Err(error) = result {
            warn!(execution_target_id = %target_id, %error, "Session observation deferred");
            owner.mark_observation_failed(target_id.as_str(), &error).await;
            // Only transport events or actual terminal exit may cancel a healthy stream.
            state
                .control_center
                .refresh_target_projection(target_id, "observation");
        }
        if failed && retry < retry_delays.len() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(retry_delays[retry]);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    event = requests.recv() => {
                        match event {
                            Ok(crate::host::session_lifecycle::SessionRefreshRequest::Observe(target)) if target.as_str() == target_id.as_str() => break,
                            Err(_) => break,
                            _ => {},
                        }
                    }
                    event = transports.recv() => {
                        match event {
                            Ok(change) if change.target_id == target_id.as_str() && change.status == SshConnectionStatus::Connected => break,
                            Err(_) => break,
                            _ => {},
                        }
                    }
                }
            }
            retry += 1;
            continue;
        }
        retry = 0;
        if !owner.finish_refresh(target_id.as_str()) {
            break;
        }
    }
}
