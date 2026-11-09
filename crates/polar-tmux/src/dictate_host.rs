//! Standalone composition. Embedded Polar injects its owner-bound runtime and
//! never loads the daemon user's personal Codex credentials.
mod auth;
mod preview;
use anyhow::{Context, Result};
use polar_dictate::{
    runtime::{DictateRuntime, TranscriptCleanup},
    DictateStore,
};
use polar_voice::{VoiceRuntime, VoiceRuntimeConfig};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub const EXTENSION: &str = "polar.dictate";
pub struct DictateResources {
    pub owner: String,
    pub voice: Arc<VoiceRuntime>,
    pub cleanup: Option<Arc<dyn TranscriptCleanup>>,
    pub on_mount: Option<Box<dyn FnOnce(Arc<DictateRuntime>) + Send>>,
}
pub struct DictateHost {
    settings: Option<polar_station_settings::SettingsRepository>,
    pub runtime: Arc<DictateRuntime>,
    account: Option<Arc<auth::NativeAccount>>,
    login: Mutex<Option<(codex_login::ShutdownHandle, tokio::task::JoinHandle<()>)>>,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for DictateHost {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
        if let Some((cancel, task)) = self.login.get_mut().take() {
            cancel.shutdown();
            task.abort();
        }
    }
}
impl DictateHost {
    pub fn embedded(runtime: Arc<DictateRuntime>) -> Self {
        Self {
            settings: None,
            runtime,
            account: None,
            login: Mutex::new(None),
            worker: None,
        }
    }
    pub async fn from_resources(
        database: PathBuf,
        settings: polar_station_settings::SettingsRepository,
        resources: DictateResources,
    ) -> Result<Self> {
        let store = Arc::new(polar_dictate_store_sqlite::SqliteDictateStore::open(
            database,
            resources.owner,
        )?);
        store.recover().await?;
        store.expire(now_ms()).await?;
        let voice_settings = settings.clone();
        let mut runtime = DictateRuntime::new(store, resources.voice)
            .with_voice_settings(Arc::new(move || Ok(voice_settings.load_voice()?)));
        if let Some(cleanup) = resources.cleanup {
            runtime = runtime.with_cleanup(cleanup);
        }
        let runtime = Arc::new(runtime);
        if let Some(mount) = resources.on_mount {
            mount(runtime.clone());
        }
        let service = runtime.clone();
        let worker = tokio::spawn(async move {
            if service.run().await.is_err() {
                tracing::error!("Dictate storage worker stopped; saved audio retained");
            }
        });
        Ok(Self {
            settings: Some(settings),
            runtime,
            account: None,
            login: Mutex::new(None),
            worker: Some(worker),
        })
    }
    pub async fn standalone(
        database: PathBuf,
        settings: polar_station_settings::SettingsRepository,
        home: PathBuf,
        owner: String,
    ) -> Result<Self> {
        let account = Arc::new(auth::NativeAccount::open(home).await?);
        let voice = Arc::new(VoiceRuntime::new(
            Arc::new(
                polar_voice_provider_codex::dictate::CodexDictateProvider::new(
                    account.credentials.clone(),
                ),
            ),
            VoiceRuntimeConfig {
                maximum_host_concurrency: 1,
                maximum_stream_packets: 30_000,
                provider_timeout: Duration::from_secs(100),
                ..Default::default()
            },
        ).with_streaming_provider(account.credentials.clone()));
        let mut service = Self::from_resources(
            database,
            settings,
            DictateResources {
                owner: owner.clone(),
                voice,
                cleanup: Some(Arc::new(AdamCleanup::new(owner, account.clone())?)),
                on_mount: None,
            },
        )
        .await?;
        service.account = Some(account);
        Ok(service)
    }
    pub async fn request(&self, payload: Value) -> Result<Value> {
        match payload.get("op").and_then(Value::as_str) {
            Some("voice_settings") => Ok(serde_json::to_value(
                self.settings
                    .as_ref()
                    .context("Voice device settings belong to the local capture Host")?
                    .load_voice()?,
            )?),
            Some("voice_settings_update") => {
                let settings: polar_dictate::VoiceSettings = serde_json::from_value(
                    payload
                        .get("settings")
                        .cloned()
                        .context("Missing Voice settings")?,
                )?;
                let saved=self.settings.as_ref()
                    .context("Voice device settings belong to the local capture Host")?
                    .update_voice(settings.revision,settings)?;
                self.runtime.settings_changed();
                Ok(serde_json::to_value(saved)?)
            }
            Some("voice_status") => {
                let status=self.account.as_ref()
                    .context("Account settings are managed by this Polar Host")?
                    .status().await?;
                self.runtime.settings_changed(); // Account reload also fences a prepared transport.
                Ok(status)
            }
            Some("voice_login") => {
                let account = self
                    .account
                    .as_ref()
                    .context("Sign in through this Polar Host's account settings")?;
                let mut active = self.login.lock().await;
                if let Some((cancel, task)) = active.take() {
                    cancel.shutdown();
                    task.abort();
                }
                let server = account.login()?;
                let url = server.auth_url.clone();
                let cancel = server.cancel_handle();
                let task = tokio::spawn(async move {
                    let _ = server.block_until_done().await;
                });
                *active = Some((cancel, task));
                Ok(json!({"authorization_url":url}))
            }
            Some("voice_cancel_login") => {
                if let Some((cancel, task)) = self.login.lock().await.take() {
                    cancel.shutdown();
                    task.abort();
                }
                Ok(json!({"cancelled":true}))
            }
            Some("wait") => {
                let request = serde_json::from_value(payload)?;
                let polar_dictate::api::Response::Recording { recording } =
                    self.runtime.dispatch(request).await?
                else {
                    anyhow::bail!("Invalid Dictate wait response");
                };
                Ok(serde_json::to_value(recording)?)
            }
            _ => {
                let mut request: polar_dictate::api::Request = serde_json::from_value(payload)?;
                if let (
                    Some(settings),
                    polar_dictate::api::Request::Start {
                        cleanup, context, ..
                    },
                ) = (&self.settings, &mut request)
                {
                    let settings = settings.load_voice()?;
                    *cleanup = settings.cleanup;
                    if !settings.cleanup || !settings.context {
                        *context = context.as_ref().map(|_| String::new());
                    }
                }
                Ok(serde_json::to_value(self.runtime.dispatch(request).await?)?)
            }
        }
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
struct AdamCleanupState {
    runner: polar_inhouse_backend_adam::AdamAgentRunner,
    session: polar_text_edit::EditSession,
}
pub struct AdamCleanup {
    state: tokio::sync::Mutex<AdamCleanupState>,
    owner: String,
}
impl AdamCleanup {
    pub fn new(owner: String, credentials: Arc<dyn polar_inhouse_backend_adam::AdamOAuthCredentialResolver>) -> Result<Self> {
        let config = polar_inhouse_backend_adam::parse_adam_agent_config(
            "dictate",
            r#"{"llm_connection_mode":"openai","model":"gpt-6-luna","reasoning_effort":"low","max_tool_iterations":1}"#,
        )?;
        Ok(Self { owner, state: tokio::sync::Mutex::new(AdamCleanupState {
            runner: polar_inhouse_backend_adam::AdamAgentRunner::new(config).with_oauth_credential_resolver(credentials),
            session: polar_text_edit::EditSession::default(),
        }) })
    }
}
#[async_trait::async_trait]
impl TranscriptCleanup for AdamCleanup {
    async fn clean(&self, request: polar_text_edit::EditRequest) -> Result<String> {
        self.edit(request, None).await
    }
    async fn clean_recording(&self, recording_id: &str, request: polar_text_edit::EditRequest) -> Result<String> {
        self.edit(request, Some(recording_id)).await
    }
    async fn release_recording(&self, recording_id: &str) {
        let mut state = self.state.lock().await;
        state.session.release_scope(recording_id);
    }
}
impl AdamCleanup {
    async fn edit(&self, request: polar_text_edit::EditRequest, recording_id: Option<&str>) -> Result<String> {
        let started = std::time::Instant::now();
        let operation_id = request.operation_id.clone();
        let mut state = self.state.lock().await;
        let config = polar_text_edit::ExecutionConfig {
            turn_ref: polar_inhouse_contract::InhouseBackendTurnRef {
                owner_id: self.owner.clone(),
                thread_id: format!("dictate:{}", recording_id.unwrap_or(&request.operation_id)),
                inhouse_polar_id: "dictate".into(),
                request_message_id: request.operation_id.clone(),
                backend_profile_id: "dictate-cleanup".into(),
            },
            model: Some("gpt-6-luna".into()),
            timeout: Duration::from_secs(8),
            max_text_bytes: 64 * 1024,
            context_window_tokens: 128_000,
            reserve_tokens: 16_000,
        };
        let result = if let Some(id) = recording_id {
            state.session.bind_scope(id);
            let AdamCleanupState { runner, session } = &mut *state;
            polar_text_edit::edit_in_session(runner, session, request, config).await
        } else {
            polar_text_edit::edit(&mut state.runner, request, config).await
        };
        tracing::info!(target: "polar_dictate::latency", operation_id, event="dictate.cleanup",
            work_us=started.elapsed().as_micros() as u64, failed=result.is_err(), "Dictate processing stage");
        Ok(result?.text)
    }
}


