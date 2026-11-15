//! Local chat grants are encrypted Vault authority, never an OS/config allowlist.
use crate::PolarVault;
use anyhow::{ensure, Result};
use polar_identity::NodeId;
use polar_trust::LocalChatAuthority;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SCOPE: &str = "local_chat_authority_v1";

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grants {
    initialized: bool,
    grants: BTreeMap<u32, LocalChatAuthority>,
}

impl PolarVault {
    async fn local_chat_grants(&self, node: &NodeId) -> Result<Grants> {
        let grants: Grants = match self.get_secret_material(SCOPE, &node.to_hex()).await? {
            Some(value) => serde_json::from_str(&value)?,
            None => Grants::default(),
        };
        for (uid, grant) in &grants.grants {
            ensure!(
                grant.instance_node_id == *node && grant.uid == *uid && grant.revision > 0,
                "invalid local chat authority binding"
            );
        }
        Ok(grants)
    }

    /// Installation-only seed. A revoked owner is never restored by replay.
    pub async fn initialize_local_chat_owner(&self, node: &NodeId, uid: u32) -> Result<()> {
        let _writer = self.authorization_write.lock().await;
        let mut grants = self.local_chat_grants(node).await?;
        if grants.initialized {
            return Ok(());
        }
        grants.grants.entry(uid).or_insert(LocalChatAuthority {
            instance_node_id: *node,
            uid,
            revision: 1,
            active: true,
        });
        grants.initialized = true;
        self.put_secret_material(SCOPE, &node.to_hex(), &serde_json::to_string(&grants)?)
            .await?;
        self.local_chat_changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(())
    }

    pub async fn local_chat_authority(
        &self,
        node: &NodeId,
        uid: u32,
    ) -> Result<Option<LocalChatAuthority>> {
        Ok(self
            .local_chat_grants(node)
            .await?
            .grants
            .get(&uid)
            .cloned())
    }

    pub async fn list_local_chat_authorities(
        &self,
        node: &NodeId,
    ) -> Result<Vec<LocalChatAuthority>> {
        Ok(self
            .local_chat_grants(node)
            .await?
            .grants
            .into_values()
            .collect())
    }

    /// Called only by the instance's existing privileged management surface.
    /// Persist before notification: a failed write cannot grant or revoke access.
    pub async fn set_local_chat_authority(
        &self,
        node: &NodeId,
        uid: u32,
        active: bool,
    ) -> Result<LocalChatAuthority> {
        let _writer = self.authorization_write.lock().await;
        let mut grants = self.local_chat_grants(node).await?;
        let revision = match grants.grants.get(&uid) {
            Some(previous) => previous
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("chat grant revision exhausted"))?,
            None => 1,
        };
        let grant = LocalChatAuthority {
            instance_node_id: *node,
            uid,
            revision,
            active,
        };
        grants.grants.insert(uid, grant.clone());
        self.put_secret_material(SCOPE, &node.to_hex(), &serde_json::to_string(&grants)?)
            .await?;
        self.local_chat_changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(grant)
    }

    /// Invalidates live leases without polling the database or watching config files.
    pub fn subscribe_local_chat_authority(&self) -> tokio::sync::watch::Receiver<u64> {
        self.local_chat_changes.subscribe()
    }
}
