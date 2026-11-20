//! Standalone composition. Embedded Polar injects its owner-bound runtime and
//! never loads the daemon user's personal Codex credentials.
use crate::native_account as auth;
mod learning;
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

pub use polar_station_protocol::DICTATE_EXTENSION as EXTENSION;
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
    learning: Option<Arc<polar_transcreation::learning::LearningRuntime>>,
    learning_worker: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for DictateHost {
    fn drop(&mut self) {
        if let Some(task) = &self.learning_worker {
            task.abort();
        }
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
            learning: None,
            learning_worker: None,
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
            learning: None,
            learning_worker: None,
        })
    }
    pub async fn standalone(
        database: PathBuf,
        settings: polar_station_settings::SettingsRepository,
        home: PathBuf,
        owner: String,
    ) -> Result<Self> {
        let account = Arc::new(auth::NativeAccount::open(home).await?);
        let credentials = Arc::new(polar_voice_provider_codex::CodexVoiceProvider::from_auth_manager(account.manager.clone()));
        let voice = Arc::new(
            VoiceRuntime::new(
                Arc::new(
                    polar_voice_provider_codex::dictate::CodexDictateProvider::new(
                        credentials.clone(),
                    ),
                ),
                VoiceRuntimeConfig {
                    maximum_host_concurrency: 3,
                    maximum_stream_packets: 30_000,
                    provider_timeout: Duration::from_secs(100),
                    ..Default::default()
                },
            )
            .with_streaming_provider(credentials.clone()),
        );
        let learning = Arc::new(polar_transcreation::learning::LearningRuntime::new(
            Arc::new(polar_station_store_sqlite::SqliteLearningStore(
                settings.shared_database(),
            )),
            Arc::new(learning::AdamLearning::new(owner.clone(), account.clone())?),
        ));
        let cleanup =
            AdamCleanup::new(owner.clone(), account.clone())?.with_settings(settings.clone()).with_learning(learning.clone());
        let mut service = Self::from_resources(
            database,
            settings,
            DictateResources {
                owner: owner.clone(),
                voice,
                cleanup: Some(Arc::new(cleanup)),
                on_mount: None,
            },
        )
        .await?;
        service.account = Some(account);
        service.learning = Some(learning.clone());
        service.learning_worker = Some(tokio::spawn(async move {
            if let Err(error) = learning.run().await {
                tracing::error!(%error,"Automatic learning worker stopped");
            }
        }));
        Ok(service)
    }
    pub async fn request(&self, payload: Value) -> Result<Value> {
        match payload.get("op").and_then(Value::as_str) {
            Some("learn_refine") => {
                let sample = serde_json::from_value(payload["sample"].clone())?;
                self.learning
                    .as_ref()
                    .context("Automatic learning is unavailable on this Host")?
                    .admit(sample)
                    .map_err(anyhow::Error::msg)?;
                Ok(json!({"stored":true}))
            }
            Some("refine_learning") => Ok(serde_json::to_value(
                self.learning
                    .as_ref()
                    .context("Automatic learning is unavailable on this Host")?
                    .state()
                    .map_err(anyhow::Error::msg)?,
            )?),
            Some("refine_learning_update") => {
                let change = polar_transcreation::learning::LearningChange::Configure {
                    expected_revision: payload["revision"]
                        .as_u64()
                        .context("Missing learning revision")?,
                    enabled: payload["enabled"]
                        .as_bool()
                        .context("Missing learning enabled state")?,
                    retire: payload["retire"]
                        .as_u64()
                        .map(usize::try_from)
                        .transpose()?,
                    clear: payload["clear"].as_bool().unwrap_or(false),
                    now_ms: now_ms(),
                };
                Ok(serde_json::to_value(
                    self.learning
                        .as_ref()
                        .context("Automatic learning is unavailable on this Host")?
                        .configure(change)
                        .map_err(anyhow::Error::msg)?,
                )?)
            }
            Some("voice_settings") => Ok(serde_json::to_value(
                self.settings
                    .as_ref()
                    .context("Voice device settings belong to the local capture Host")?
                    .load_voice()?,
            )?),
            Some("reference_policy") => {
                let settings = self.settings.as_ref().context("Voice settings belong to the local Host")?.load_voice()?;
                Ok(serde_json::to_value(self.runtime.reference_policy(&settings))?)
            }
            Some("voice_output_modes") => {
                Ok(json!({"choices":polar_dictate::DictateOutputMode::choices()}))
            }
            Some("voice_output_mode_update") => {
                let repository = self
                    .settings
                    .as_ref()
                    .context("Voice settings belong to the local Host")?;
                let mut settings = repository.load_voice()?;
                let mode = serde_json::from_value(payload["mode"].clone())?;
                settings.set_output_mode(mode);
                let saved = repository.update_voice(settings.revision, settings)?;
                self.runtime.settings_changed();
                Ok(serde_json::to_value(saved)?)
            }
            Some("voice_retention_update") => {
                let repository = self
                    .settings
                    .as_ref()
                    .context("Voice settings belong to the local Host")?;
                let mut settings = repository.load_voice()?;
                settings.retention = serde_json::from_value(payload["retention"].clone())?;
                let saved = repository.update_voice(settings.revision, settings)?;
                self.runtime.settings_changed();
                Ok(serde_json::to_value(saved)?)
            }
            Some("voice_transcription_mode_update") => {
                let revision = payload
                    .get("revision")
                    .and_then(Value::as_u64)
                    .context("Missing Voice settings revision")?;
                let mode = serde_json::from_value(
                    payload
                        .get("mode")
                        .cloned()
                        .context("Missing transcription mode")?,
                )?;
                let saved = self
                    .settings
                    .as_ref()
                    .context("Voice device settings belong to the local capture Host")?
                    .update_voice_mode(revision, mode)?;
                self.runtime.settings_changed();
                Ok(serde_json::to_value(saved)?)
            }
            Some("voice_settings_update") => {
                let settings: polar_dictate::VoiceSettings = serde_json::from_value(
                    payload
                        .get("settings")
                        .cloned()
                        .context("Missing Voice settings")?,
                )?;
                let saved = self
                    .settings
                    .as_ref()
                    .context("Voice device settings belong to the local capture Host")?
                    .update_voice(settings.revision, settings)?;
                self.runtime.settings_changed();
                Ok(serde_json::to_value(saved)?)
            }
            Some("voice_status") => self.account.as_ref()
                .context("Account settings are managed by this Polar Host")?.status().await,
            Some("voice_refresh") => {
                let account = self.account.as_ref()
                    .context("Account settings are managed by this Polar Host")?;
                if account.refresh().await? { self.runtime.settings_changed(); }
                account.status().await
            }
            Some("voice_login") => {
                let account = self.account.as_ref()
                    .context("Sign in through this Polar Host's account settings")?.clone();
                let mut active = self.login.lock().await;
                if let Some((cancel, task)) = active.take() {
                    cancel.shutdown(); task.abort(); let _ = task.await;
                }
                let server = match account.login() {
                    Ok(server) => server,
                    Err(error) => {
                        account.set_login_status(auth::LoginStatus::Failed).await;
                        return Err(error);
                    }
                };
                let url = server.auth_url.clone();
                let cancel = server.cancel_handle();
                account.set_login_status(auth::LoginStatus::Pending).await;
                let runtime = self.runtime.clone();
                let task = tokio::spawn(async move {
                    let status = match server.block_until_done().await {
                        Ok(()) => match account.refresh().await {
                            Ok(_) => auth::LoginStatus::Succeeded,
                            Err(_) => auth::LoginStatus::Failed,
                        },
                        Err(_) => auth::LoginStatus::Failed,
                    };
                    account.set_login_status(status).await;
                    runtime.settings_changed();
                });
                *active = Some((cancel, task));
                Ok(json!({"authorization_url":url}))
            }
            Some("voice_cancel_login") => {
                let mut active = self.login.lock().await;
                if let Some((cancel, task)) = active.take() {
                    if !task.is_finished() {
                        cancel.shutdown(); task.abort(); let _ = task.await;
                        if let Some(account) = &self.account {
                            account.set_login_status(auth::LoginStatus::Cancelled).await;
                            self.runtime.settings_changed();
                        }
                    }
                }
                Ok(json!({"cancelled":true}))
            }
            Some("input_sources") => {
                let id=payload["id"].as_str().ok_or_else(||anyhow::anyhow!("Missing operation"))?;
                let content=payload["content"].clone();
                anyhow::ensure!(id.len()<=128 && content.to_string().len()<=128*1024,"Invalid captured input sources");
                self.runtime.record_input_sources(id,content).await;
                Ok(json!({"ok":true}))
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
                    if !settings.context {
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
    session: polar_transcreation::EditSession,
}
pub struct AdamCleanup {
    settings: Option<polar_station_settings::SettingsRepository>,
    state: tokio::sync::Mutex<AdamCleanupState>,
    owner: String,
    learning: Option<Arc<polar_transcreation::learning::LearningRuntime>>,
}
impl AdamCleanup {
    pub fn with_settings(mut self, settings: polar_station_settings::SettingsRepository) -> Self {
        self.settings = Some(settings); self
    }
    pub fn with_learning(
        mut self,
        learning: Arc<polar_transcreation::learning::LearningRuntime>,
    ) -> Self {
        self.learning = Some(learning);
        self
    }
    pub fn new(
        owner: String,
        credentials: Arc<dyn polar_inhouse_backend_adam::AdamOAuthCredentialResolver>,
    ) -> Result<Self> {
        let config = polar_inhouse_backend_adam::parse_adam_agent_config(
            "dictate",
            r#"{"llm_connection_mode":"openai","model":"gpt-6-luna","reasoning_effort":"low","max_tool_iterations":1}"#,
        )?;
        Ok(Self {
            owner,
            learning: None,
            settings: None,
            state: tokio::sync::Mutex::new(AdamCleanupState {
                runner: polar_inhouse_backend_adam::AdamAgentRunner::new(config)
                    .with_oauth_credential_resolver(credentials),
                session: polar_transcreation::EditSession::default(),
            }),
        })
    }
}
#[async_trait::async_trait]
impl TranscriptCleanup for AdamCleanup {
    async fn clean_observed(&self,recording_id:Option<&str>,request:polar_transcreation::EditRequest,observer:Box<dyn polar_transcreation::InhouseTurnEventHandler + Send>)->Result<String> {
        self.edit(request,recording_id,Some(observer)).await
    }

    async fn prepare_recording(&self, recording: &polar_dictate::Recording) -> Result<()> {
        let Ok(mut state) = self.state.try_lock() else {
            return Ok(());
        };
        let turn_ref = polar_inhouse_contract::InhouseBackendTurnRef {
            owner_id: self.owner.clone(),
            thread_id: format!("dictate:{}", recording.id),
            inhouse_polar_id: "dictate".into(),
            request_message_id: recording.id.clone(),
            backend_profile_id: "dictate-cleanup".into(),
        };
        polar_transcreation::prepare_execution(&mut state.runner, &turn_ref).await?;
        Ok(())
    }
    async fn clean(&self, request: polar_transcreation::EditRequest) -> Result<String> {
        self.edit(request, None,None).await
    }
    async fn clean_recording(
        &self,
        recording_id: &str,
        request: polar_transcreation::EditRequest,
    ) -> Result<String> {
        self.edit(request, Some(recording_id),None).await
    }
    async fn release_recording(&self, recording_id: &str) {
        let mut state = self.state.lock().await;
        state.session.release_scope(recording_id);
    }
}
impl AdamCleanup {
    async fn edit(
        &self,
        mut request: polar_transcreation::EditRequest,
        recording_id: Option<&str>,
        mut observer:Option<Box<dyn polar_transcreation::InhouseTurnEventHandler + Send>>,
    ) -> Result<String> {
        let started = std::time::Instant::now();
        let mut rules_revision = 0;
        let mut learned=json!({"status":"not_collected"});
        if let Some(learning) = &self.learning {
            if let Ok(state) = learning.state() {
                rules_revision = state.preferences.revision;
                let english = matches!(
                    request.edit,
                    polar_transcreation::EditKind::CleanupTranslateEnglish { .. }
                );
                let rules = state
                    .preferences
                    .reference(request.project.as_deref(), english);
                learned=json!({"status":"prepared","revision":rules_revision,"rules":rules,"included":false});
                if !rules.is_empty() {
                    if let polar_transcreation::EditKind::CleanupTranscript { context }
                    | polar_transcreation::EditKind::CleanupTranslateEnglish { context } =
                        &mut request.edit
                    {
                        let value = serde_json::to_string(
                            &json!({"nearby_context":context,"confirmed_personal_terminology":rules}),
                        )?;
                        if value.len() <= 16 * 1024 {
                            *context = Some(value);
                            learned["included"]=json!(true);
                        }
                    }
                }
            }
        }
        if let Some(observer)=observer.as_mut(){observer.on_input_snapshot(polar_transcreation::InhouseInputSnapshot{request_id:request.operation_id.clone(),channel:"learning".into(),status:"prepared".into(),content:learned}).await;}
        tracing::info!(target:"polar_dictate::latency",operation_id=request.operation_id,event="dictate.cleanup.context_ready",work_us=started.elapsed().as_micros() as u64);
        let operation_id = request.operation_id.clone();
        let lock_started = std::time::Instant::now();
        let mut state = self.state.lock().await;
        tracing::info!(target:"polar_dictate::latency",operation_id,event="dictate.cleanup.lock_acquired",work_us=started.elapsed().as_micros() as u64,wait_us=lock_started.elapsed().as_micros() as u64);
        let config = polar_transcreation::ExecutionConfig {
            prompts: self.settings.as_ref().map(|s| s.load_voice().map(|v| v.prompts)).transpose()?.unwrap_or_default(),
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
            polar_transcreation::edit_in_session_observed(runner, session, request, config,observer).await
        } else {
            let scope = if matches!(
                request.edit,
                polar_transcreation::EditKind::CleanupTranslateEnglish { .. }
            ) {
                "manual-english"
            } else {
                "manual-cleanup"
            };
            let scope = format!(
                "{}:{}:{}:{}:{}",
                self.owner,
                request.scope.as_deref().unwrap_or(&request.operation_id),
                request.project.as_deref().unwrap_or(""),
                scope,
                rules_revision
            );
            state.session.bind_scope(&scope);
            let AdamCleanupState { runner, session } = &mut *state;
            polar_transcreation::edit_in_session_observed(runner, session, request, config,observer).await
        };
        tracing::info!(target: "polar_dictate::latency", operation_id, event="dictate.cleanup",
            work_us=started.elapsed().as_micros() as u64, failed=result.is_err(), "Dictate processing stage");
        Ok(result?.text)
    }
}


