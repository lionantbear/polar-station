use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::PolarVault;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialBundle {
    PolarInkDevice {
        device_id: String,
        credential_version: String,
        hmac_secret: Vec<u8>,
    },
    TelegramBot {
        bot_token: String,
    },
    TelegramGrammers {
        api_id: String,
        api_hash: String,
    },
    OauthApp {
        client_id: String,
        client_secret: String,
        refresh_token: Option<String>,
    },
    ProviderSession {
        session_blob: Vec<u8>,
    },
    OpaqueJson(serde_json::Value),
}

#[async_trait]
pub trait CredentialBundleResolver: Send + Sync {
    async fn resolve_credential_bundle(&self, credential_ref: &str) -> Result<CredentialBundle>;
}

#[async_trait]
pub trait CredentialBundleStore: CredentialBundleResolver {
    async fn store_credential_bundle(
        &self,
        credential_ref: &str,
        bundle: &CredentialBundle,
    ) -> Result<()>;

    async fn delete_credential_bundle(&self, credential_ref: &str) -> Result<()>;
}

impl PolarVault {
    pub async fn store_credential_bundle(
        &self,
        credential_ref: &str,
        bundle: &CredentialBundle,
    ) -> Result<()> {
        self.put_secret_material(
            "credential_bundle",
            credential_ref,
            &serde_json::to_string(bundle).context("serialize credential bundle")?,
        )
        .await
    }
}

#[async_trait]
impl CredentialBundleStore for PolarVault {
    async fn store_credential_bundle(
        &self,
        credential_ref: &str,
        bundle: &CredentialBundle,
    ) -> Result<()> {
        PolarVault::store_credential_bundle(self, credential_ref, bundle).await
    }

    async fn delete_credential_bundle(&self, credential_ref: &str) -> Result<()> {
        self.delete_secret_material("credential_bundle", credential_ref)
            .await
    }
}

#[async_trait]
impl CredentialBundleResolver for PolarVault {
    async fn resolve_credential_bundle(&self, credential_ref: &str) -> Result<CredentialBundle> {
        let raw = self
            .get_secret_material("credential_bundle", credential_ref)
            .await?
            .with_context(|| format!("credential bundle not found: {credential_ref}"))?;
        serde_json::from_str(&raw).context("decode credential bundle")
    }
}
