use anyhow::{anyhow, Result};
use polar_identity::{IdentitySeed, NodeId, UnixTimestamp};
use polar_trust::ControlScope;
use std::sync::Arc;

mod cipher;
pub mod keys;
pub mod protection;
pub use cipher::{SoftwareVaultCipher, VaultCipher, VaultCiphertext};

pub struct PolarVault {
    store: Arc<dyn VaultStore>,
    cipher: Arc<dyn VaultCipher>,
    authorization: tokio::sync::OnceCell<
        std::sync::RwLock<std::collections::HashMap<NodeId, authorization::PeerAuthorization>>,
    >,
    local_chat_changes: tokio::sync::watch::Sender<u64>,
    secret_changes: tokio::sync::watch::Sender<u64>,
    key_directory: tokio::sync::OnceCell<keys::directory::ManagedKeyDirectory>,
    authorization_write: tokio::sync::Mutex<()>,
    material_index:
        tokio::sync::OnceCell<std::sync::RwLock<std::collections::HashSet<(String, String)>>>,
    material_write: tokio::sync::Mutex<()>,
    management_write: tokio::sync::Mutex<()>,
    public_state: tokio::sync::OnceCell<std::sync::RwLock<public_state::PublicState>>,
}
mod authorization;
mod local_chat;
mod public_state;
pub type AuthenticatedPublicStateSnapshot = (u64, Vec<(String, String, String)>);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySecurityState {
    pub name: String,
    pub status: String,
    pub version: Option<String>,
    pub installed_at: Option<u64>,
    pub last_healthcheck_at: Option<u64>,
    pub auth_present: bool,
    pub auth_sha256: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ManagementRequestStatus {
    Pending,
    Approved,
    Rejected,
}
impl ManagementRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagementRequestRecord {
    pub request_id: String,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub requester_node_id: NodeId,
    pub requester_node_name: Option<String>,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub manager_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    pub request_message_id: String,
    pub response_message_id: Option<String>,
    pub request_transport_plane: String,
    pub target_channel_id: Option<String>,
    pub target_thread_id: Option<String>,
    pub target_inhouse_polar_id: Option<String>,
    pub requested_scope: ControlScope,
    pub request_text: Option<String>,
    pub endpoint_hint: Option<String>,
    pub status: ManagementRequestStatus,
    #[serde(with = "polar_trust::serde_value::timestamp")]
    pub requested_at: UnixTimestamp,
    #[serde(with = "polar_trust::serde_value::optional_timestamp")]
    pub resolved_at: Option<UnixTimestamp>,
}

mod credential_bundle;
mod inhouse_identity;
mod operations;
mod secrets;
pub use credential_bundle::{CredentialBundle, CredentialBundleResolver, CredentialBundleStore};
pub use operations::VaultStore;
pub use secrets::{
    CredentialResolver, SecretId, SecretImportMetadata, SecretInput, SecretKind, SecretLease,
    SecretMetadata, SecretPage, SecretQuery, StoredSecret, VaultAuthority,
};
pub use secrets::{Ed25519SecretExecutor, SecretUseSession, SecretUseTiming};
