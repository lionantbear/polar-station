//! Client-only preferences use the existing local atomic file writer. Host
//! revisions remain Host revisions; only the presented Settings view is local.
use super::*;
use polar_station_protocol::{
    AppearanceSettingsV1, HostApplicationMessage, SettingsRequestEnvelopeV1,
    SettingsResponseEnvelopeV1, SettingsResultV1, SettingsV1,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
    pub appearance: Option<AppearanceSettingsV1>,
    pub selected_backend: Option<backend::Endpoint>,
}
impl Preferences {
    fn validate(&self) -> Result<()> {
        if let Some(value) = &self.appearance {
            value.validate()?;
        }
        if let Some(endpoint) = &self.selected_backend {
            anyhow::ensure!(
                endpoint.socket_path.is_absolute(),
                "Invalid saved backend path"
            );
            polar_station_protocol::validate_stable_identifier(
                endpoint
                    .host_id
                    .as_deref()
                    .context("Saved backend lacks identity")?,
                "saved backend",
            )?;
        }
        Ok(())
    }
}
#[derive(Debug)]
pub(super) enum Change {
    Initialize(AppearanceSettingsV1),
    Appearance(SettingsRequestEnvelopeV1),
    Connected(backend::Endpoint),
}
pub(super) async fn load(path: Option<PathBuf>) -> Result<Preferences> {
    let Some(path) = path else {
        return Ok(Preferences::default());
    };
    tokio::task::spawn_blocking(move || {
        let document = crate::native_state_file::exchange(&path, None, 64 * 1024)?;
        let value: Preferences = match document {
            Some(text) => serde_json::from_str(&text)?,
            None => Preferences::default(),
        };
        value.validate()?;
        Ok(value)
    })
    .await?
}
pub(super) async fn save(
    change: Change,
    path: Option<PathBuf>,
    backend: &Arc<Mutex<backend::BackendState>>,
    state: &Arc<Mutex<DesktopClientStateMachine>>,
    wake: &Wake,
) {
    let mut next = backend
        .lock()
        .unwrap_or_else(|v| v.into_inner())
        .preferences
        .clone();
    let request = match &change {
        Change::Initialize(value) => {
            if next.appearance.is_some() {
                return;
            }
            next.appearance = Some(value.clone());
            None
        }
        Change::Connected(endpoint) => {
            if next.selected_backend.as_ref() == Some(endpoint) {
                return;
            }
            next.selected_backend = Some(endpoint.clone());
            None
        }
        Change::Appearance(request) => {
            let polar_station_protocol::SettingsCommandV1::UpdateAppearance { value } =
                &request.command
            else {
                return;
            };
            next.appearance = Some(value.clone());
            Some(request.clone())
        }
    };
    let written = next.clone();
    let result: Result<()> = async {
        next.validate()?;
        let path = path.context("Client preference storage is not configured")?;
        let document = serde_json::to_string(&next)?;
        tokio::task::spawn_blocking(move || {
            crate::native_state_file::exchange(&path, Some(&document), 64 * 1024)
        })
        .await??;
        Ok(())
    }
    .await;
    if result.is_ok() {
        backend
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .preferences = written.clone();
    }
    let mut state = state.lock().unwrap_or_else(|v| v.into_inner());
    if let Some(request) = request {
        let result = match result {
            Ok(()) => {
                let mut projection = state.settings_snapshot.clone().unwrap_or_default();
                projection.appearance = written.appearance.unwrap_or_default();
                state.present_settings(&mut projection);
                SettingsResultV1::Projection {
                    theme_projection: Box::new(projection.appearance.theme.projection()),
                    projection,
                }
            }
            Err(error) => SettingsResultV1::Error {
                error: ProtocolError::new("client_preferences_failed", error.to_string(), false),
                projection: state.settings_snapshot.clone(),
            },
        };
        let frame = LocalHostFrame::ApplicationResponse {
            response: HostApplicationMessage::SettingsV1(Box::new(SettingsResponseEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: request.api,
                request_id: request.request_id,
                revision: state.settings_revision,
                result,
            })),
        };
        let _ = state.requests.response(&frame);
        let generation = state.current_generation();
        let _ = state.accept_host_frame(generation, frame);
    } else if let Err(error) = result {
        state
            .content_events
            .push_back(DesktopClientEvent::BackendCompleted {
                request_id: "client-preferences".into(),
                switched: false,
                preview: None,
                error: Some(error.to_string()),
            });
    }
    drop(state);
    wake();
}
impl DesktopClientStateMachine {
    pub(super) fn present_settings(&mut self, projection: &mut SettingsV1) {
        projection.revision = self.settings_revision;
        if self.settings_snapshot.as_ref() != Some(projection) {
            self.settings_revision = self.settings_revision.saturating_add(1).max(1);
            projection.revision = self.settings_revision;
            self.settings_snapshot = Some(projection.clone());
        }
    }
}


