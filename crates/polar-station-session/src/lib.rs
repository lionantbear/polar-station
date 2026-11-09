//! Station management contracts and a mounted runtime resource. The Host owns
//! inventory, connections, input, fences and receipts; this owns no second store.
use anyhow::{ensure, Context, Result};
pub use polar_station_inbox::DeliveryStatus;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock, Weak};

pub mod routing;

pub mod methods {
    pub const LIST: &str = "station.sessions.list";
    pub const SEND: &str = "station.sessions.send";
    pub const RECEIPT: &str = "station.sessions.receipt";
    pub const OFFER: &str = "station.sessions.offer";
    pub const SEND_CONFIRMED: &str = "station.sessions.send_confirmed";
    pub const ALL: &[&str] = &[LIST, SEND, RECEIPT, OFFER, SEND_CONFIRMED];
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    pub target_id: Option<String>,
}
/// Existing authenticated local Session operation envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalRequest {
    List {
        input: ListRequest,
    },
    Send {
        principal: String,
        input: SendRequest,
    },
    Receipt {
        principal: String,
        input: ReceiptRequest,
    },
}

/// An access path, not a global physical-Host deduplication key or capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionReference {
    pub source_host_id: String,
    pub host_epoch: String,
    pub target_id: String,
    /// Target configuration generation; unrelated tmux topology does not invalidate it.
    pub target_revision: u64,
    pub server_generation: String,
    pub session_id: String,
    pub session_created: i64,
    pub pane_id: String,
    pub pane_incarnation: String,
}
impl SessionReference {
    pub fn validate(&self) -> Result<()> {
        for s in [&self.source_host_id, &self.host_epoch, &self.target_id] {
            identifier(s)?;
        }
        ensure!(
            self.target_revision > 0 && self.session_created >= 0,
            "invalid Session observation version"
        );
        for (s, prefix) in [(&self.session_id, '$'), (&self.pane_id, '%')] {
            ensure!(
                s.strip_prefix(prefix).is_some_and(|v| !v.is_empty()
                    && v.bytes().all(|b| b.is_ascii_digit())
                    && v.parse::<u64>().is_ok()),
                "original tmux object ID required"
            );
        }
        for s in [&self.server_generation, &self.pane_incarnation] {
            ensure!(
                s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid incarnation fingerprint"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendRequest {
    pub intent_id: String,
    pub reference: SessionReference,
    pub text: String,
    pub press_enter: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRequest {
    pub intent_id: String,
    pub reference: SessionReference,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetView {
    pub target_id: String,
    pub label: String,
    pub available: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneView {
    #[serde(default)]
    pub active: bool,
    pub reference: SessionReference,
    pub working_directory: String,
    pub available: bool,
}
/// Facts from the same original agent catalog used by the Session list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentContext {
    pub title: String,
    pub preview: Option<String>,
    pub status: String,
    pub updated_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionView {
    #[serde(default)]
    pub agent: Option<AgentContext>,
    pub target_id: String,
    pub session_id: String,
    /// Stable Station reference proven by the marker in this live observation.
    #[serde(default)]
    pub managed_reference: Option<String>,
    pub name: String,
    pub kind: String,
    pub original_agent_id: Option<String>,
    pub summary: Vec<String>,
    pub panes: Vec<PaneView>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub source_host_id: String,
    #[serde(default)]
    pub source_host_name: String,
    pub host_epoch: String,
    pub targets: Vec<TargetView>,
    pub sessions: Vec<SessionView>,
}
/// Read-only validation of an existing terminal before a client opens its view.
/// This carries resource facts, never an executable command or credentials.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedSessionView {
    pub reference: SessionReference,
    pub label: String,
    pub agent_type: String,
    pub working_directory: String,
    pub summary: Vec<String>,
}
/// Delivered means terminal input, never task acceptance or completion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub intent_id: String,
    pub reference: SessionReference,
    pub state: DeliveryStatus,
    pub error: Option<String>,
}
#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    async fn list(&self, request: ListRequest) -> Result<Catalog>;
    async fn send(&self, principal: &str, request: SendRequest) -> Result<Receipt>;
    async fn receipt(&self, principal: &str, request: ReceiptRequest) -> Result<Receipt>;
}
#[derive(Default)]
pub struct SessionRuntime {
    backend: RwLock<Option<Weak<dyn SessionBackend>>>,
}
impl SessionRuntime {
    pub fn bind(&self, backend: &Arc<dyn SessionBackend>) -> Result<()> {
        let mut slot = self.backend.write().expect("Station runtime resource");
        ensure!(
            slot.as_ref().and_then(Weak::upgrade).is_none(),
            "Station Session runtime already mounted"
        );
        *slot = Some(Arc::downgrade(backend));
        Ok(())
    }
    fn backend(&self) -> Result<Arc<dyn SessionBackend>> {
        self.backend
            .read()
            .expect("Station runtime resource")
            .as_ref()
            .and_then(Weak::upgrade)
            .context("Station Session management is unavailable on this host")
    }
    pub async fn list(&self, request: ListRequest) -> Result<Catalog> {
        if let Some(id) = &request.target_id {
            identifier(id)?;
        }
        self.backend()?.list(request).await
    }
    pub async fn prepare_view(&self, reference: SessionReference) -> Result<PreparedSessionView> {
        reference.validate()?;
        let catalog = self.list(ListRequest { target_id: Some(reference.target_id.clone()) }).await?;
        let (session, pane) = catalog.sessions.iter().find_map(|session| session.panes.iter()
            .find(|pane| pane.available && pane.reference == reference)
            .map(|pane|(session,pane))).context("The selected terminal changed; choose a fresh session before sending.")?;
        ensure!(catalog.targets.iter().any(|target| target.available && target.target_id == reference.target_id), "The target Host is unavailable.");
        Ok(PreparedSessionView { reference, label:session.name.clone(), agent_type:session.kind.clone(),
            working_directory:pane.working_directory.clone(), summary:session.summary.clone() })
    }
    pub async fn send(&self, principal: &str, request: SendRequest) -> Result<Receipt> {
        identifier(principal)?;
        identifier(&request.intent_id)?;
        request.reference.validate()?;
        validate_terminal_text(&request.text)?;
        self.backend()?.send(principal, request).await
    }
    pub async fn receipt(&self, principal: &str, request: ReceiptRequest) -> Result<Receipt> {
        identifier(principal)?;
        identifier(&request.intent_id)?;
        request.reference.validate()?;
        self.backend()?.receipt(principal, request).await
    }
}
pub fn validate_terminal_text(text: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty()
            && text.len() <= 8192
            && !text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "terminal text must be nonempty, at most 8192 bytes and contain no control sequences"
    );
    Ok(())
}
fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control),
        "invalid Station management identifier"
    );
    Ok(())
}
