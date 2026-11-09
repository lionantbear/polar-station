//! Local import and restricted notification pairing, using the existing secure store.
use super::*;
#[cfg(any(test, target_os = "macos"))]
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(any(test, target_os = "macos"))]
use polar_tmux_protocol::{HostPeerScopeV1, NotificationPairingV1};
#[cfg(any(test, target_os = "macos"))]
use rand_core::{OsRng, RngCore};

impl ImessageSender {
    pub(super) async fn setup_current(
        &self,
        settings: ImessageSenderSettingsV1,
        source: &str,
    ) -> Result<NotificationResultV1, &'static str> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (settings, source);
            Err("platform_unsupported")
        }
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME").ok_or("home_unavailable")?;
            let path = std::path::PathBuf::from(home)
                .join("Library/Application Support/Codex iMessage Bridge/config.json");
            self.setup_from_path(settings, source, &path).await
        }
    }

    #[cfg(any(test, target_os = "macos"))]
    pub(super) async fn setup_from_path(
        &self,
        mut settings: ImessageSenderSettingsV1,
        source: &str,
        path: &std::path::Path,
    ) -> Result<NotificationResultV1, &'static str> {
        use std::os::unix::fs::MetadataExt;
        if settings.recipient_credential_ref.is_none() {
            let metadata =
                std::fs::symlink_metadata(path).map_err(|_| "current_recipient_unavailable")?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
                || metadata.len() > 65536
            {
                return Err("current_recipient_not_private");
            }
            let bytes =
                Zeroizing::new(std::fs::read(path).map_err(|_| "current_recipient_unavailable")?);
            #[derive(serde::Deserialize)]
            struct CurrentRecipient {
                recipient: String,
            }
            let document: CurrentRecipient =
                serde_json::from_slice(&bytes).map_err(|_| "current_recipient_invalid")?;
            let request = NotificationCommandV1::SetSenderRecipient {
                expected_revision: settings.revision,
                recipient: document.recipient,
            };
            request
                .validate()
                .map_err(|_| "current_recipient_invalid")?;
            let NotificationCommandV1::SetSenderRecipient { recipient, .. } = request else {
                unreachable!()
            };
            let NotificationResultV1::Settings { settings: saved } = self
                .set_recipient(settings.clone(), settings.revision, recipient)
                .await?
            else {
                return Err("settings_unavailable");
            };
            settings = saved;
        }
        self.pair_source(settings, source).await
    }

    // The same key is reused for an already authorized source; retries cannot
    // silently revoke an installed execution Host or export a root credential.
    #[cfg(any(test, target_os = "macos"))]
    async fn pair_source(
        &self,
        mut settings: ImessageSenderSettingsV1,
        source: &str,
    ) -> Result<NotificationResultV1, &'static str> {
        let account = HostPeerScopeV1::NotificationSender
            .credential_account(source)
            .map_err(|_| "invalid_source")?;
        let credentials = self.credentials.clone();
        let key = tokio::task::spawn_blocking(move || {
            if let Some(key) = credentials
                .load("com.polarealm.PolarStation.peers", &account)
                .map_err(|_| "credential_unavailable")?
            {
                return Ok::<_, &'static str>(Zeroizing::new(key));
            }
            let mut bytes = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(bytes.as_mut());
            let encoded = Zeroizing::new(STANDARD.encode(&bytes[..]));
            credentials
                .save("com.polarealm.PolarStation.peers", &account, &encoded)
                .map_err(|_| "credential_unavailable")?;
            Ok(encoded)
        })
        .await
        .map_err(|_| "credential_unavailable")??;
        if !settings
            .allowed_source_principals
            .iter()
            .any(|s| s == source)
        {
            settings.allowed_source_principals.push(source.into());
            settings = self
                .repository
                .update_imessage_sender(settings.revision, settings)
                .map_err(|_| "revision_conflict")?;
        }
        let pairing = NotificationPairingV1 {
            schema: "polar-station-notification-pair-v1".into(),
            source_host_id: source.into(),
            sender_host_id: self.host_id.clone(),
            sender_generation: settings.sender_generation,
            destination_ref: settings
                .recipient_credential_ref
                .ok_or("recipient_missing")?,
            notification_key: key.to_string(),
        };
        pairing.validate().map_err(|_| "pairing_invalid")?;
        Ok(NotificationResultV1::Pairing { pairing })
    }
}
