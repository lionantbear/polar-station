//! Standalone Host account composition. Credential persistence and authentication
//! remain owned by codex-login; consumers receive the same existing AuthManager.
use anyhow::{Context, Result};
use codex_login::{AuthCredentialsStoreMode, AuthKeyringBackendKind, AuthManager, AuthRouteConfig};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
struct NativeAuthOptions {
    home: PathBuf,
    store: AuthCredentialsStoreMode,
    keyring: AuthKeyringBackendKind,
}
impl NativeAuthOptions {
    fn parse(text: &str, home: &Path) -> Result<Self> {
        // Use the pinned Codex schema and its path resolution; do not duplicate
        // auth-home parsing or depend on the complete Codex runtime for config.
        let _guard = codex_config::AbsolutePathBufGuard::new(home);
        let config: codex_config::config_toml::ConfigToml = toml::from_str(text)
            .context("Codex config.toml is invalid; fix it before using the account")?;
        let store = config.cli_auth_credentials_store.unwrap_or_default();
        anyhow::ensure!(
            store != AuthCredentialsStoreMode::Ephemeral,
            "Codex uses process-only credentials; sign in using a persistent native store"
        );
        let keyring = match config
            .features
            .as_ref()
            .and_then(|features| features.entries().get("secret_auth_storage").copied())
        {
            Some(true) => AuthKeyringBackendKind::Secrets,
            Some(false) => AuthKeyringBackendKind::Direct,
            None => AuthKeyringBackendKind::default(),
        };
        Ok(Self {
            home: config
                .auth_home
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| home.into()),
            store,
            keyring,
        })
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LoginStatus {
    Idle,
    Pending,
    Succeeded,
    Failed,
    Cancelled,
}

pub(crate) struct NativeAccount {
    pub manager: Arc<AuthManager>,
    home: PathBuf,
    options: NativeAuthOptions,
    login_status: tokio::sync::Mutex<LoginStatus>,
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
        let options = NativeAuthOptions::parse(&text, &home)?;
        let factory = codex_http_client::HttpClientFactory::new(
            codex_http_client::OutboundProxyPolicy::ReqwestDefault,
        );
        let manager = Arc::new(
            AuthManager::new(
                options.home.clone(),
                false,
                options.store,
                None,
                None,
                options.keyring,
                AuthRouteConfig::from_http_client_factory(factory),
            )
            .await,
        );
        Ok(Self {
            manager,
            home: options.home.clone(),
            options,
            login_status: tokio::sync::Mutex::new(LoginStatus::Idle),
        })
    }

    /// A snapshot: opening a page neither touches credential storage nor refreshes tokens.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let auth = self.manager.auth_cached();
        let rejected = auth
            .as_ref()
            .is_some_and(|a| self.manager.refresh_failure_for_auth(a).is_some());
        let signed_in = !rejected && auth.as_ref().is_some_and(|a| a.uses_codex_backend());
        let login = *self.login_status.lock().await;
        let message = match login {
            LoginStatus::Pending => "Complete sign-in in the browser.",
            LoginStatus::Failed => {
                "Sign-in did not complete. Retry sign-in or check the account connection."
            }
            LoginStatus::Cancelled => "Sign-in cancelled.",
            _ if rejected => "Sign in again to restore account access.",
            _ if signed_in => "Account connected.",
            _ => "Sign in to connect an account. No CLI installation is needed.",
        };
        Ok(serde_json::json!({"signed_in":signed_in, "message":message,
            "account":auth.and_then(|a|a.get_account_email()), "login_status": login,
            "login_error":matches!(login,LoginStatus::Failed).then_some("Sign-in did not complete. Retry sign-in or check the account connection."),
            "reauth_required": rejected}))
    }

    /// Explicit import/recheck of the existing native store. A failed read must
    /// preserve the cached owner and surface the error rather than report logout.
    pub async fn refresh(&self) -> Result<bool> {
        self.manager
            .reload_with_cached_auth()
            .await
            .context("Could not read the existing Codex credential store")
    }
    pub async fn set_login_status(&self, status: LoginStatus) {
        *self.login_status.lock().await = status;
    }
    pub fn login(&self) -> Result<codex_login::LoginServer> {
        let mut options = codex_login::ServerOptions::new(
            self.home.clone(),
            codex_login::CLIENT_ID.into(),
            None,
            self.options.store,
            self.options.keyring,
            AuthRouteConfig::from_http_client_factory(self.manager.http_client_factory()),
        );
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
        let auth = self
            .manager
            .auth()
            .await
            .filter(|a| a.uses_codex_backend())
            .ok_or_else(|| {
                AdamBackendError::Auth("Sign in through Local Diagnostics → Account".into())
            })?;
        if self.manager.refresh_failure_for_auth(&auth).is_some() {
            return Err(AdamBackendError::Auth(
                "Sign in again through Local Diagnostics → Account".into(),
            ));
        }
        let token = auth.get_token().map_err(|_| {
            AdamBackendError::Auth("Sign in again through Local Diagnostics → Account".into())
        })?;
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


