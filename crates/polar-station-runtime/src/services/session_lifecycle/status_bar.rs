use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::future::join_all;
use polar_station_protocol::{
    PaneContentBindingV1, SshTransportGenerationV1, TmuxStatusPurposeEditV1,
};
use polar_station_settings::{TmuxStatusBarSettings, TmuxStatusFacts, TmuxStatusField};
use polar_station_workspace::{
    ManagedSessionRecord, ProcessRestoreSpec, SessionKind, SessionLaunchSpec, SessionStartupGit,
};
use polar_tmux_core::{TmuxClient, TmuxSessionBinding};

use super::SessionLifecycle;
use crate::services::{settings_service::SettingsSessionAccess, terminal_source};

const STARTUP_GIT_PROBE: &str = include_str!("startup_git_probe.sh");

#[derive(Default)]
struct StatusContext<'a> {
    workspace: Option<&'a str>,
    title: Option<&'a str>,
    observed_permission: Option<&'a str>,
}

pub(super) async fn read_startup_git(
    client: &TmuxClient,
    directory: Option<&str>,
) -> Option<SessionStartupGit> {
    let directory = polar_ssh::quote_posix(directory?);
    let script = format!(
        "set -- {directory} /usr/bin/git /usr/bin/xcode-select\n{STARTUP_GIT_PROBE}",
    );
    let output = tokio::time::timeout(
        Duration::from_secs(1),
        crate::services::target_directory::target_output(client.target(), &script),
    )
    .await
    .ok()?
    .ok()?;
    let output = std::str::from_utf8(&output).ok()?;
    let (root, branch) = output.split_once('\0')?;
    let worktree = std::path::Path::new(root).file_name()?.to_str()?.to_owned();
    Some(SessionStartupGit {
        worktree,
        branch: (!branch.is_empty()).then(|| branch.to_owned()),
    })
}

impl SessionLifecycle {
    pub(super) async fn configure_status_bar(
        &self,
        client: &TmuxClient,
        record: &ManagedSessionRecord,
        title: Option<&str>,
    ) -> Result<()> {
        let selected = self.settings.load()?.unwrap_or_default().tmux_status_bar;
        let session = record.runtime_tmux_session_id.as_str();
        let facts = collect_facts(
            client,
            &selected,
            session,
            &record.launch.session_name,
            record.created_unix_seconds,
            Some(&record.launch),
            StatusContext {
                title,
                ..StatusContext::default()
            },
        )
        .await;
        client
            .set_session_status_line(session, &selected.render_facts(&facts))
            .await?;
        Ok(())
    }

    pub(in crate::services) async fn refresh_workspace_status_bar(
        &self,
        access: SettingsSessionAccess<'_>,
        bindings: Vec<PaneContentBindingV1>,
        selected: &TmuxStatusBarSettings,
        workspace: Option<&str>,
        purpose_edit: Option<&TmuxStatusPurposeEditV1>,
    ) -> Option<String> {
        let mut by_target = BTreeMap::<_, Vec<_>>::new();
        for binding in bindings {
            let scoped = by_target
                .entry(binding.execution_target.clone())
                .or_default();
            if !scoped.contains(&binding) {
                scoped.push(binding);
            }
        }
        let failed = join_all(by_target.into_iter().map(|(target, bindings)| async move {
            let id = target.as_str();
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let registered = access.targets.get(id).context("Execution target is unavailable")?;
                let gate = registered.operation_gate();
                let _operation = gate.read().await;
                let client = registered.managed_client(access.ssh).await?;
                let transport = access.ssh.transport_generation(id).await;
                let mut updated = BTreeSet::new();
                let mut complete = true;
                let mut bindings = bindings;
                if let Some(edit) = purpose_edit {
                    if let Some(index) = bindings.iter().position(|pane| pane == &edit.pane) {
                        bindings.swap(0, index);
                    }
                }
                for pane in bindings {
                    let result = async {
                        let identity = self.status_binding(access, &pane, transport.as_ref()).await?;
                        if updated.contains(&identity.session_id) {
                            return Ok(());
                        }
                        let purpose = purpose_edit.filter(|edit| edit.pane == pane).map(|edit| edit.text.trim());
                        let mut facts = self.bound_status_facts(access, &client, &pane, &identity, selected, workspace).await?;
                        if let Some(purpose) = purpose {
                            facts.insert(TmuxStatusField::Purpose, purpose.to_owned());
                        }
                        anyhow::ensure!(access.targets.get(id).is_some_and(|current|
                            current.configuration_revision() == registered.configuration_revision()),
                            "Execution target changed");
                        client.set_bound_session_status(&identity, &selected.render_facts(&facts), purpose).await?;
                        updated.insert(identity.session_id);
                        Ok::<_, anyhow::Error>(())
                    }.await;
                    if let Err(error) = result {
                        complete = false;
                        tracing::warn!(%error, target_id = id, "Could not refresh tmux Session status");
                    }
                }
                Ok::<_, anyhow::Error>(complete)
            }).await;
            match result {
                Ok(Ok(true)) => None,
                Ok(Ok(false)) => Some(target),
                Ok(Err(error)) => {
                    tracing::warn!(%error, target_id = id, "Could not refresh target's tmux Session status");
                    Some(target)
                }
                Err(error) => {
                    tracing::warn!(%error, target_id = id, "Timed out refreshing target's tmux Session status");
                    Some(target)
                }
            }
        })).await;
        let failed: Vec<_> = failed
            .iter()
            .flatten()
            .map(|target| target.as_str())
            .collect();
        (!failed.is_empty()).then(|| {
            format!(
                "Settings saved. Some open sessions could not refresh on {}. Save again to retry.",
                failed.join(", "),
            )
        })
    }
    pub(in crate::services) async fn preview_status_bar(
        &self,
        access: SettingsSessionAccess<'_>,
        pane: &PaneContentBindingV1,
        workspace: Option<&str>,
    ) -> Result<TmuxStatusFacts> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let id = pane.execution_target.as_str();
            let registered = access
                .targets
                .get(id)
                .context("Execution target is unavailable")?;
            let gate = registered.operation_gate();
            let _operation = gate.read().await;
            let client = registered.managed_client(access.ssh).await?;
            let transport = access.ssh.transport_generation(id).await;
            let identity = self
                .status_binding(access, pane, transport.as_ref())
                .await?;
            let facts = self
                .bound_status_facts(
                    access,
                    &client,
                    pane,
                    &identity,
                    &TmuxStatusBarSettings::all_fields(),
                    workspace,
                )
                .await?;
            anyhow::ensure!(
                access
                    .targets
                    .get(id)
                    .is_some_and(|current| current.configuration_revision()
                        == registered.configuration_revision()),
                "Execution target changed"
            );
            Ok(facts)
        })
        .await?
    }

    async fn status_binding(
        &self,
        access: SettingsSessionAccess<'_>,
        pane: &PaneContentBindingV1,
        transport: Option<&SshTransportGenerationV1>,
    ) -> Result<TmuxSessionBinding> {
        let id = pane.execution_target.as_str();
        let binding = self
            .attachment_binding(id, &pane.tmux_session_id, transport)
            .await?
            .context("The original Session is unavailable")?;
        if let Some(source) = &pane.source {
            terminal_source::check_host(access.targets, id, source)?;
            terminal_source::check_session(source, &binding)?;
        }
        Ok(binding)
    }

    async fn bound_status_facts(
        &self,
        access: SettingsSessionAccess<'_>,
        client: &TmuxClient,
        pane: &PaneContentBindingV1,
        identity: &TmuxSessionBinding,
        selected: &TmuxStatusBarSettings,
        workspace: Option<&str>,
    ) -> Result<TmuxStatusFacts> {
        let id = pane.execution_target.as_str();
        let record = self.record_for(id, &pane.tmux_session_id).await;
        let name = self
            .observations
            .lock()
            .await
            .get(id)
            .and_then(|target| {
                target
                    .snapshot
                    .sessions
                    .iter()
                    .find(|session| session.id == identity.session_id)
            })
            .map(|session| session.name.clone())
            .context("Session display name is unavailable")?;
        let launch = record.as_ref().map(|record| &record.launch);
        #[cfg(feature = "codex-control")]
        let metadata = if selected.0.contains(&TmuxStatusField::SessionName)
            || selected.0.contains(&TmuxStatusField::PermissionMode)
        {
            match access.codex {
                Some(codex) => {
                    codex
                        .status_terminal_metadata(id, &pane.tmux_session_id, &name, launch)
                        .await
                }
                None => None,
            }
        } else {
            None
        };
        #[cfg(not(feature = "codex-control"))]
        let metadata: Option<(Option<String>, Option<String>)> = {
            let _ = access;
            None
        };
        let (title, observed_permission) = metadata.unwrap_or_default();
        Ok(collect_facts(
            client,
            selected,
            &identity.session_id,
            &name,
            Some(identity.created_unix_seconds),
            launch,
            StatusContext {
                workspace,
                title: title.as_deref(),
                observed_permission: observed_permission.as_deref(),
            },
        )
        .await)
    }
}

async fn collect_facts(
    client: &TmuxClient,
    selected: &TmuxStatusBarSettings,
    session: &str,
    name: &str,
    created: Option<i64>,
    launch: Option<&SessionLaunchSpec>,
    context: StatusContext<'_>,
) -> TmuxStatusFacts {
    use TmuxStatusField as Field;

    let launch_path = launch.and_then(|launch| launch.working_directory.as_deref());
    let convention = SessionKind::from_terminal_name(name);
    let kind = convention
        .map(|(kind, _)| kind)
        .or_else(|| launch.map(|value| SessionKind::from_restore_spec(&value.process)))
        .unwrap_or(SessionKind::Terminal);
    let is_terminal = matches!(kind, SessionKind::Terminal | SessionKind::Command);
    let needs_window_title = !is_terminal
        && context.title.is_none()
        && selected.0.contains(&Field::SessionName);
    let saved_git = launch.and_then(|launch| launch.startup_git.as_ref());
    let needs_git = saved_git.is_none()
        && (selected.0.contains(&Field::StartupBranch) || selected.0.contains(&Field::Worktree));
    let needs_directory = launch_path.is_none()
        && (needs_git
            || selected.0.contains(&Field::Project)
            || selected.0.contains(&Field::StartupPath));
    let startup = if needs_window_title
        || needs_directory
        || selected.0.contains(&Field::Host)
        || selected.0.contains(&Field::Program)
        || selected.0.contains(&Field::User)
        || selected.0.contains(&Field::Purpose)
    {
        client
            .startup_display_facts(session)
            .await
            .unwrap_or_default()
    } else {
        Default::default()
    };
    let path = launch_path
        .or_else(|| (!startup.directory.is_empty()).then_some(startup.directory.as_str()));
    let observed_git = if needs_git {
        read_startup_git(client, path).await
    } else {
        None
    };
    let observed_branch = observed_git
        .as_ref()
        .and_then(|git| git.branch.as_ref())
        .map(|branch| format!("{branch} (current)"));
    let branch = saved_git
        .and_then(|git| git.branch.as_deref())
        .or(observed_branch.as_deref());
    let worktree = saved_git
        .or(observed_git.as_ref())
        .map(|git| git.worktree.as_str());
    let folder = path.map(|path| {
        std::path::Path::new(path)
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or(path)
    });
    let original_id = convention
        .map(|(_, id)| id)
        .or_else(|| launch.and_then(|value| value.process.original_agent_id()));
    let display_name = if is_terminal {
        Some(name)
    } else {
        [context.title, Some(startup.explicit_window_name.as_str())]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|title| {
                !title.is_empty()
                    && *title != name
                    && *title != session
                    && Some(*title) != original_id
            })
    };
    let (session_type, program) = match kind {
        SessionKind::Terminal | SessionKind::Command => (
            if kind == SessionKind::Command { "Command" } else { "Shell" },
            startup
                .command
                .split_whitespace()
                .next()
                .filter(|value| !value.contains(['\'', '"', '$', '=']))
                .and_then(|value| std::path::Path::new(value).file_name()?.to_str()),
        ),
        SessionKind::CodexPlane => ("Codex", Some("codex")),
        SessionKind::ClaudePlane => ("Claude", Some("claude")),
    };
    let created = created
        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        });
    let legacy_permission = match launch.map(|value| &value.process) {
        Some(ProcessRestoreSpec::Codex { yolo: true, .. }) => Some("YOLO"),
        Some(ProcessRestoreSpec::Codex { yolo: false, .. }) => Some("Default"),
        _ => None,
    };
    let observed_permission = context
        .observed_permission
        .map(|mode| format!("{mode} (observed)"));
    let permission = launch
        .and_then(|launch| launch.startup_permission.as_deref())
        .or(legacy_permission)
        .or(observed_permission.as_deref());
    selected
        .0
        .iter()
        .filter_map(|&field| {
            let value: &str = match field {
                Field::Connection => None,
                Field::Host => Some(startup.host.as_str()),
                Field::Project => folder,
                Field::SessionType => Some(session_type),
                Field::SessionName => display_name,
                Field::StartupPath => path,
                Field::Program => program,
                Field::User => Some(startup.user.as_str()),
                Field::PermissionMode => permission,
                Field::CreatedAt => created.as_deref(),
                Field::SessionId => Some(session),
                Field::StartupBranch => branch,
                Field::Worktree => worktree,
                Field::Workspace => context.workspace,
                Field::Purpose => Some(startup.purpose.as_str()),
            }?;
            let value: String = value
                .chars()
                .filter(|character| !character.is_control())
                .collect();
            (!value.trim().is_empty()).then_some((field, value))
        })
        .collect()
}


