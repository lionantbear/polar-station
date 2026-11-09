//! Authenticated peer authorization. Legacy SQL tables are projections only.
use crate::PolarVault;
use anyhow::{ensure, Result};
use polar_identity::{NodeId, UnixTimestamp};
use polar_trust::{ControlAuthorityRecord, ControlScope, ManagedNodeRecord, TrustRecord};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::RwLock};

const SCOPE: &str = "peer_authorization_v1";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PeerAuthorization {
    #[serde(with = "polar_trust::serde_value::node_id")]
    peer_node_id: NodeId,
    trust: Option<TrustRecord>,
    control: Option<ControlAuthorityRecord>,
    managed: Option<ManagedNodeRecord>,
}

impl PeerAuthorization {
    fn empty(peer_node_id: NodeId) -> Self {
        Self {
            peer_node_id,
            trust: None,
            control: None,
            managed: None,
        }
    }
    fn validate(&self, name: &str) -> Result<()> {
        ensure!(
            self.peer_node_id.to_hex() == name,
            "authorization peer key mismatch"
        );
        ensure!(
            self.trust
                .as_ref()
                .is_none_or(|r| r.peer_node_id == self.peer_node_id),
            "trust peer mismatch"
        );
        ensure!(
            self.control
                .as_ref()
                .is_none_or(|r| r.peer_node_id == self.peer_node_id),
            "control peer mismatch"
        );
        ensure!(
            self.managed
                .as_ref()
                .is_none_or(|r| r.peer_node_id == self.peer_node_id),
            "managed peer mismatch"
        );
        Ok(())
    }
}

impl PolarVault {
    async fn authorization_state(&self) -> Result<&RwLock<HashMap<NodeId, PeerAuthorization>>> {
        self.authorization
            .get_or_try_init(|| async {
                let mut peers = HashMap::new();
                for (name, json) in self.list_secret_material_named_in_scope(SCOPE).await? {
                    let peer: PeerAuthorization = serde_json::from_str(&json)?;
                    peer.validate(&name)?;
                    peers.insert(peer.peer_node_id, peer);
                }
                Ok(RwLock::new(peers))
            })
            .await
    }

    /// Startup must finish this before opening restricted ingress.
    pub async fn initialize_authorization_state(&self) -> Result<()> {
        self.authorization_state().await?;
        self.material_names().await?;
        Ok(())
    }

    async fn change_authorization(
        &self,
        id: NodeId,
        change: impl FnOnce(&mut PeerAuthorization) + Send,
    ) -> Result<()> {
        let _writer = self.authorization_write.lock().await;
        let state = self.authorization_state().await?;
        let mut peer = state
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .get(&id)
            .cloned()
            .unwrap_or_else(|| PeerAuthorization::empty(id));
        change(&mut peer);
        peer.validate(&id.to_hex())?;
        // One encrypted peer record atomically includes both control-bus halves.
        self.put_secret_material(SCOPE, &id.to_hex(), &serde_json::to_string(&peer)?)
            .await?;
        state
            .write()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .insert(id, peer);
        Ok(())
    }

    pub async fn put_trust_record(&self, record: &TrustRecord) -> Result<()> {
        let record = record.clone();
        self.change_authorization(record.peer_node_id, move |peer| peer.trust = Some(record))
            .await
    }
    pub async fn get_trust_record(&self, id: &NodeId) -> Result<Option<TrustRecord>> {
        Ok(self
            .authorization_state()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .get(id)
            .and_then(|r| r.trust.clone()))
    }
    pub async fn list_trust_records(&self) -> Result<Vec<TrustRecord>> {
        let mut records = self
            .authorization_state()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .values()
            .filter_map(|r| r.trust.clone())
            .collect::<Vec<_>>();
        records.sort_by(|a, b| a.peer_node_name.as_str().cmp(b.peer_node_name.as_str()));
        Ok(records)
    }
    pub async fn count_trust_records(&self) -> Result<i64> {
        Ok(self.list_trust_records().await?.len() as i64)
    }
    pub async fn put_control_authority(&self, record: &ControlAuthorityRecord) -> Result<()> {
        let record = record.clone();
        self.change_authorization(record.peer_node_id, move |peer| peer.control = Some(record))
            .await
    }
    pub async fn get_control_authority(
        &self,
        id: &NodeId,
    ) -> Result<Option<ControlAuthorityRecord>> {
        Ok(self
            .authorization_state()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .get(id)
            .and_then(|r| r.control.clone()))
    }
    pub async fn revoke_control_authority(&self, id: &NodeId) -> Result<()> {
        self.change_authorization(*id, |peer| peer.control = None)
            .await
    }
    pub async fn put_managed_node(&self, record: &ManagedNodeRecord) -> Result<()> {
        let record = record.clone();
        self.change_authorization(record.peer_node_id, move |peer| peer.managed = Some(record))
            .await
    }
    pub async fn get_managed_node(&self, id: &NodeId) -> Result<Option<ManagedNodeRecord>> {
        Ok(self
            .authorization_state()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .get(id)
            .and_then(|r| r.managed.clone()))
    }
    pub async fn list_managed_nodes(&self) -> Result<Vec<ManagedNodeRecord>> {
        Ok(self
            .authorization_state()
            .await?
            .read()
            .map_err(|_| anyhow::anyhow!("authorization cache poisoned"))?
            .values()
            .filter_map(|r| r.managed.clone())
            .collect())
    }
    pub async fn revoke_managed_node(&self, id: &NodeId) -> Result<()> {
        self.change_authorization(*id, |peer| peer.managed = None)
            .await
    }
    pub async fn materialize_control_bus_ready_peer_pair(
        &self,
        local: NodeId,
        peer: NodeId,
        at: UnixTimestamp,
        scope: ControlScope,
    ) -> Result<()> {
        self.change_authorization(peer, |record| {
            record.control = Some(ControlAuthorityRecord {
                peer_node_id: peer,
                granted_at: at,
                granted_by: local,
                scope,
            });
            record.managed = Some(ManagedNodeRecord {
                peer_node_id: peer,
                recorded_at: at,
                recorded_by: local,
            });
        })
        .await
    }
    pub async fn clear_control_bus_ready_peer_pair(&self, peer: &NodeId) -> Result<()> {
        self.change_authorization(*peer, |record| {
            record.control = None;
            record.managed = None;
        })
        .await
    }
}
