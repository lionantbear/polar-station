use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use polar_station_store_sqlite::{read_document, write_document, HostDatabase, HostDocument};

use polar_tmux_protocol::{
    ProtocolError, ResourceLogEventV1, ResourceLogLevelV1, ResourceLogTailV1, ResourcesCommandV1,
    ResourcesRequestEnvelopeV1, ResourcesResponseEnvelopeV1, ResourcesResultV1, PROTOCOL_VERSION,
    RESOURCE_LOG_MAX_BYTES, RESOURCE_LOG_MAX_EVENTS,
};
use polar_tmux_resources::{
    apply_catalog, project_resources_with_targets, ResourceCatalog, ResourceError,
    ResourceErrorCode, ResourceId, ResourceRuntimeState, ResourceRuntimeStatus, ResourceTargetKind,
    ResourcesControlProjection,
};
use tokio::sync::Mutex;

use crate::{
    host::{
        application_state::ApplicationStatePublisher,
        resource_provider::{platform_resource_runtime_root, SystemResourceProvider},
    },
    ssh_connection_manager::SshConnectionManager,
    target_registry::TargetRegistry,
};

trait ResourceCatalogStore: Send + Sync {
    fn load_or_initialize(&self) -> Result<ResourceCatalog, ResourceServiceError>;
    fn save(&self, catalog: &ResourceCatalog) -> Result<(), ResourceServiceError>;
}

#[derive(Debug)]
struct ResourceServiceError {
    code: &'static str,
    message: &'static str,
}

impl ResourceServiceError {
    fn persistence() -> Self {
        Self {
            code: "resource_persistence_failed",
            message: "The global Resource catalog could not be persisted safely.",
        }
    }
}

#[derive(Debug)]
struct SqliteResourceCatalogStore {
    database: HostDatabase,
}

impl ResourceCatalogStore for SqliteResourceCatalogStore {
    fn load_or_initialize(&self) -> Result<ResourceCatalog, ResourceServiceError> {
        self.database
            .runtime()
            .map_err(|_| ResourceServiceError::persistence())?
            .write_blocking(|connection| {
                Ok((|| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(|_| ResourceServiceError::persistence())?;
                    let catalog = match read_document(&tx, HostDocument::Resources)
                        .map_err(|_| ResourceServiceError::persistence())?
                    {
                        Some(bytes) => serde_json::from_slice(&bytes)
                            .map_err(|_| ResourceServiceError::persistence())?,
                        None => {
                            let catalog = ResourceCatalog::empty();
                            write_document(
                                &tx,
                                HostDocument::Resources,
                                &serde_json::to_vec(&catalog)
                                    .map_err(|_| ResourceServiceError::persistence())?,
                            )
                            .map_err(|_| ResourceServiceError::persistence())?;
                            catalog
                        }
                    };
                    tx.commit()
                        .map_err(|_| ResourceServiceError::persistence())?;
                    Ok(catalog)
                })())
            })
            .map_err(|_| ResourceServiceError::persistence())?
    }

    fn save(&self, catalog: &ResourceCatalog) -> Result<(), ResourceServiceError> {
        let bytes = serde_json::to_vec(catalog).map_err(|_| ResourceServiceError::persistence())?;
        self.database
            .runtime()
            .map_err(|_| ResourceServiceError::persistence())?
            .write_blocking(move |db| write_document(db, HostDocument::Resources, &bytes))
            .map_err(|_| ResourceServiceError::persistence())
    }
}





pub(crate) struct ResourceService {
    store: Arc<dyn ResourceCatalogStore>,
    targets: BTreeMap<String, ResourceTargetKind>,
    target_registry: Option<TargetRegistry>,
    catalog: Arc<Mutex<ResourceCatalog>>,
    statuses: Arc<Mutex<BTreeMap<ResourceId, ResourceRuntimeStatus>>>,
    logs: Arc<Mutex<BTreeMap<ResourceId, VecDeque<ResourceLogEventV1>>>>,
    recovery: Option<super::resource_recovery::ResourceRecovery>,
    state_events: ApplicationStatePublisher,
}

impl ResourceService {
    pub(crate) async fn open(
        database: HostDatabase,
        targets: BTreeMap<String, ResourceTargetKind>,
        target_registry: TargetRegistry,
        ssh_connections: SshConnectionManager,
        state_events: ApplicationStatePublisher,
    ) -> Result<Self, &'static str> {
        let mut service = Self::with_store(
            Arc::new(SqliteResourceCatalogStore { database }),
            targets,
            state_events,
        )
        .map_err(|_| "failed to load the global Resource catalog")?;
        service.target_registry = Some(target_registry.clone());
        let provider = SystemResourceProvider::new(
            target_registry,
            ssh_connections.clone(),
            platform_resource_runtime_root(),
        );
        let catalog = service.catalog.lock().await.clone();
        *service.statuses.lock().await = pending_statuses(&catalog, &service.targets);
        service.recovery = Some(super::resource_recovery::ResourceRecovery::new(
            provider,
            ssh_connections,
            catalog,
            service.statuses.clone(),
            service.logs.clone(),
            service.state_events.clone(),
        ));
        Ok(service)
    }

    fn with_store(
        store: Arc<dyn ResourceCatalogStore>,
        targets: BTreeMap<String, ResourceTargetKind>,
        state_events: ApplicationStatePublisher,
    ) -> Result<Self, ResourceServiceError> {
        let catalog = store.load_or_initialize()?;
        let statuses = unavailable_statuses(&catalog);
        Ok(Self {
            store,
            targets,
            target_registry: None,
            catalog: Arc::new(Mutex::new(catalog)),
            statuses: Arc::new(Mutex::new(statuses)),
            logs: Arc::new(Mutex::new(BTreeMap::new())),
            recovery: None,
            state_events,
        })
    }

    

    

    pub(crate) fn start_background_recovery(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.recovery.as_ref()?.start()
    }

    fn current_targets(&self) -> BTreeMap<String, ResourceTargetKind> {
        self.target_registry.as_ref().map_or_else(|| self.targets.clone(), |registry| registry.descriptors().into_iter().map(|t| {
            (t.id.as_str().into(), if t.kind == polar_tmux_protocol::ExecutionTargetKind::Ssh { ResourceTargetKind::Ssh } else { ResourceTargetKind::Local })
        }).collect())
    }

    pub(crate) async fn connection_references(&self, id: &str) -> Vec<String> {
        let catalog = self.catalog.lock().await;
        catalog.file_mappings.iter().map(|v| (&v.execution_target_id, &v.name))
            .chain(catalog.port_forwards.iter().map(|v| (&v.execution_target_id, &v.name)))
            .chain(catalog.android_mirrors.iter().map(|v| (&v.execution_target_id, &v.name)))
            .filter(|(target, _)| target.as_str() == id).map(|(_, name)| format!("Resource: {name}")).collect()
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(recovery) = &self.recovery {
            recovery.shutdown().await;
        }
    }

    pub(crate) async fn dispatch(
        &self,
        request: ResourcesRequestEnvelopeV1,
    ) -> ResourcesResponseEnvelopeV1 {
        if request.validate().is_err() {
            return self
                .error_response(
                    request.request_id,
                    "resource_request_invalid",
                    "The Resources request is invalid.",
                )
                .await;
        }
        match request.command.clone() {
            ResourcesCommandV1::GetProjection => {
                let catalog = self.catalog.lock().await.clone();
                ResourcesResponseEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: request.api,
                    request_id: request.request_id,
                    revision: catalog.revision,
                    result: ResourcesResultV1::Projection {
                        projection: self.project(&catalog).await,
                    },
                }
            }
            ResourcesCommandV1::GetLogTail { resource_id } => {
                let catalog = self.catalog.lock().await;
                let resource_id = match resource_id.parse::<ResourceId>() {
                    Ok(resource_id) if find_resource(&catalog, resource_id) => resource_id,
                    _ => {
                        return self
                            .domain_error_response(
                                request.request_id,
                                &catalog,
                                ResourceError::new(
                                    ResourceErrorCode::ResourceNotFound,
                                    "The selected Resource no longer exists.",
                                ),
                            )
                            .await;
                    }
                };
                let events = self
                    .logs
                    .lock()
                    .await
                    .get(&resource_id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                ResourcesResponseEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: request.api,
                    request_id: request.request_id,
                    revision: catalog.revision,
                    result: ResourcesResultV1::LogTail {
                        tail: ResourceLogTailV1 {
                            resource_id: resource_id.to_string(),
                            events,
                            truncated: false,
                        },
                    },
                }
            }
            ResourcesCommandV1::Apply { catalog: candidate } => {
                let mut catalog = self.catalog.lock().await;
                let expected = request.expected_revision.expect("validated mutation");
                match apply_catalog(&catalog, expected, candidate, &self.current_targets()) {
                    Ok(next) => {
                        if let Err(error) = self.store.save(&next) {
                            return self
                                .service_error_response(
                                    request.request_id,
                                    catalog.revision,
                                    error,
                                    Some(&catalog),
                                )
                                .await;
                        }
                        *catalog = next;
                        *self.statuses.lock().await = pending_statuses(&catalog, &self.current_targets());
                        if let Some(recovery) = &self.recovery {
                            recovery.apply(catalog.clone());
                        }
                        let projection = self.project(&catalog).await;
                        ResourcesResponseEnvelopeV1 {
                            protocol_version: PROTOCOL_VERSION,
                            api: request.api,
                            request_id: request.request_id,
                            revision: catalog.revision,
                            result: ResourcesResultV1::CommandAccepted { projection },
                        }
                    }
                    Err(error) => {
                        self.domain_error_response(request.request_id, &catalog, error)
                            .await
                    }
                }
            }
            ResourcesCommandV1::Restart { resource_id } => {
                self.runtime_command(request, resource_id, RuntimeResourceCommand::Restart)
                    .await
            }
            ResourcesCommandV1::OpenMirror { resource_id } => {
                self.runtime_command(request, resource_id, RuntimeResourceCommand::OpenMirror)
                    .await
            }
        }
    }

    pub(crate) async fn current_revision(&self) -> u64 {
        self.catalog.lock().await.revision
    }

    async fn runtime_command(
        &self,
        request: ResourcesRequestEnvelopeV1,
        resource_id: String,
        command: RuntimeResourceCommand,
    ) -> ResourcesResponseEnvelopeV1 {
        let catalog = self.catalog.lock().await;
        if request.expected_revision != Some(catalog.revision) {
            return self
                .domain_error_response(
                    request.request_id,
                    &catalog,
                    ResourceError::new(
                        ResourceErrorCode::CatalogStale,
                        "The Resource catalog changed. Refresh and try again.",
                    ),
                )
                .await;
        }
        let resource_id = match resource_id.parse::<ResourceId>() {
            Ok(resource_id) => resource_id,
            Err(error) => {
                return self
                    .domain_error_response(request.request_id, &catalog, error)
                    .await;
            }
        };
        let is_file = catalog
            .file_mappings
            .iter()
            .any(|entry| entry.id == resource_id);
        let is_forward = catalog
            .port_forwards
            .iter()
            .any(|entry| entry.id == resource_id);
        let android = catalog
            .android_mirrors
            .iter()
            .find(|entry| entry.id == resource_id);
        if !is_file && !is_forward && android.is_none() {
            return self
                .domain_error_response(
                    request.request_id,
                    &catalog,
                    ResourceError::new(
                        ResourceErrorCode::ResourceNotFound,
                        "The selected Resource no longer exists.",
                    ),
                )
                .await;
        }
        if command == RuntimeResourceCommand::OpenMirror
            && !android.is_some_and(|entry| entry.enabled)
        {
            return self
                .domain_error_response(
                    request.request_id,
                    &catalog,
                    ResourceError::new(
                        ResourceErrorCode::AndroidNotEnabled,
                        "Open Mirror requires an enabled Android Resource.",
                    ),
                )
                .await;
        }
        let Some(recovery) = &self.recovery else {
            return ResourcesResponseEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: request.api,
                request_id: request.request_id,
                revision: catalog.revision,
                result: ResourcesResultV1::Error {
                    error: ProtocolError::new(
                        "resource_provider_unavailable",
                        "This Resource provider is not available in the current Host build.",
                        false,
                    ),
                    projection: Some(self.project(&catalog).await),
                },
            };
        };
        recovery.command(resource_id, command == RuntimeResourceCommand::OpenMirror);
        ResourcesResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: request.api,
            request_id: request.request_id,
            revision: catalog.revision,
            result: ResourcesResultV1::CommandAccepted {
                projection: self.project(&catalog).await,
            },
        }
    }

    async fn error_response(
        &self,
        request_id: String,
        code: &'static str,
        message: &'static str,
    ) -> ResourcesResponseEnvelopeV1 {
        let catalog = self.catalog.lock().await;
        ResourcesResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: polar_tmux_protocol::ResourcesApiV1::ResourcesV1,
            request_id,
            revision: catalog.revision,
            result: ResourcesResultV1::Error {
                error: ProtocolError::new(code, message, false),
                projection: Some(self.project(&catalog).await),
            },
        }
    }

    async fn domain_error_response(
        &self,
        request_id: String,
        catalog: &ResourceCatalog,
        error: ResourceError,
    ) -> ResourcesResponseEnvelopeV1 {
        ResourcesResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: polar_tmux_protocol::ResourcesApiV1::ResourcesV1,
            request_id,
            revision: catalog.revision,
            result: ResourcesResultV1::Error {
                error: ProtocolError::new(error.code.as_str(), error.message, false),
                projection: Some(self.project(catalog).await),
            },
        }
    }

    async fn service_error_response(
        &self,
        request_id: String,
        revision: u64,
        error: ResourceServiceError,
        catalog: Option<&ResourceCatalog>,
    ) -> ResourcesResponseEnvelopeV1 {
        ResourcesResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: polar_tmux_protocol::ResourcesApiV1::ResourcesV1,
            request_id,
            revision,
            result: ResourcesResultV1::Error {
                error: ProtocolError::new(error.code, error.message, false),
                projection: match catalog {
                    Some(catalog) => Some(self.project(catalog).await),
                    None => None,
                },
            },
        }
    }

    async fn project(&self, catalog: &ResourceCatalog) -> ResourcesControlProjection {
        project_resources_with_targets(
            catalog,
            &*self.statuses.lock().await,
            self.current_targets()
                .iter()
                .filter(|(_, kind)| **kind == ResourceTargetKind::Ssh)
                .map(|(id, _)| id.clone())
                .collect(),
        )
    }
}

pub(super) async fn append_resource_log(
    logs: &Mutex<BTreeMap<ResourceId, VecDeque<ResourceLogEventV1>>>,
    resource_id: ResourceId,
    level: ResourceLogLevelV1,
    message: String,
) {
    let message = sanitize_log_message(&message);
    let event = ResourceLogEventV1 {
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64),
        level,
        message,
    };
    let mut logs = logs.lock().await;
    let events = logs.entry(resource_id).or_default();
    if events
        .back()
        .is_some_and(|previous| previous.level == event.level && previous.message == event.message)
    {
        return;
    }
    events.push_back(event);
    while events.len() > RESOURCE_LOG_MAX_EVENTS
        || events
            .iter()
            .map(|event| event.message.len())
            .sum::<usize>()
            > RESOURCE_LOG_MAX_BYTES
    {
        events.pop_front();
    }
}

fn find_resource(catalog: &ResourceCatalog, id: ResourceId) -> bool {
    catalog.file_mappings.iter().any(|entry| entry.id == id)
        || catalog.port_forwards.iter().any(|entry| entry.id == id)
        || catalog.android_mirrors.iter().any(|entry| entry.id == id)
}

fn sanitize_log_message(message: &str) -> String {
    let sanitized = message
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect::<String>();
    if sanitized.is_empty() {
        "Resource state changed.".to_owned()
    } else {
        sanitized
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeResourceCommand {
    Restart,
    OpenMirror,
}

fn unavailable_statuses(catalog: &ResourceCatalog) -> BTreeMap<ResourceId, ResourceRuntimeStatus> {
    let mut statuses = BTreeMap::new();
    for (id, enabled) in catalog
        .file_mappings
        .iter()
        .map(|entry| (entry.id, entry.enabled))
        .chain(
            catalog
                .port_forwards
                .iter()
                .map(|entry| (entry.id, entry.enabled)),
        )
        .chain(
            catalog
                .android_mirrors
                .iter()
                .map(|entry| (entry.id, entry.enabled)),
        )
    {
        statuses.insert(
            id,
            if enabled {
                ResourceRuntimeStatus {
                    state: ResourceRuntimeState::NeedsAttention,
                    generation: 0,
                    message: "Provider is not available in this Host build.".to_owned(),
                    can_restart: false,
                    can_open_mirror: false,
                }
            } else {
                ResourceRuntimeStatus::disabled()
            },
        );
    }
    statuses
}

fn pending_statuses(
    catalog: &ResourceCatalog,
    targets: &BTreeMap<String, ResourceTargetKind>,
) -> BTreeMap<ResourceId, ResourceRuntimeStatus> {
    let mut statuses = BTreeMap::new();
    for (id, enabled, target_id) in catalog
        .file_mappings
        .iter()
        .map(|entry| (entry.id, entry.enabled, entry.execution_target_id.as_str()))
        .chain(
            catalog
                .port_forwards
                .iter()
                .map(|entry| (entry.id, entry.enabled, entry.execution_target_id.as_str())),
        )
        .chain(
            catalog
                .android_mirrors
                .iter()
                .map(|entry| (entry.id, entry.enabled, entry.execution_target_id.as_str())),
        )
    {
        statuses.insert(
            id,
            if !enabled {
                ResourceRuntimeStatus::disabled()
            } else if targets.get(target_id) == Some(&ResourceTargetKind::Ssh) {
                ResourceRuntimeStatus {
                    state: ResourceRuntimeState::WaitingForSsh,
                    generation: 0,
                    message: "Waiting for the shared SSH connection.".to_owned(),
                    can_restart: false,
                    can_open_mirror: false,
                }
            } else {
                ResourceRuntimeStatus {
                    state: ResourceRuntimeState::Starting,
                    generation: 0,
                    message: "Starting in the background.".to_owned(),
                    can_restart: false,
                    can_open_mirror: false,
                }
            },
        );
    }
    statuses
}
