//! Receipt and authorization owner for the Messages capability embedded in Host.
use crate::host_identity::CredentialStore;
use polar_station_settings::{ImessageSenderSettingsV1, SettingsRepository, UpdateError};
use polar_station_store_sqlite::{ImessageReceiptRow, SqliteCodexStore};
use polar_tmux_protocol::{
    NotificationCommandV1, NotificationReceiptStatusV1, NotificationReceiptV1,
    NotificationRequestEnvelopeV1, NotificationResponseEnvelopeV1, NotificationResultV1,
    NotificationSenderStatusV1,
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

mod setup;

pub(crate) type PlatformFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlatformSendOutcome {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Sent,
    Rejected(&'static str),
    Unknown(&'static str),
}

/// The platform adapter owns only main-thread Messages interaction.
pub(crate) trait MessagesPlatform: Send + Sync {
    fn supported(&self) -> bool;
    fn authorized(&self, request_permission: bool) -> PlatformFuture<bool>;
    fn send(
        &self,
        recipient: Zeroizing<String>,
        text: Zeroizing<String>,
    ) -> PlatformFuture<PlatformSendOutcome>;
}

pub(crate) struct SystemMessagesPlatform;
impl MessagesPlatform for SystemMessagesPlatform {
    fn supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            macos::available()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    fn authorized(&self, request_permission: bool) -> PlatformFuture<bool> {
        #[cfg(target_os = "macos")]
        {
            Box::pin(macos::authorized(request_permission))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = request_permission;
            Box::pin(async { false })
        }
    }
    fn send(
        &self,
        recipient: Zeroizing<String>,
        text: Zeroizing<String>,
    ) -> PlatformFuture<PlatformSendOutcome> {
        #[cfg(target_os = "macos")]
        {
            Box::pin(macos::send(recipient, text))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (recipient, text);
            Box::pin(async { PlatformSendOutcome::Rejected("platform_unsupported") })
        }
    }
}

pub(crate) struct ImessageSender {
    host_id: String,
    repository: SettingsRepository,
    ledger: SqliteCodexStore,
    platform: Arc<dyn MessagesPlatform>,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
    serial: Mutex<()>,
    events: super::application_state::ApplicationStatePublisher,
}

/// A cancelled future cannot establish that Apple Events did not send.
struct SendingClaim {
    ledger: SqliteCodexStore,
    principal: String,
    request_id: String,
    completed: bool,
}
impl Drop for SendingClaim {
    fn drop(&mut self) {
        if !self.completed
            && self
                .ledger
                .finish_imessage(
                    &self.principal,
                    &self.request_id,
                    "unknown",
                    Some("sender_interrupted".into()),
                    now_ms(),
                )
                .is_err()
        {
            tracing::warn!("Messages cancellation receipt could not be persisted");
        }
    }
}

impl ImessageSender {
    pub(crate) fn new(
        host_id: String,
        repository: SettingsRepository,
        ledger: SqliteCodexStore,
        platform: Arc<dyn MessagesPlatform>,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
    ) -> Self {
        Self {
            host_id,
            repository,
            ledger,
            platform,
            credentials,
            serial: Mutex::new(()),
            events: Default::default(),
        }
    }
    pub(crate) fn with_events(
        mut self,
        events: super::application_state::ApplicationStatePublisher,
    ) -> Self {
        self.events = events;
        self
    }

    /// `local` is derived from the authenticated transport, never request data.
    pub(crate) async fn dispatch(
        &self,
        principal: &str,
        local: bool,
        request: NotificationRequestEnvelopeV1,
    ) -> NotificationResponseEnvelopeV1 {
        let changes_state = matches!(
            &request.command,
            NotificationCommandV1::UpdateSenderSettings { .. }
                | NotificationCommandV1::SetupCurrentSender { .. }
                | NotificationCommandV1::SetSenderRecipient { .. }
                | NotificationCommandV1::SendNotification { .. }
                | NotificationCommandV1::RequestMessagesAuthorization
        );
        let result = if request.validate().is_err() {
            error("invalid_request")
        } else {
            self.execute(principal, local, request.command)
                .await
                .unwrap_or_else(error)
        };
        if changes_state && !matches!(&result, NotificationResultV1::Error { .. }) {
            self.events
                .publish(polar_tmux_protocol::ApplicationStateApiV1::NotificationV1);
            self.events.work(super::application_state::DeliveryChannel::Imessage);
        }
        NotificationResponseEnvelopeV1 {
            protocol_version: polar_tmux_protocol::PROTOCOL_VERSION,
            api: request.api,
            request_id: request.request_id,
            result,
        }
    }

    async fn set_recipient(
        &self,
        settings: ImessageSenderSettingsV1,
        expected_revision: u64,
        recipient: String,
    ) -> Result<NotificationResultV1, &'static str> {
        let credential_ref = format!(
            "imessage-recipient-{}",
            crate::host_identity::random_uuid_v4()
        );
        let account = credential_ref.clone();
        let credentials = self.credentials.clone();
        let recipient = Zeroizing::new(recipient);
        tokio::task::spawn_blocking(move || {
            credentials.save("com.polarealm.PolarStation.channels", &account, &recipient)
        })
        .await
        .map_err(|_| "credential_unavailable")?
        .map_err(|_| "credential_unavailable")?;
        let mut next = settings;
        next.recipient_credential_ref = Some(credential_ref);
        next.sender_generation = crate::host_identity::random_uuid_v4();
        let settings = self
            .repository
            .update_imessage_sender(expected_revision, next)
            .map_err(|_| "settings_unavailable")?;
        Ok(NotificationResultV1::Settings { settings })
    }

    async fn execute(
        &self,
        principal: &str,
        local: bool,
        command: NotificationCommandV1,
    ) -> Result<NotificationResultV1, &'static str> {
        // Configuration mutations serialize with sending. Reads remain available
        // while Messages is working, and send admission uses try_lock below.
        let _guard = if matches!(
            &command,
            NotificationCommandV1::UpdateSenderSettings { .. }
                | NotificationCommandV1::SetupCurrentSender { .. }
                | NotificationCommandV1::SetSenderRecipient { .. }
                | NotificationCommandV1::RequestMessagesAuthorization
        ) {
            Some(self.serial.lock().await)
        } else {
            None
        };
        let settings = self
            .repository
            .load_imessage_sender()
            .map_err(|_| "settings_unavailable")?;
        if !local
            && !settings
                .allowed_source_principals
                .iter()
                .any(|p| p == principal)
        {
            return Err("source_denied");
        }
        match command {
            NotificationCommandV1::Connector { .. } => Err("connector_unavailable"),
            NotificationCommandV1::SetupCurrentSender {
                expected_revision,
                source_host_id,
            } => {
                if !local {
                    return Err("scope_denied");
                }
                if settings.revision != expected_revision {
                    return Err("revision_conflict");
                }
                self.setup_current(settings, &source_host_id).await
            }
            NotificationCommandV1::SetSenderRecipient {
                expected_revision,
                recipient,
            } => {
                if !local {
                    return Err("scope_denied");
                }
                if settings.revision != expected_revision {
                    return Err("revision_conflict");
                }
                self.set_recipient(settings, expected_revision, recipient)
                    .await
            }
            NotificationCommandV1::GetSenderSettings => {
                if !local {
                    return Err("scope_denied");
                }
                Ok(NotificationResultV1::Settings { settings })
            }
            NotificationCommandV1::UpdateSenderSettings {
                expected_revision,
                settings: mut next,
            } => {
                if !local {
                    return Err("scope_denied");
                }
                if settings.recipient_credential_ref != next.recipient_credential_ref
                    || settings.sender_generation.is_empty()
                {
                    next.sender_generation = crate::host_identity::random_uuid_v4();
                } else {
                    next.sender_generation = settings.sender_generation;
                }
                let settings = self
                    .repository
                    .update_imessage_sender(expected_revision, next)
                    .map_err(|e| match e {
                        UpdateError::RevisionConflict { .. } => "revision_conflict",
                        _ => "settings_unavailable",
                    })?;
                Ok(NotificationResultV1::Settings { settings })
            }
            NotificationCommandV1::GetSenderStatus
            | NotificationCommandV1::RequestMessagesAuthorization => {
                let request_permission =
                    matches!(command, NotificationCommandV1::RequestMessagesAuthorization);
                if request_permission && !local {
                    return Err("scope_denied");
                }
                let supported = self.platform.supported();
                let authorized = supported
                    && tokio::time::timeout(
                        std::time::Duration::from_secs(if request_permission { 120 } else { 5 }),
                        self.platform.authorized(request_permission),
                    )
                    .await
                    .unwrap_or(false);
                Ok(NotificationResultV1::Status {
                    status: NotificationSenderStatusV1 {
                        host_id: self.host_id.clone(),
                        supported,
                        enabled: settings.enabled,
                        authorized,
                        sender_generation: settings.sender_generation,
                        error_code: if !supported {
                            Some("platform_unsupported".into())
                        } else if !authorized {
                            Some("messages_authorization_required".into())
                        } else {
                            None
                        },
                    },
                })
            }
            NotificationCommandV1::GetReceipt {
                request_id,
                sender_generation,
            } => {
                let row = self
                    .ledger
                    .imessage_receipt(principal, &request_id)
                    .map_err(|_| "storage_unavailable")?
                    .ok_or("receipt_not_found")?;
                if row.sender_generation != sender_generation {
                    return Err("generation_mismatch");
                }
                Ok(receipt(row))
            }
            NotificationCommandV1::SendNotification {
                request_id,
                expected_sender_generation,
                destination_ref,
                text,
            } => {
                self.send(
                    principal,
                    &settings,
                    request_id,
                    expected_sender_generation,
                    destination_ref,
                    text,
                )
                .await
            }
        }
    }

    async fn send(
        &self,
        principal: &str,
        settings: &ImessageSenderSettingsV1,
        request_id: String,
        generation: String,
        destination: String,
        text: String,
    ) -> Result<NotificationResultV1, &'static str> {
        if !settings.enabled {
            return Err("sender_disabled");
        }
        if settings.sender_generation != generation {
            return Err("generation_mismatch");
        }
        if settings.recipient_credential_ref.as_deref() != Some(&destination) {
            return Err("destination_denied");
        }
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        if let Some(row) = self
            .ledger
            .imessage_receipt(principal, &request_id)
            .map_err(|_| "storage_unavailable")?
        {
            if row.sender_generation != generation
                || row.destination_ref != destination
                || row.content_digest != digest
            {
                return Err("request_id_conflict");
            }
            return Ok(receipt(row));
        }
        if !self.platform.supported() {
            return Err("platform_unsupported");
        }
        let _guard = self.serial.try_lock().map_err(|_| "sender_busy")?;
        let current = self
            .repository
            .load_imessage_sender()
            .map_err(|_| "settings_unavailable")?;
        if current.revision != settings.revision {
            return Err("settings_changed");
        }
        let credentials = self.credentials.clone();
        let credential_ref = destination.clone();
        let recipient = tokio::task::spawn_blocking(move || {
            credentials
                .load("com.polarealm.PolarStation.channels", &credential_ref)
                .map(|value| value.map(Zeroizing::new))
        })
        .await
        .map_err(|_| "credential_unavailable")?
        .map_err(|_| "credential_unavailable")?
        .ok_or("credential_missing")?;
        if recipient.is_empty() || recipient.len() > 512 || recipient.chars().any(char::is_control)
        {
            return Err("recipient_invalid");
        }
        let now = now_ms();
        let row = ImessageReceiptRow {
            source_principal: principal.into(),
            request_id: request_id.clone(),
            sender_generation: generation,
            destination_ref: destination,
            content_digest: digest,
            status: "sending".into(),
            error_code: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        if !self
            .ledger
            .claim_imessage(row)
            .map_err(|_| "storage_unavailable")?
        {
            return Err("request_id_conflict");
        }
        let mut claim = SendingClaim {
            ledger: self.ledger.clone(),
            principal: principal.into(),
            request_id: request_id.clone(),
            completed: false,
        };
        // Once claimed, interruption or process failure leaves a durable unknown;
        // no timeout path schedules another platform send.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.platform.send(recipient, Zeroizing::new(text)),
        )
        .await
        .unwrap_or(PlatformSendOutcome::Unknown("platform_timeout"));
        let (status, error_code) = match outcome {
            PlatformSendOutcome::Sent => ("sent_to_messages", None),
            PlatformSendOutcome::Rejected(code) => ("failed", Some(code.into())),
            PlatformSendOutcome::Unknown(code) => ("unknown", Some(code.into())),
        };
        self.ledger
            .finish_imessage(principal, &request_id, status, error_code, now_ms())
            .map_err(|_| "storage_unavailable")?;
        claim.completed = true;
        self.ledger
            .imessage_receipt(principal, &request_id)
            .map_err(|_| "storage_unavailable")?
            .map(receipt)
            .ok_or("receipt_not_found")
    }
}

fn error(code: &str) -> NotificationResultV1 {
    NotificationResultV1::Error { code: code.into() }
}
fn receipt(row: ImessageReceiptRow) -> NotificationResultV1 {
    let status = match row.status.as_str() {
        "sending" => NotificationReceiptStatusV1::Sending,
        "sent_to_messages" => NotificationReceiptStatusV1::SentToMessages,
        "failed" => NotificationReceiptStatusV1::Failed,
        _ => NotificationReceiptStatusV1::Unknown,
    };
    NotificationResultV1::Receipt {
        receipt: NotificationReceiptV1 {
            request_id: row.request_id,
            sender_generation: row.sender_generation,
            status,
            error_code: row.error_code,
        },
    }
}
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}


