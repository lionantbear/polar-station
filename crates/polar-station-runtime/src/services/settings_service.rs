use std::sync::Arc;
use std::time::Duration;

use polar_station_protocol::{
    ApplicationStateApiV1, CodexNotifyConfigStateV1, ProtocolError, SettingsCommandV1,
    SettingsRequestEnvelopeV1, SettingsResponseEnvelopeV1, SettingsResultV1, PROTOCOL_VERSION,
};
use polar_station_settings::{
    SettingsRepository, SettingsSectionUpdateV1, SettingsV1, TmuxStatusFacts, UpdateError,
};

use super::{
    application_state::ApplicationStatePublisher,
    codex_config::{
        configure_codex_notify, default_codex_config_path, inspect_codex_notify,
        station_notify_command, CodexNotifyConfigStatus,
    },
    session_lifecycle::SessionLifecycle,
    session_monitor::SessionMonitorSupervisor,
    session_registry::SessionRegistry,
    session_summary_provider::CodexAppServerSummaryProvider,
};
use crate::{ssh_connection_manager::SshConnectionManager, target_registry::TargetRegistry};

#[derive(Clone, Copy)]
pub struct SettingsSessionAccess<'a> {
    pub lifecycle: &'a SessionLifecycle,
    pub targets: &'a TargetRegistry,
    pub ssh: &'a SshConnectionManager,
    #[cfg(feature = "codex-control")]
    pub codex: Option<&'a super::codex_control::CodexControlService>,
}

type SettingsOutcome = Result<(u64, SettingsResultV1), (&'static str, String, bool)>;

#[derive(Clone)]
pub struct SettingsService {
    repository: Arc<SettingsRepository>,
    sessions: SessionRegistry,
    session_monitors: SessionMonitorSupervisor,
    events: ApplicationStatePublisher,
    tmux_status_save: Arc<tokio::sync::Mutex<()>>,
}

impl SettingsService {
    pub async fn open(
        repository: SettingsRepository,
        sessions: SessionRegistry,
        session_monitors: SessionMonitorSupervisor,
        events: ApplicationStatePublisher,
    ) -> anyhow::Result<Self> {
        let settings = repository.load_or_initialize()?;
        let provider = CodexAppServerSummaryProvider::from_settings(&settings.session_summary)
            .map_err(anyhow::Error::msg)?;
        sessions
            .set_summary_provider(
                provider,
                Duration::from_millis(settings.monitoring.quiet_period_ms),
            )
            .await;
        session_monitors
            .configure(
                settings.monitoring.enabled,
                settings.monitoring.codex_title_detection,
            )
            .await;
        Ok(Self {
            repository: Arc::new(repository),
            sessions,
            session_monitors,
            events,
            tmux_status_save: Arc::default(),
        })
    }

    pub async fn dispatch(
        &self,
        request: SettingsRequestEnvelopeV1,
        session_access: SettingsSessionAccess<'_>,
    ) -> SettingsResponseEnvelopeV1 {
        let request_id = request.request_id.clone();
        let outcome = match request.command {
            SettingsCommandV1::GetProjection { tmux_preview, workspace_name } => {
                let facts = match tmux_preview {
                    Some(pane) => session_access
                        .lifecycle
                        .preview_status_bar(session_access, &pane, workspace_name.as_deref())
                        .await
                        .ok(),
                    None => None,
                };
                self.projection_result(facts)
            }
            SettingsCommandV1::UpdateAppearance { value } => {
                self.update(
                    request.expected_revision,
                    SettingsSectionUpdateV1::Appearance(value),
                )
                .await
            }
            SettingsCommandV1::UpdateAttention { value } => {
                self.update(
                    request.expected_revision,
                    SettingsSectionUpdateV1::Attention(value),
                )
                .await
            }
            SettingsCommandV1::UpdateMonitoring { value } => {
                self.update(
                    request.expected_revision,
                    SettingsSectionUpdateV1::Monitoring(value),
                )
                .await
            }
            SettingsCommandV1::UpdateSessionSummary { value } => {
                self.update(
                    request.expected_revision,
                    SettingsSectionUpdateV1::SessionSummary(value),
                )
                .await
            }
            SettingsCommandV1::UpdateTmuxStatusBar {
                value,
                workspace_sessions,
                workspace_name,
                tmux_purpose,
            } => {
                let _save = self.tmux_status_save.lock().await;
                match self.save(
                    request.expected_revision,
                    SettingsSectionUpdateV1::TmuxStatusBar(value),
                ) {
                    Ok(settings) => {
                        let warning = session_access
                            .lifecycle
                            .refresh_workspace_status_bar(
                                session_access,
                                workspace_sessions,
                                &settings.tmux_status_bar,
                                workspace_name.as_deref(),
                                tmux_purpose.as_deref(),
                            )
                            .await;
                        self.events.publish(ApplicationStateApiV1::SettingsV1);
                        Ok(Self::project(settings, warning, None))
                    }
                    Err(error) => Err(error),
                }
            }
            SettingsCommandV1::TestSessionSummaryProvider { value } => {
                match CodexAppServerSummaryProvider::test_settings(&value).await {
                    Ok(summary) => Ok((0, SettingsResultV1::ProviderTested { summary })),
                    Err(message) => Err(("provider_test_failed", message, true)),
                }
            }
            SettingsCommandV1::GetCodexNotifyStatus {
                execution_target_id,
            } => Ok((0, codex_notify_status(&execution_target_id, false))),
            SettingsCommandV1::ConfigureCodexNotify {
                execution_target_id,
            } => Ok((0, codex_notify_status(&execution_target_id, true))),
        };
        let (revision, result) = match outcome {
            Ok(value) => value,
            Err((code, message, retryable)) => {
                let projection = self.repository.load().ok().flatten();
                let revision = projection.as_ref().map_or(0, |value| value.revision);
                (
                    revision,
                    SettingsResultV1::Error {
                        error: ProtocolError::new(code, &message, retryable),
                        tmux_status_bar: projection
                            .as_ref()
                            .map(|settings| settings.tmux_status_bar.clone()),
                        projection,
                    },
                )
            }
        };
        SettingsResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: request.api,
            request_id,
            revision,
            result,
        }
    }

    fn projection_result(&self, facts: Option<TmuxStatusFacts>) -> SettingsOutcome {
        let projection = self
            .repository
            .load()
            .map_err(|error| ("settings_unavailable", error.to_string(), true))?
            .unwrap_or_default();
        Ok(Self::project(projection, None, facts))
    }

    async fn update(
        &self,
        expected_revision: Option<u64>,
        update: SettingsSectionUpdateV1,
    ) -> SettingsOutcome {
        let settings = self.save(expected_revision, update)?;
        let provider = CodexAppServerSummaryProvider::from_settings(&settings.session_summary)
            .map_err(|message| ("provider_configuration_invalid", message, false))?;
        self.sessions
            .set_summary_provider(
                provider,
                Duration::from_millis(settings.monitoring.quiet_period_ms),
            )
            .await;
        self.session_monitors
            .configure(
                settings.monitoring.enabled,
                settings.monitoring.codex_title_detection,
            )
            .await;
        self.events.publish(ApplicationStateApiV1::SettingsV1);
        Ok(Self::project(settings, None, None))
    }

    fn save(
        &self,
        expected_revision: Option<u64>,
        update: SettingsSectionUpdateV1,
    ) -> Result<SettingsV1, (&'static str, String, bool)> {
        let expected = expected_revision.ok_or((
            "revision_required",
            "Settings update requires a revision".to_owned(),
            false,
        ))?;
        match self.repository.update_section(expected, update) {
            Ok(settings) => Ok(settings),
            Err(UpdateError::RevisionConflict { .. }) => Err((
                "revision_conflict",
                "Settings changed; reload and retry".to_owned(),
                true,
            )),
            Err(error) => Err(("settings_update_failed", error.to_string(), true)),
        }
    }

    fn project(
        settings: SettingsV1,
        tmux_status_warning: Option<String>,
        tmux_status_facts: Option<TmuxStatusFacts>,
    ) -> (u64, SettingsResultV1) {
        (
            settings.revision,
            SettingsResultV1::Projection {
                tmux_status_bar: Some(settings.tmux_status_bar.clone()),
                tmux_status_warning,
                tmux_status_facts,
                theme_projection: Box::new(settings.appearance.theme.projection()),
                projection: settings,
            },
        )
    }
}

fn codex_notify_status(execution_target_id: &str, configure: bool) -> SettingsResultV1 {
    let status = if execution_target_id != "local" {
        CodexNotifyConfigStateV1::Unsupported
    } else {
        let status = default_codex_config_path()
            .and_then(|path| {
                let command = station_notify_command()?;
                if configure {
                    configure_codex_notify(&path, &command)
                } else {
                    Ok(inspect_codex_notify(&path, &command))
                }
            })
            .unwrap_or(CodexNotifyConfigStatus::Invalid);
        match status {
            CodexNotifyConfigStatus::Missing => CodexNotifyConfigStateV1::Missing,
            CodexNotifyConfigStatus::Configured => CodexNotifyConfigStateV1::Configured,
            CodexNotifyConfigStatus::Conflict => CodexNotifyConfigStateV1::Conflict,
            CodexNotifyConfigStatus::Invalid => CodexNotifyConfigStateV1::Invalid,
            CodexNotifyConfigStatus::Unsafe => CodexNotifyConfigStateV1::Unsafe,
        }
    };
    SettingsResultV1::CodexNotifyStatus {
        execution_target_id: execution_target_id.to_owned(),
        status,
    }
}
