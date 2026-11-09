//! Optional in-process local-IPC extension handlers.
//!
//! This boundary routes bounded requests only. A composition must explicitly
//! inject each implementation; the default standalone Host advertises none.
//! Target identity, actor authorization, SDK allowlists and storage remain the
//! injected domain adapter's responsibility. No arbitrary SDK method proxy is
//! installed here. A negotiation is NOT a Polar authorization grant.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use polar_local_access_contract::{
    negotiate_local_extensions, LocalExtensionCancelV1, LocalExtensionOfferV1,
    LocalExtensionRequestV1, LocalExtensionResponseV1, LocalExtensionResultV1, ProtocolError,
    MAX_EXTENSION_IN_FLIGHT, MAX_LOCAL_EXTENSIONS,
};
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet};

const MAX_HOST_EXTENSION_IN_FLIGHT: usize = 64;

/// Revoked when the owning IPC connection is torn down. Domain leases keep a
/// Weak reference so no connection credential is kept alive by its leases.
#[derive(Debug)]
pub struct LocalExtensionLifetime {
    live: AtomicBool,
    closed: tokio::sync::Notify,
}

impl Default for LocalExtensionLifetime {
    fn default() -> Self {
        Self {
            live: AtomicBool::new(true),
            closed: tokio::sync::Notify::new(),
        }
    }
}

impl LocalExtensionLifetime {
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.live.store(false, Ordering::Release);
        self.closed.notify_waiters();
    }
    /// Register before checking the atomic so socket teardown cannot be missed.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.closed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_live() {
                return;
            }
            notified.await;
        }
    }
}

/// Identifies the authenticated local frontend registration, not a Polar actor.
#[derive(Debug, Clone)]
pub struct LocalExtensionContext {
    pub frontend_id: String,
    pub registration_id: String,
    pub request_id: String,
    pub deadline: tokio::time::Instant,
    /// Obtained from the server's UnixStream peer credential after verify_peer.
    /// None must never be interpreted as an authenticated local OS user.
    pub peer_uid: Option<u32>,
    pub lifetime: Arc<LocalExtensionLifetime>,
    pub events: Option<LocalExtensionEvents>,
}

/// Bounded event lane owned by the authenticated socket. No sender is allowed
/// to outlive its connection or to change the negotiated extension identity.
#[derive(Debug, Clone)]
pub struct LocalExtensionEvents {
    sender: tokio::sync::mpsc::Sender<polar_local_access_contract::LocalExtensionEventV1>,
    lifetime: std::sync::Weak<LocalExtensionLifetime>,
    grant: LocalExtensionOfferV1,
}
impl LocalExtensionEvents {
    pub async fn emit(
        &self,
        subscription_id: &str,
        sequence: u64,
        payload: Value,
    ) -> std::result::Result<(), ProtocolError> {
        self.emit_with_policy(subscription_id, sequence, payload, true)
            .await
    }
    /// External target failure/backpressure must not tear down other targets.
    pub async fn emit_isolated(
        &self,
        subscription_id: &str,
        sequence: u64,
        payload: Value,
    ) -> std::result::Result<(), ProtocolError> {
        self.emit_with_policy(subscription_id, sequence, payload, false)
            .await
    }
    async fn emit_with_policy(
        &self,
        subscription_id: &str,
        sequence: u64,
        payload: Value,
        close_connection: bool,
    ) -> std::result::Result<(), ProtocolError> {
        let lifetime = self
            .lifetime
            .upgrade()
            .filter(|l| l.is_live())
            .ok_or_else(|| {
                ProtocolError::new("stream_closed", "The connection is closed.", false)
            })?;
        let event = polar_local_access_contract::LocalExtensionEventV1 {
            extension: self.grant.name.clone(),
            version: self.grant.version,
            subscription_id: subscription_id.to_owned(),
            sequence,
            payload,
        };
        event
            .validate_with_limit(self.grant.max_payload_bytes as usize)
            .map_err(|_| {
                ProtocolError::new(
                    "stream_limit",
                    "The event exceeds the negotiated limit.",
                    false,
                )
            })?;
        tokio::select! {
            biased;
            _ = lifetime.cancelled() => Err(ProtocolError::new("stream_closed", "The connection is closed.", false)),
            result = tokio::time::timeout(Duration::from_secs(2), self.sender.send(
                event)) => {
                if matches!(result, Ok(Ok(()))) { Ok(()) } else {
                    // A record must not be partially delivered then silently skipped.
                    // Tear down authority; the frontend will reconnect and resnapshot.
                    if close_connection { lifetime.close(); }
                    Err(ProtocolError::new("stream_backpressure", "Reconnect and reload the Thread snapshot.", false))
                }
            }
        }
    }
}

pub type LocalExtensionFuture<'a> =
    Pin<Box<dyn Future<Output = std::result::Result<Value, ProtocolError>> + Send + 'a>>;

pub trait LocalExtensionHandler: Send + Sync {
    fn offer(&self) -> LocalExtensionOfferV1;
    /// Implementations must validate the domain request, bound any blocking
    /// work, check target/actor leases and use the existing business ingress.
    /// Dropping this future cancels observation; it need not undo a mutation.
    fn request<'a>(
        &'a self,
        context: LocalExtensionContext,
        payload: Value,
    ) -> LocalExtensionFuture<'a>;
}

struct RegisteredExtension {
    offer: LocalExtensionOfferV1,
    handler: Arc<dyn LocalExtensionHandler>,
}

pub struct LocalExtensionRegistry {
    handlers: BTreeMap<String, RegisteredExtension>,
    request_slots: Arc<Semaphore>,
}

impl Default for LocalExtensionRegistry {
    fn default() -> Self {
        Self {
            handlers: BTreeMap::new(),
            request_slots: Arc::new(Semaphore::new(MAX_HOST_EXTENSION_IN_FLIGHT)),
        }
    }
}

impl LocalExtensionRegistry {
    pub fn take_handler(&mut self, name: &str) -> Option<Arc<dyn LocalExtensionHandler>> {
        self.handlers.remove(name).map(|entry| entry.handler)
    }

    pub fn register(&mut self, handler: Arc<dyn LocalExtensionHandler>) -> Result<()> {
        let offer = handler.offer();
        offer.validate().context("invalid local extension offer")?;
        if self.handlers.contains_key(&offer.name) {
            bail!("local extension is already registered");
        }
        if self.handlers.len() >= MAX_LOCAL_EXTENSIONS {
            bail!("local extension capacity reached");
        }
        self.handlers
            .insert(offer.name.clone(), RegisteredExtension { offer, handler });
        Ok(())
    }

    pub fn offers(&self) -> Vec<LocalExtensionOfferV1> {
        self.handlers
            .values()
            .map(|entry| entry.offer.clone())
            .collect()
    }

    pub fn negotiate(
        &self,
        offered: &[LocalExtensionOfferV1],
    ) -> Result<Vec<LocalExtensionOfferV1>> {
        Ok(negotiate_local_extensions(offered, &self.offers())?)
    }
}

type RequestKey = (String, u16, String);
fn key(name: &str, version: u16, request_id: &str) -> RequestKey {
    (name.to_owned(), version, request_id.to_owned())
}

/// One connection's bounded tasks. Drop aborts every owned future. No spawned
/// task survives as an untracked background request after socket teardown.
pub struct LocalExtensionSession {
    registry: Arc<LocalExtensionRegistry>,
    grants: Vec<LocalExtensionOfferV1>,
    frontend_id: String,
    registration_id: String,
    peer_uid: Option<u32>,
    lifetime: Arc<LocalExtensionLifetime>,
    event_sender:
        Option<tokio::sync::mpsc::Sender<polar_local_access_contract::LocalExtensionEventV1>>,
    sequence: u64,
    pending: BTreeMap<RequestKey, (u64, AbortHandle)>,
    tasks: JoinSet<(RequestKey, u64, LocalExtensionResponseV1)>,
}

impl LocalExtensionSession {
    pub fn new(
        registry: Arc<LocalExtensionRegistry>,
        grants: Vec<LocalExtensionOfferV1>,
        frontend_id: String,
        registration_id: String,
        peer_uid: Option<u32>,
    ) -> Self {
        Self {
            registry,
            grants,
            frontend_id,
            registration_id,
            peer_uid,
            lifetime: Arc::new(LocalExtensionLifetime::default()),
            event_sender: None,
            sequence: 0,
            pending: BTreeMap::new(),
            tasks: JoinSet::new(),
        }
    }

    pub fn with_event_sender(
        mut self,
        sender: tokio::sync::mpsc::Sender<polar_local_access_contract::LocalExtensionEventV1>,
    ) -> Self {
        self.event_sender = Some(sender);
        self
    }
    pub fn lifetime(&self) -> Arc<LocalExtensionLifetime> {
        self.lifetime.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Immediate rejection is returned to the existing bounded socket writer.
    /// Duplicate in-flight IDs are a protocol violation: do not send two
    /// ambiguous responses with the same correlation key or execute twice.
    pub fn submit(
        &mut self,
        request: LocalExtensionRequestV1,
    ) -> Result<Option<LocalExtensionResponseV1>> {
        request.validate()?;
        let Some(grant) = self
            .grants
            .iter()
            .find(|g| g.permits(&request.extension, request.version))
        else {
            return Ok(Some(LocalExtensionResponseV1::for_request(
                &request,
                LocalExtensionResultV1::Unsupported {},
            )));
        };
        request.validate_with_limit(grant.max_payload_bytes as usize)?;
        let Some(entry) = self.registry.handlers.get(&request.extension) else {
            return Ok(Some(LocalExtensionResponseV1::for_request(
                &request,
                LocalExtensionResultV1::Unsupported {},
            )));
        };
        let request_key = key(&request.extension, request.version, &request.request_id);
        if self.pending.contains_key(&request_key) {
            bail!("duplicate in-flight extension request ID");
        }
        // Include cancelled-but-not-yet-reaped tasks in the bound.
        if self.tasks.len() >= MAX_EXTENSION_IN_FLIGHT {
            return Ok(Some(rejected(
                &request,
                "extension_busy",
                "The extension request queue is full.",
                true,
            )));
        }
        let Ok(permit) = self.registry.request_slots.clone().try_acquire_owned() else {
            return Ok(Some(rejected(
                &request,
                "extension_busy",
                "The Host extension request queue is full.",
                true,
            )));
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .context("extension request sequence exhausted")?;
        let sequence = self.sequence;
        let limit = grant.max_payload_bytes as usize;
        let handler = entry.handler.clone();
        let timeout = Duration::from_millis(u64::from(request.timeout_ms));
        let context = LocalExtensionContext {
            frontend_id: self.frontend_id.clone(),
            registration_id: self.registration_id.clone(),
            request_id: request.request_id.clone(),
            deadline: tokio::time::Instant::now() + timeout,
            peer_uid: self.peer_uid,
            lifetime: self.lifetime.clone(),
            events: self
                .event_sender
                .clone()
                .map(|sender| LocalExtensionEvents {
                    sender,
                    lifetime: Arc::downgrade(&self.lifetime),
                    grant: grant.clone(),
                }),
        };
        let completion_key = request_key.clone();
        let handle = self.tasks.spawn(async move {
            let _permit = permit;
            let mut response = LocalExtensionResponseV1::for_request(
                &request,
                LocalExtensionResultV1::Unsupported {},
            );
            let result =
                tokio::time::timeout(timeout, handler.request(context, request.payload)).await;
            response.result = match result {
                Ok(Ok(payload)) => LocalExtensionResultV1::Success { payload },
                Ok(Err(error)) if error.validate_sanitized().is_ok() => {
                    LocalExtensionResultV1::Rejected { error }
                }
                Ok(Err(_)) => LocalExtensionResultV1::Rejected {
                    error: ProtocolError::new(
                        "extension_failed",
                        "The extension request failed.",
                        false,
                    ),
                },
                Err(_) => LocalExtensionResultV1::Rejected {
                    error: ProtocolError::new(
                        "extension_timeout",
                        "The operation may have completed. Check its result before retrying.",
                        false,
                    ),
                },
            };
            if response.validate_with_limit(limit).is_err() {
                response.result = LocalExtensionResultV1::Rejected {
                    error: ProtocolError::new(
                        "extension_response_limit",
                        "The extension response exceeded the negotiated limit.",
                        false,
                    ),
                };
            }
            (completion_key, sequence, response)
        });
        self.pending.insert(request_key, (sequence, handle));
        Ok(None)
    }

    pub fn cancel(&mut self, request: LocalExtensionCancelV1) -> Result<LocalExtensionResponseV1> {
        request.validate()?;
        let permitted = self
            .grants
            .iter()
            .any(|g| g.permits(&request.extension, request.version));
        let result = if !permitted {
            LocalExtensionResultV1::Unsupported {}
        } else if let Some((_, handle)) = self.pending.remove(&key(
            &request.extension,
            request.version,
            &request.request_id,
        )) {
            handle.abort();
            LocalExtensionResultV1::Cancelled {}
        } else {
            LocalExtensionResultV1::Rejected {
                error: ProtocolError::new(
                    "extension_not_pending",
                    "The request is no longer pending. Check its result.",
                    false,
                ),
            }
        };
        Ok(LocalExtensionResponseV1 {
            extension: request.extension,
            version: request.version,
            request_id: request.request_id,
            result,
        })
    }

    pub async fn next_response(&mut self) -> Result<Option<LocalExtensionResponseV1>> {
        while let Some(completion) = self.tasks.join_next().await {
            match completion {
                Ok((key, sequence, response)) => {
                    // Cancel/reuse must not publish an old completion for a new request.
                    if self.pending.get(&key).is_some_and(|(s, _)| *s == sequence) {
                        self.pending.remove(&key);
                        return Ok(Some(response));
                    }
                }
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(error).context("local extension task failed"),
            }
        }
        Ok(None)
    }
}

impl Drop for LocalExtensionSession {
    fn drop(&mut self) {
        self.lifetime.close();
        self.tasks.abort_all();
    }
}

fn rejected(
    request: &LocalExtensionRequestV1,
    code: &str,
    message: &str,
    retryable: bool,
) -> LocalExtensionResponseV1 {
    LocalExtensionResponseV1::for_request(
        request,
        LocalExtensionResultV1::Rejected {
            error: ProtocolError::new(code, message, retryable),
        },
    )
}


