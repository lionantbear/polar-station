use anyhow::{Context, Result};
use polar_voice_provider_codex::{
    AuthCredentialsStoreMode, AuthKeyringBackendKind, CodexVoiceProvider,
};
use std::{path::PathBuf, sync::Arc};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeAuthOptions {
    store: AuthCredentialsStoreMode,
    keyring: AuthKeyringBackendKind,
}
impl NativeAuthOptions {
    fn parse(text: &str) -> Result<Self, String> {
        let config = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| "Codex config.toml is invalid; fix it before using Dictate".to_string())?;
        let store = match config.get("cli_auth_credentials_store") {
            None => AuthCredentialsStoreMode::default(),
            Some(item) => serde_json::from_value(serde_json::Value::String(
                item.as_str()
                    .ok_or("Codex credential store must be a string")?
                    .into(),
            ))
            .map_err(|_| "Unsupported Codex credential store".to_string())?,
        };
        if store == AuthCredentialsStoreMode::Ephemeral {
            return Err("Codex uses process-only credentials; sign in using a persistent native store to use Dictate".into());
        }
        let keyring = match config
            .get("features")
            .and_then(|v| v.get("secret_auth_storage"))
        {
            None => AuthKeyringBackendKind::default(),
            Some(item) => match item.as_bool() {
                Some(true) => AuthKeyringBackendKind::Secrets,
                Some(false) => AuthKeyringBackendKind::Direct,
                None => return Err("Codex secret_auth_storage must be a boolean".into()),
            },
        };
        Ok(Self { store, keyring })
    }
}

pub struct NativeAccount {
    pub credentials: Arc<CodexVoiceProvider>,
    home: PathBuf,
    options: NativeAuthOptions,
}
impl NativeAccount {
    pub async fn open(home: PathBuf) -> Result<Self> {
        let text = match tokio::fs::read(home.join("config.toml")).await {
            Ok(bytes) => {
                anyhow::ensure!(bytes.len() <= 1024 * 1024, "Codex configuration too large");
                String::from_utf8(bytes)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let options = NativeAuthOptions::parse(&text).map_err(anyhow::Error::msg)?;
        let credentials = Arc::new(
            CodexVoiceProvider::from_codex_auth_store(home.clone(), options.store, options.keyring)
                .await,
        );
        Ok(Self {
            credentials,
            home,
            options,
        })
    }
    pub async fn status(&self) -> Result<serde_json::Value> {
        let manager = self
            .credentials
            .auth_manager()
            .context("native account missing")?;
        manager.reload().await;
        let auth = manager.auth().await;
        Ok(
            serde_json::json!({"signed_in":auth.as_ref().is_some_and(|a| a.uses_codex_backend()),
            "account":auth.and_then(|a|a.get_account_email()),"encoder":"built_in_opus_webm",
            "cleanup_model":"gpt-6-luna","cleanup_reasoning":"low"}),
        )
    }
    pub fn login(&self) -> Result<codex_login::LoginServer> {
        let factory = codex_http_client::HttpClientFactory::new(
            codex_http_client::OutboundProxyPolicy::ReqwestDefault,
        );
        let mut options = codex_login::ServerOptions::new(
            self.home.clone(),
            codex_login::CLIENT_ID.into(),
            None,
            self.options.store,
            self.options.keyring,
            codex_login::AuthRouteConfig::from_http_client_factory(factory),
        );
        // The native UI opens the URL; a background Host need not own a desktop browser.
        options.open_browser = false;
        Ok(codex_login::run_login_server(options)?)
    }
}
#[async_trait::async_trait]
impl polar_inhouse_backend_adam::AdamOAuthCredentialResolver for NativeAccount {
    async fn resolve_adam_oauth_credential(
        &self,
        _: &polar_inhouse_backend_adam::OpenAiConfig,
    ) -> std::result::Result<
        polar_inhouse_backend_adam::AdamOAuthCredential,
        polar_inhouse_backend_adam::AdamBackendError,
    > {
        use polar_inhouse_backend_adam::{AdamBackendError, AdamOAuthCredential, SecretString};
        let manager = self
            .credentials
            .auth_manager()
            .ok_or_else(|| AdamBackendError::Auth("Sign in in Settings → Voice".into()))?;
        manager.reload().await;
        let auth = manager
            .auth()
            .await
            .filter(|a| a.uses_codex_backend())
            .ok_or_else(|| AdamBackendError::Auth("Sign in in Settings → Voice".into()))?;
        if manager.refresh_failure_for_auth(&auth).is_some() {
            return Err(AdamBackendError::Auth(
                "Sign in again in Settings → Voice".into(),
            ));
        }
        let token = auth
            .get_token()
            .map_err(|_| AdamBackendError::Auth("Sign in again in Settings → Voice".into()))?;
        Ok(AdamOAuthCredential {
            token_type: "Bearer".into(),
            access_token: SecretString::new(token),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            account_id: auth.get_account_id(),
            installation_id: None,
        })
    }
}
