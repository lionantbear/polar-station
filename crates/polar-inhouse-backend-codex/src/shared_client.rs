//! Profile-owned app-server with a single event reader and independent threads.

use crate::client::{AppServerClient, AppServerEvent, AppServerRequestHandle};
use crate::codex_app_server_compat::{
    ClientRequest, JSONRPCErrorError, RequestId, TypedRequestError,
};
use crate::thread_lifecycle::{ThreadLifecycleIo, ThreadSetups};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

const EVENT_CAPACITY: usize = 2048;

#[derive(Default)]
struct Routes(Mutex<HashMap<String, mpsc::Sender<AppServerEvent>>>);

impl Routes {
    fn subscribe(&self, thread: &str) -> anyhow::Result<mpsc::Receiver<AppServerEvent>> {
        let mut routes = self.0.lock().expect("Codex routes poisoned");
        anyhow::ensure!(
            routes.get(thread).is_none_or(mpsc::Sender::is_closed),
            "Codex thread already has an active consumer"
        );
        let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
        routes.insert(thread.to_string(), tx);
        Ok(rx)
    }

    fn deliver(&self, event: AppServerEvent) -> bool {
        let Some(thread) = event_thread(&event) else {
            return false;
        };
        let mut routes = self.0.lock().expect("Codex routes poisoned");
        let sent = routes
            .get(&thread)
            .is_some_and(|tx| tx.try_send(event).is_ok());
        if !sent {
            // Close only this subscriber. It observes a closed stream instead
            // of silently accepting an incomplete reply. Other threads proceed.
            routes.remove(&thread);
        }
        sent
    }

    fn close(&self) {
        self.0.lock().expect("Codex routes poisoned").clear();
    }
}

fn event_thread(event: &AppServerEvent) -> Option<String> {
    let value = match event {
        AppServerEvent::ServerNotification(notification) => serde_json::to_value(notification),
        AppServerEvent::ServerRequest(request) => serde_json::to_value(request),
        _ => return None,
    }
    .ok()?;
    value
        .get("params")?
        .get("threadId")?
        .as_str()
        .map(str::to_owned)
}

/// One native authentication manager and event reader for a backend profile.
pub(crate) struct SharedCodexClient {
    requests: AppServerRequestHandle,
    stop: oneshot::Sender<()>,
    routes: Arc<Routes>,
    setups: Arc<ThreadSetups>,
}

impl SharedCodexClient {
    pub(crate) fn start(client: AppServerClient) -> Arc<Self> {
        Self::start_guarded(client, None)
    }

    pub(crate) fn start_guarded(
        mut client: AppServerClient,
        guard: Option<(std::fs::File, Option<crate::client::PrivateServer>)>,
    ) -> Arc<Self> {
        let requests = client.request_handle();
        let (stop, mut ended) = oneshot::channel();
        let routes = Arc::new(Routes::default());
        let worker_routes = Arc::clone(&routes);
        let setups = Arc::new(ThreadSetups::default());
        let worker_setups = Arc::clone(&setups);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut ended => break,
                    event = client.next_event() => match event {
                        Some(AppServerEvent::Lagged { .. } | AppServerEvent::Disconnected { .. }) => {
                            worker_routes.close();
                            break;
                        }
                        Some(AppServerEvent::ThreadStreamDisconnected(_) | AppServerEvent::ThreadStreamReconnected(_)) => continue,
                        Some(event @ (AppServerEvent::ServerRequest(_) | AppServerEvent::ServerNotification(_))) => {
                            if let AppServerEvent::ServerNotification(notification) = &event {
                                if let Ok(value) = serde_json::to_value(notification) {
                                    worker_setups.observe(&value);
                                }
                            }
                            let pending_id = match &event {
                                AppServerEvent::ServerRequest(request) => Some(request.id().clone()),
                                _ => None,
                            };
                            if !worker_routes.deliver(event) {
                                if let Some(id) = pending_id {
                                    let _ = client.reject_server_request(id, JSONRPCErrorError {
                                        code: -32603,
                                        message: "No active Polar thread consumer for this request".to_string(),
                                        data: None,
                                    }).await;
                                }
                            }
                        }
                        None => break,
                    }
                }
            }
            worker_routes.close();
            worker_setups.close();
            let _ = client.shutdown().await;
            if let Some((lock, child)) = guard {
                if let Some(child) = child {
                    child.shutdown().await;
                }
                drop(lock);
            }
        });
        Arc::new(Self {
            requests,
            stop,
            routes,
            setups,
        })
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.stop.is_closed()
    }

    pub(crate) fn lease(self: &Arc<Self>) -> ThreadClient {
        ThreadClient {
            shared: Arc::clone(self),
            events: None,
            thread: None,
        }
    }

    pub(crate) async fn request_typed<T: DeserializeOwned>(
        &self,
        request: ClientRequest,
    ) -> Result<T, TypedRequestError> {
        self.requests.request_typed(request).await
    }
}

/// A runner owns its thread queue; it does not own the profile's OAuth runtime.
pub(crate) struct ThreadClient {
    shared: Arc<SharedCodexClient>,
    events: Option<mpsc::Receiver<AppServerEvent>>,
    thread: Option<String>,
}

impl Drop for ThreadClient {
    fn drop(&mut self) {
        self.finish_turn();
    }
}

impl ThreadClient {
    pub(crate) fn turn_lease(&self) -> anyhow::Result<ThreadLease> {
        let thread = self
            .thread
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Codex thread is not bound"))?;
        let sender = self
            .shared
            .routes
            .0
            .lock()
            .expect("Codex routes poisoned")
            .get(thread)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Codex thread route closed"))?;
        Ok(ThreadLease {
            routes: Arc::clone(&self.shared.routes),
            thread: thread.clone(),
            sender,
        })
    }

    pub(crate) fn finish_turn(&mut self) {
        self.events = None;
        if let Some(thread) = self.thread.take() {
            let mut routes = self.shared.routes.0.lock().expect("Codex routes poisoned");
            if routes.get(&thread).is_some_and(mpsc::Sender::is_closed) {
                routes.remove(&thread);
            }
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub(crate) fn bind_thread(&mut self, thread: &str) -> anyhow::Result<()> {
        if self.thread.as_deref() == Some(thread)
            && self
                .events
                .as_ref()
                .is_some_and(|events| !events.is_closed())
        {
            return Ok(());
        }
        // Release a previous turn subscription before renewing the same thread.
        self.finish_turn();
        self.events = Some(self.shared.routes.subscribe(thread)?);
        self.thread = Some(thread.to_string());
        Ok(())
    }

    pub(crate) async fn request_typed<T: DeserializeOwned>(
        &self,
        request: ClientRequest,
    ) -> Result<T, TypedRequestError> {
        self.shared.request_typed(request).await
    }

    pub(crate) async fn next_event(&mut self) -> Option<AppServerEvent> {
        self.events.as_mut()?.recv().await
    }

    pub(crate) async fn resolve_server_request(
        &self,
        id: RequestId,
        value: Value,
    ) -> io::Result<()> {
        self.shared.requests.resolve_server_request(id, value).await
    }

    pub(crate) async fn reject_server_request(
        &self,
        id: RequestId,
        error: JSONRPCErrorError,
    ) -> io::Result<()> {
        self.shared.requests.reject_server_request(id, error).await
    }
}

/// Removes only this lease's route, including when its owning future is cancelled.
pub(crate) struct ThreadLease {
    routes: Arc<Routes>,
    thread: String,
    sender: mpsc::Sender<AppServerEvent>,
}

impl Drop for ThreadLease {
    fn drop(&mut self) {
        let mut routes = self.routes.0.lock().expect("Codex routes poisoned");
        if routes
            .get(&self.thread)
            .is_some_and(|current| current.same_channel(&self.sender))
        {
            routes.remove(&self.thread);
        }
    }
}

#[async_trait::async_trait]
impl ThreadLifecycleIo for ThreadClient {
    fn setups(&self) -> Arc<ThreadSetups> {
        Arc::clone(&self.shared.setups)
    }
    async fn lifecycle_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        Ok(self
            .request_typed(crate::codex_app_server_compat::lifecycle_request(
                method, params,
            )?)
            .await?)
    }
}


