use std::sync::Arc;
use std::time::Duration;

use polar_station_settings::{SettingsRepository, SettingsSectionUpdateV1, UpdateError};
use polar_tmux_protocol::{
    ApplicationStateApiV1, CodexNotifyConfigStateV1, ProtocolError, SettingsCommandV1,
    SettingsRequestEnvelopeV1, SettingsResponseEnvelopeV1, SettingsResultV1, PROTOCOL_VERSION,
};

use super::{
    application_state::ApplicationStatePublisher,
    codex_config::{
        configure_codex_notify, default_codex_config_path, inspect_codex_notify,
        station_notify_command, CodexNotifyConfigStatus,
    },
    session_monitor::SessionMonitorSupervisor,
    session_registry::SessionRegistry,
    session_summary_provider::CodexAppServerSummaryProvider,
};

#[derive(Clone)]
pub(crate) struct SettingsService {
    repository: Arc<SettingsRepository>,
    sessions: SessionRegistry,
    session_monitors: SessionMonitorSupervisor,
    events: ApplicationStatePublisher,
}

impl SettingsService {
    pub(crate) async fn open(
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
        })
    }

    pub(crate) async fn dispatch(
        &self,
        request: SettingsRequestEnvelopeV1,
    ) -> SettingsResponseEnvelopeV1 {
        let request_id = request.request_id.clone();
        let outcome = match request.command {
            SettingsCommandV1::GetProjection => self.projection_result(),
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

    fn projection_result(&self) -> Result<(u64, SettingsResultV1), (&'static str, String, bool)> {
        let projection = self
            .repository
            .load()
            .map_err(|error| ("settings_unavailable", error.to_string(), true))?
            .unwrap_or_default();
        Ok((
            projection.revision,
            SettingsResultV1::Projection { theme_projection: Box::new(projection.appearance.theme.projection()), projection },
        ))
    }

    async fn update(
        &self,
        expected_revision: Option<u64>,
        update: SettingsSectionUpdateV1,
    ) -> Result<(u64, SettingsResultV1), (&'static str, String, bool)> {
        let expected = expected_revision.ok_or((
            "revision_required",
            "Settings update requires a revision".to_owned(),
            false,
        ))?;
        let settings = match self.repository.update_section(expected, update) {
            Ok(settings) => settings,
            Err(UpdateError::RevisionConflict { .. }) => {
                return Err((
                    "revision_conflict",
                    "Settings changed; reload and retry".to_owned(),
                    true,
                ));
            }
            Err(error) => return Err(("settings_update_failed", error.to_string(), true)),
        };
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
        Ok((
            settings.revision,
            SettingsResultV1::Projection {
                theme_projection: Box::new(settings.appearance.theme.projection()),
                projection: settings,
            },
        ))
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
