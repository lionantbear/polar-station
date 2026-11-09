//! Thin command adapter over the shared local Host client. No credential or DB I/O.
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use polar_station_settings::CodexControlSettingsV1;
use polar_tmux::{
    desktop_client::{DesktopClient, DesktopClientConfig, DesktopClientEvent},
    host_identity::random_uuid_v4,
    local_ipc,
};
use polar_tmux_protocol::*;
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Notify;

#[derive(Debug, Subcommand)]
pub enum ConnectorCommand {
    /// Inspect independent monitor and connector routing settings.
    Status,
    /// List persisted streams on a local connector.
    Streams { connector: String },
    /// Read the durable outcome of a published completion.
    Receipt { connector: String, delivery: String },
    /// Explicitly move replies after checking the current return Host.
    SetReturnRoute {
        stream: String,
        #[arg(long)]
        expected_host: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        endpoint: String,
    },
    /// Apply revisioned connector settings from a JSON file (no credentials).
    Apply {
        #[arg(long)]
        file: PathBuf,
    },
    /// Submit an original completion event through a configured connector.
    Publish {
        #[arg(long)]
        file: PathBuf,
    },
}
pub async fn connector(command: ConnectorCommand) -> Result<()> {
    let host = Host::connect(ApplicationStateApiV1::NotificationV1).await?;
    let command = match command {
        ConnectorCommand::Status => ConnectorCommandV1::GetSettings,
        ConnectorCommand::Streams { connector } => ConnectorCommandV1::GetStreams {
            connector_id: connector,
        },
        ConnectorCommand::Receipt {
            connector,
            delivery,
        } => ConnectorCommandV1::GetReceipt {
            connector_id: connector,
            delivery_id: delivery,
        },
        ConnectorCommand::SetReturnRoute {
            stream,
            expected_host,
            host,
            endpoint,
        } => ConnectorCommandV1::SetReturnRoute {
            stream_id: stream,
            expected_host_id: expected_host,
            host_id: host,
            endpoint_id: endpoint,
        },
        ConnectorCommand::Apply { file } => {
            let settings: polar_station_settings::MessageConnectorSettingsV1 =
                serde_json::from_slice(&std::fs::read(file)?)?;
            settings.validate()?;
            ConnectorCommandV1::UpdateSettings {
                expected_revision: settings.revision,
                settings,
            }
        }
        ConnectorCommand::Publish { file } => {
            let publication: ConnectorPublicationV1 =
                serde_json::from_slice(&std::fs::read(file)?)?;
            publication.validate()?;
            ConnectorCommandV1::Publish { publication }
        }
    };
    let result = host
        .notification(NotificationCommandV1::Connector {
            command: Box::new(command),
        })
        .await?;
    if let NotificationResultV1::Connector { result } = result {
        if let ConnectorResultV1::Error { code } = result.as_ref() {
            bail!("Connector: {code}");
        }
        print(serde_json::to_value(result)?)
    } else {
        bail!("Unexpected connector response")
    }
}

#[derive(Debug, Subcommand)]
pub enum TelegramCommand {
    /// Import the current bot and chat through the Host secure store. Receiving stays off.
    Setup {
        #[arg(long, required = true)]
        from_current: bool,
        /// Required for group chats. Private chats use the verified chat identity.
        #[arg(long)]
        allowed_sender: Option<i64>,
    },
    Enable,
    Disable,
    Status,
}

#[derive(Debug, Subcommand)]
pub enum CodexCommand {
    /// Show Host sessions and conversations loaded in the original daemon.
    List {
        #[arg(long, default_value = "local-codex")]
        endpoint: String,
    },
    /// Explicitly associate a tmux Session with its original Codex thread.
    Bind {
        #[arg(long)]
        session: String,
        #[arg(long)]
        thread: String,
        #[arg(long, default_value = "local")]
        target: String,
        #[arg(long, default_value = "local-codex")]
        endpoint: String,
        #[arg(long, value_parser = ["current"])]
        telegram: Option<String>,
    },
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ImessageCommand {
    /// Import the current Mac recipient and export notification-only pairing.
    Setup {
        #[arg(long, required = true)]
        from_current: bool,
        #[arg(long)]
        source_host: String,
        #[arg(long)]
        pairing_file: PathBuf,
    },
    /// Explicitly request local Messages Automation permission.
    Authorize,
    Status,
    /// Admit an existing Mac sender through a configured SSH target.
    Connect {
        #[arg(long)]
        ssh_target: String,
        #[arg(long)]
        pairing_file: PathBuf,
        #[arg(long, default_value_t = 47820)]
        port: u16,
    },
    /// Enable the local Mac sender, or iMessage for one bound local Session.
    Enable {
        #[arg(long)]
        session: Option<String>,
    },
    Disable {
        #[arg(long)]
        session: Option<String>,
    },
}

struct Host {
    client: DesktopClient,
    wake: Arc<Notify>,
    owner: String,
}
impl Host {
    async fn connect(api: ApplicationStateApiV1) -> Result<Self> {
        let wake = Arc::new(Notify::new());
        let signal = wake.clone();
        let mut client = DesktopClient::new(
            DesktopClientConfig {
                socket_path: local_ipc::default_socket_path()?,
                registration: LocalClientFrame::Register {
                    frontend_id: format!("station-cli-{}", random_uuid_v4()),
                    platform: HostPlatform::current(),
                    // Registration requires an action. Without RefreshSnapshotV1 this
                    // connection cannot be selected as a desktop rendering target.
                    supported_actions: vec![FrontendActionName::Refresh],
                    capabilities: vec![FrontendCapability::SessionFeedV1],
                    frontend_instance_id: None,
                    frontend_generation: 0,
                    extensions: Vec::new(),
                    host_control_versions: Some(HostControlVersionsV1 {
                        desktop: None,
                        codex: Some(CODEX_CONTROL_API_VERSION),
                        notification: Some(NOTIFICATION_API_VERSION),
                    }),
                },
                event_capacity: 256,
            },
            move || signal.notify_one(),
        )?;
        client.start()?;
        let mut host = Self {
            client,
            wake,
            owner: String::new(),
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match host.event().await? {
                    DesktopClientEvent::Ready { host_id, host_control_versions, .. } => {
                        if host_control_versions.is_none_or(|v| !v.permits(api)) {
                            bail!("Host does not support this control API; install the current Host first");
                        }
                        host.owner = host_id;
                        return Ok(host);
                    }
                    _ => continue,
                }
            }
        }).await.context("Host connection timed out")?
    }

    async fn event(&self) -> Result<DesktopClientEvent> {
        loop {
            while let Some(event) = self.client.pop_event() {
                match event {
                    DesktopClientEvent::Unavailable { .. } => bail!("Host disconnected; an in-flight write may have succeeded. Check status before retrying"),
                    DesktopClientEvent::Frame { ref frame, .. } if matches!(**frame, LocalHostFrame::Ping { .. }) => {
                        if let LocalHostFrame::Ping { nonce } = **frame {
                            self.client.try_send(LocalClientFrame::Pong { nonce })?;
                        }
                    }
                    event => return Ok(event),
                }
            }
            self.wake.notified().await;
        }
    }

    async fn request(&self, command: CodexCommandV1) -> Result<CodexResultV1> {
        let request_id = random_uuid_v4();
        self.client.try_send(LocalClientFrame::ApplicationRequest {
            request: ClientApplicationMessage::CodexV1(Box::new(CodexRequestEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: CodexApiV1::CodexV1,
                request_id: request_id.clone(),
                owner_host_id: self.owner.clone(),
                command,
            })),
        })?;
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let DesktopClientEvent::Frame { frame, .. } = self.event().await? {
                    if let LocalHostFrame::ApplicationResponse {
                        response: HostApplicationMessage::CodexV1(response),
                    } = *frame
                    {
                        if response.request_id == request_id && response.owner_host_id == self.owner
                        {
                            if let CodexResultV1::Error { code } = response.result {
                                bail!("Host: {code}");
                            }
                            return Ok(response.result);
                        }
                    }
                }
            }
        })
        .await
        .context("Host response timed out; check status before retrying a write")?
    }

    async fn settings(&self) -> Result<CodexControlSettingsV1> {
        match self.request(CodexCommandV1::GetSettings).await? {
            CodexResultV1::Settings { settings } => Ok(settings),
            _ => bail!("Unexpected Host settings response"),
        }
    }

    async fn notification(&self, command: NotificationCommandV1) -> Result<NotificationResultV1> {
        let request_id = random_uuid_v4();
        self.client.try_send(LocalClientFrame::ApplicationRequest {
            request: ClientApplicationMessage::NotificationV1(Box::new(
                NotificationRequestEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: NotificationApiV1::NotificationV1,
                    request_id: request_id.clone(),
                    command,
                },
            )),
        })?;
        tokio::time::timeout(Duration::from_secs(130), async {
            loop {
                if let DesktopClientEvent::Frame { frame, .. } = self.event().await? {
                    if let LocalHostFrame::ApplicationResponse {
                        response: HostApplicationMessage::NotificationV1(response),
                    } = *frame
                    {
                        if response.request_id == request_id {
                            if let NotificationResultV1::Error { code } = response.result {
                                bail!("Host: {code}");
                            }
                            return Ok(response.result);
                        }
                    }
                }
            }
        })
        .await
        .context("Host response timed out; inspect sender state before retrying")?
    }

    async fn sessions(&self) -> Result<Vec<SessionProjectionV1>> {
        self.client
            .try_send(LocalClientFrame::SubscribeSessionFeed {
                known_epoch: None,
                after_revision: None,
            })?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let DesktopClientEvent::Frame { frame, .. } = self.event().await? {
                    if let LocalHostFrame::SessionFeedSnapshot { snapshot } = *frame {
                        return Ok(snapshot.sessions);
                    }
                }
            }
        })
        .await
        .context("Host session snapshot timed out")?
    }
}

pub async fn telegram(command: TelegramCommand) -> Result<()> {
    let host = Host::connect(ApplicationStateApiV1::CodexV1).await?;
    let settings = host.settings().await?;
    let result = match command {
        TelegramCommand::Setup {
            from_current: _,
            allowed_sender,
        } => {
            host.request(CodexCommandV1::SetupCurrentTelegram {
                expected_revision: settings.revision,
                allowed_sender_id: allowed_sender,
            })
            .await?
        }
        TelegramCommand::Enable => {
            host.request(CodexCommandV1::SetTelegramEnabled {
                expected_revision: settings.revision,
                enabled: true,
            })
            .await?
        }
        TelegramCommand::Disable => {
            host.request(CodexCommandV1::SetTelegramEnabled {
                expected_revision: settings.revision,
                enabled: false,
            })
            .await?
        }
        TelegramCommand::Status => host.request(CodexCommandV1::GetTelegramStatus).await?,
    };
    // GetSettings includes routing identities. CLI output intentionally exposes
    // only non-sensitive configuration facts, not chat IDs or credential refs.
    let value = match result {
        CodexResultV1::Settings { settings } => {
            json!({"revision":settings.revision,"control_enabled":settings.enabled,"telegram_enabled":settings.telegram.enabled,"configured":settings.telegram.bot_credential_ref.is_some(),"endpoints":settings.endpoints.iter().map(|e| &e.id).collect::<Vec<_>>()})
        }
        CodexResultV1::TelegramStatus { status } => {
            json!({"configured":settings.telegram.bot_credential_ref.is_some(),"enabled":settings.telegram.enabled,"status":status})
        }
        _ => bail!("Unexpected Host channel response"),
    };
    print(value)
}

pub async fn codex(command: CodexCommand) -> Result<()> {
    let host = Host::connect(ApplicationStateApiV1::CodexV1).await?;
    let value = match command {
        CodexCommand::List { endpoint } => {
            let sessions = host.sessions().await?;
            let threads = host
                .request(CodexCommandV1::ListLoaded {
                    endpoint_id: endpoint,
                })
                .await?;
            json!({"sessions":sessions.iter().map(|s| json!({"key":s.key,"name":s.display_name})).collect::<Vec<_>>(),"conversations":threads})
        }
        CodexCommand::Bind {
            session,
            thread,
            target,
            endpoint,
            telegram,
        } => {
            let sessions = host.sessions().await?;
            let key = select_session(&sessions, &target, &session)?;
            let result = host
                .request(CodexCommandV1::Bind {
                    session: key,
                    endpoint_id: endpoint,
                    thread_id: thread,
                })
                .await?;
            let result = if let Some(destination) = telegram {
                let CodexResultV1::Binding {
                    binding: Some(state),
                } = result
                else {
                    bail!("Host did not return the new binding");
                };
                host.request(CodexCommandV1::UpdateNotifications {
                    binding_id: state.binding.binding_id,
                    expected_revision: state.binding.revision,
                    telegram_destination_ref: Some(destination),
                    imessage_enabled: state.binding.imessage_enabled,
                })
                .await?
            } else {
                result
            };
            serde_json::to_value(result)?
        }
        CodexCommand::Status => {
            serde_json::to_value(host.request(CodexCommandV1::GetBindings).await?)?
        }
    };
    print(value)
}

fn select_session(
    sessions: &[SessionProjectionV1],
    target: &str,
    selected: &str,
) -> Result<SessionKeyV1> {
    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| {
            s.key.execution_target.as_str() == target
                && (s.key.tmux_session_id == selected || s.display_name == selected)
        })
        .collect();
    if matches.len() != 1 {
        bail!("Session must match exactly one Host Session ID or name; use codex list");
    }
    Ok(matches[0].key.clone())
}

pub async fn imessage(command: ImessageCommand) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let api = match &command {
        ImessageCommand::Connect { .. }
        | ImessageCommand::Enable { session: Some(_) }
        | ImessageCommand::Disable { session: Some(_) } => ApplicationStateApiV1::CodexV1,
        _ => ApplicationStateApiV1::NotificationV1,
    };
    let host = Host::connect(api).await?;
    match command {
        ImessageCommand::Setup {
            from_current: _,
            source_host,
            pairing_file,
        } => {
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pairing_file)
                .context("pairing output must be a new private file")?;
            let result = async {
                let NotificationResultV1::Settings { settings } = host.notification(NotificationCommandV1::GetSenderSettings).await? else { bail!("Unexpected sender response") };
                let NotificationResultV1::Pairing { pairing } = host.notification(NotificationCommandV1::SetupCurrentSender { expected_revision: settings.revision, source_host_id: source_host }).await? else { bail!("Unexpected pairing response") };
                let encoded = zeroize::Zeroizing::new(serde_json::to_vec_pretty(&pairing)?);
                output.write_all(&encoded)?; output.sync_all()?;
                print(json!({"configured":true,"pairing_file_written":true,"sender_host_id":pairing.sender_host_id}))
            }.await;
            if result.is_err() {
                drop(output);
                let _ = std::fs::remove_file(pairing_file);
            }
            result
        }
        ImessageCommand::Authorize | ImessageCommand::Status => {
            let request = if matches!(command, ImessageCommand::Authorize) {
                NotificationCommandV1::RequestMessagesAuthorization
            } else {
                NotificationCommandV1::GetSenderStatus
            };
            print(serde_json::to_value(host.notification(request).await?)?)
        }
        ImessageCommand::Connect {
            ssh_target,
            pairing_file,
            port,
        } => {
            let metadata = std::fs::symlink_metadata(&pairing_file)?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
                || metadata.len() > NOTIFICATION_MAX_ENVELOPE_BYTES as u64
            {
                bail!("pairing file must be a bounded, owner-only regular file");
            }
            let bytes = zeroize::Zeroizing::new(std::fs::read(pairing_file)?);
            let pairing = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid pairing file"))?;
            let settings = host.settings().await?;
            let result = host
                .request(CodexCommandV1::ConnectImessageSender {
                    expected_revision: settings.revision,
                    ssh_target_id: ssh_target,
                    port,
                    pairing,
                })
                .await?;
            let CodexResultV1::Settings { settings } = result else {
                bail!("Unexpected Host connection response")
            };
            print(
                json!({"revision":settings.revision,"imessage_enabled":settings.imessage_enabled,"sender_configured":settings.imessage_sender.is_some()}),
            )
        }
        command @ (ImessageCommand::Enable { .. } | ImessageCommand::Disable { .. }) => {
            let enabled = matches!(command, ImessageCommand::Enable { .. });
            let session = match command {
                ImessageCommand::Enable { session } | ImessageCommand::Disable { session } => {
                    session
                }
                _ => unreachable!(),
            };
            if let Some(selected) = session {
                let sessions = host.sessions().await?;
                let key = select_session(&sessions, "local", &selected)?;
                let CodexResultV1::Bindings { bindings } =
                    host.request(CodexCommandV1::GetBindings).await?
                else {
                    bail!("Unexpected bindings response")
                };
                let state = bindings
                    .iter()
                    .find(|s| s.binding.session.as_ref() == Some(&key))
                    .context("Session has no Codex binding")?;
                let result = host
                    .request(CodexCommandV1::UpdateNotifications {
                        binding_id: state.binding.binding_id.clone(),
                        expected_revision: state.binding.revision,
                        telegram_destination_ref: state.binding.telegram_destination_ref.clone(),
                        imessage_enabled: enabled,
                    })
                    .await?;
                print(serde_json::to_value(result)?)
            } else {
                let NotificationResultV1::Settings { mut settings } = host
                    .notification(NotificationCommandV1::GetSenderSettings)
                    .await?
                else {
                    bail!("Unexpected sender response")
                };
                settings.enabled = enabled;
                let result = host
                    .notification(NotificationCommandV1::UpdateSenderSettings {
                        expected_revision: settings.revision,
                        settings,
                    })
                    .await?;
                let NotificationResultV1::Settings { settings } = result else {
                    bail!("Unexpected sender update response")
                };
                print(json!({"revision":settings.revision,"enabled":settings.enabled}))
            }
        }
    }
}

fn print(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
