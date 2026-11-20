//! Native thread preparation shared by embedded and Unix socket app-server transports.
//! These receipts remember applied request values, never replace native status.

use anyhow::Context;
use polar_inhouse_contract::InhouseTurnOptions;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) enum LifecycleFailure {
    Database,
    Prepare,
    Busy,
    NativeState,
    Instructions,
    Dispatch,
}
impl std::fmt::Display for LifecycleFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Database => "Codex state database initialization failed",
            Self::Prepare => "Codex thread preparation failed; no message was sent",
            Self::Busy => "Codex thread is busy; no message was sent",
            Self::NativeState => "Codex thread is not ready for direct input",
            Self::Instructions => "Codex instruction application was not confirmed",
            Self::Dispatch => {
                "Codex turn failed; the invocation will not be replayed automatically"
            }
        })
    }
}
impl std::error::Error for LifecycleFailure {}

#[derive(Clone, PartialEq)]
pub(crate) struct ThreadSetup(Value);

impl ThreadSetup {
    pub(crate) fn from_params(params: &Value) -> Self {
        // Model/effort are already supported by turn/start. Input and turn
        // augmentations must never cause an otherwise idle thread to reload.
        let mut fields = serde_json::Map::new();
        for key in [
            "baseInstructions",
            "developerInstructions",
            "config",
            "modelProvider",
            "cwd",
            "approvalPolicy",
            "sandbox",
        ] {
            if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                fields.insert(key.to_owned(), value.clone());
            }
        }
        Self(Value::Object(fields))
    }
}

#[derive(Default)]
struct Entry {
    applied: Option<ThreadSetup>,
    unload_revision: u64,
}

pub(crate) struct ThreadSetups {
    entries: Mutex<HashMap<String, Entry>>,
    closed: watch::Sender<bool>,
}

impl Default for ThreadSetups {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            closed: watch::channel(false).0,
        }
    }
}

impl ThreadSetups {
    pub(crate) fn remember(&self, thread: &str, setup: ThreadSetup) {
        self.entries
            .lock()
            .expect("thread setups poisoned")
            .entry(thread.to_owned())
            .or_default()
            .applied = Some(setup);
    }

    fn invalidate(&self, thread: &str) {
        self.entries
            .lock()
            .expect("thread setups poisoned")
            .entry(thread.to_owned())
            .or_default()
            .applied = None;
    }

    fn matches(&self, thread: &str, setup: &ThreadSetup) -> bool {
        self.entries
            .lock()
            .expect("thread setups poisoned")
            .get(thread)
            .is_some_and(|entry| entry.applied.as_ref() == Some(setup))
    }

    pub(crate) fn revision(&self, thread: &str) -> u64 {
        self.entries
            .lock()
            .expect("thread setups poisoned")
            .entry(thread.to_owned())
            .or_default()
            .unload_revision
    }

    pub(crate) fn observe(&self, message: &Value) {
        let method = message.get("method").and_then(Value::as_str);
        let unloaded = method == Some("thread/closed")
            || (method == Some("thread/status/changed")
                && message
                    .pointer("/params/status/type")
                    .and_then(Value::as_str)
                    == Some("notLoaded"));
        if !unloaded {
            return;
        }
        let Some(thread) = message.pointer("/params/threadId").and_then(Value::as_str) else {
            return;
        };
        if let Some(entry) = self
            .entries
            .lock()
            .expect("thread setups poisoned")
            .get_mut(thread)
        {
            entry.unload_revision += 1;
            entry.applied = None;
        }
        // Wake all lifecycle waiters even if the status immediately returns to idle.
        self.closed.send_modify(|_| {});
    }

    pub(crate) fn close(&self) {
        self.entries.lock().expect("thread setups poisoned").clear();
        self.closed.send_replace(true);
    }

    async fn wait_for_unload(&self, thread: &str, revision: u64) -> anyhow::Result<()> {
        let mut updates = self.closed.subscribe();
        loop {
            anyhow::ensure!(
                !*updates.borrow_and_update(),
                "Codex event stream closed during thread reload"
            );
            if self.revision(thread) > revision {
                return Ok(());
            }
            updates
                .changed()
                .await
                .context("Codex lifecycle event stream closed")?;
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait ThreadLifecycleIo: Send {
    fn setups(&self) -> Arc<ThreadSetups>;
    async fn lifecycle_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value>;
    async fn wait_for_unload(&mut self, thread: &str, revision: u64) -> anyhow::Result<()> {
        self.setups().wait_for_unload(thread, revision).await
    }
}

/// A failure here must propagate, not trigger thread/start or turn replay.
pub(crate) async fn prepare_existing(
    io: &mut impl ThreadLifecycleIo,
    thread: &str,
    mut params: Value,
    options: &InhouseTurnOptions,
) -> anyhow::Result<()> {
    // The caller holds the local exclusive thread route throughout preparation
    // and dispatch. Keep the route subscribed while native subscription changes.
    let result = tokio::time::timeout(PREPARE_TIMEOUT, async {
        let setups = io.setups();
        let revision = setups.revision(thread);
        let desired = ThreadSetup::from_params(&params);
        let read = io
            .lifecycle_request(
                "thread/read",
                json!({"threadId":thread,"includeTurns":false}),
            )
            .await?;
        let snapshot = read.get("thread").context("thread/read missing thread")?;
        anyhow::ensure!(
            snapshot.get("id").and_then(Value::as_str) == Some(thread),
            "thread/read returned another thread"
        );
        anyhow::ensure!(
            snapshot
                .get("canAcceptDirectInput")
                .and_then(Value::as_bool)
                != Some(false),
            "Codex thread does not accept direct input"
        );
        let status = snapshot
            .pointer("/status/type")
            .and_then(Value::as_str)
            .context("thread/read missing native status")?;
        let reload = match status {
            "notLoaded" => false,
            "idle" if setups.matches(thread, &desired) && setups.revision(thread) == revision => {
                return Ok(())
            }
            "idle" => true,
            "active" => return Err(LifecycleFailure::Busy.into()),
            "systemError" => return Err(LifecycleFailure::NativeState.into()),
            _ => anyhow::bail!("Unsupported native Codex thread status: {status}"),
        };
        setups.invalidate(thread);
        if reload {
            let response = io
                .lifecycle_request("thread/unsubscribe", json!({"threadId":thread}))
                .await?;
            anyhow::ensure!(
                matches!(
                    response.get("status").and_then(Value::as_str),
                    Some("unsubscribed" | "notSubscribed" | "notLoaded")
                ),
                "Codex did not confirm thread unsubscribe"
            );
        }
        params["threadId"] = json!(thread);
        params["excludeTurns"] = json!(true);
        options
            .provider_attempt_gate
            .prepare("codex", "thread.resume", &params)
            .await?;
        let developer = params.get("developerInstructions").and_then(Value::as_str).map(str::to_owned);
        // Only an unload observed after this preparation may confirm the reload.
        let reload_revision = setups.revision(thread);
        let response = io.lifecycle_request("thread/resume", params).await?;
        anyhow::ensure!(
            response.pointer("/thread/id").and_then(Value::as_str) == Some(thread),
            "Codex resume changed the thread ID"
        );
        if reload {
            io.wait_for_unload(thread, reload_revision)
                .await
                .context(LifecycleFailure::Instructions)?;
        }
        let confirmed = io
            .lifecycle_request(
                "thread/read",
                json!({"threadId":thread,"includeTurns":false}),
            )
            .await?;
        anyhow::ensure!(
            confirmed.pointer("/thread/id").and_then(Value::as_str) == Some(thread),
            "Codex readback changed the thread ID"
        );
        if confirmed
            .pointer("/thread/status/type")
            .and_then(Value::as_str)
            != Some("idle")
            || confirmed
                .pointer("/thread/canAcceptDirectInput")
                .and_then(Value::as_bool)
                == Some(false)
        {
            return Err(LifecycleFailure::NativeState.into());
        }
        // Native resume reloads configuration but replays the persisted developer
        // context. Explicitly append the current snapshot through the public API;
        // otherwise a successful resume can still send the old role instructions.
        if let Some(developer) = developer.filter(|text| !text.is_empty()) {
            let injection = json!({"threadId":thread,"items":[{
                "type":"message","role":"developer","content":[{
                    "type":"input_text",
                    "text":format!("Current Polar developer instructions. This snapshot supersedes earlier Polar developer instructions for this role.\n\n{developer}")
                }]
            }]});
            options.provider_attempt_gate.prepare("codex", "thread.inject_items", &injection).await?;
            io.lifecycle_request("thread/inject_items", injection).await.context(LifecycleFailure::Instructions)?;
        }
        setups.remember(thread, desired);
        tracing::debug!(
            thread_id = thread,
            reloaded = reload,
            "Codex thread setup applied"
        );
        Ok(())
    })
    .await;
    match result {
        Ok(Err(error)) if error.is::<LifecycleFailure>() => Err(error),
        Ok(result) => result.context(LifecycleFailure::Prepare),
        Err(_) => Err(
            anyhow::anyhow!("Codex thread preparation exceeded 30 seconds")
                .context(LifecycleFailure::Instructions),
        ),
    }
}


