use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures_core::future::BoxFuture;
use grammers_client::media::Media;
use grammers_client::message::InputMessage;
use grammers_client::peer::Peer;
use grammers_client::sender::UpdatesConfiguration;
use grammers_client::update::Update;
use grammers_client::{Client, SenderPool};
use grammers_session::types::{
    ChannelState, DcOption, PeerId, PeerInfo, PeerRef, UpdateState, UpdatesState,
};
use grammers_session::{Session, SessionData};
use grammers_tl_types as tl;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::client::{ResolvedTelegramRuntimeConfig, TelegramBotRuntimeView, TelegramClient};
use crate::runtime::{
    TelegramAttachmentPayload, TelegramDownloadedFile, TelegramInboundAttachment,
    TelegramInboundEnvelope, TelegramInboundMessage, TelegramOutboundDocument,
    TelegramOutboundMessage, TelegramOutboundPhoto, TelegramSentMessage, TelegramTypingAction,
};

#[derive(Clone)]
struct CachedDownloadable {
    media: Media,
    filename: Option<String>,
    mime_type: Option<String>,
}

struct GrammersBotRuntime {
    bot_name: String,
    client: Client,
    handle: grammers_client::sender::SenderPoolFatHandle,
    runner_task: JoinHandle<()>,
    updates_task: JoinHandle<()>,
    runtime: TelegramBotRuntimeView,
}

pub struct GrammersTelegramClient {
    upload_directory: PathBuf,
    bots: Vec<GrammersBotRuntime>,
    inbound_rx: mpsc::UnboundedReceiver<Result<TelegramInboundEnvelope>>,
    media_cache: Arc<Mutex<HashMap<String, CachedDownloadable>>>,
}

impl GrammersTelegramClient {
    pub(crate) async fn from_env(config: &ResolvedTelegramRuntimeConfig) -> Result<Self> {
        Self::connect_all(config).await
    }

    async fn connect_all(config: &ResolvedTelegramRuntimeConfig) -> Result<Self> {
        let api_id = config
            .api_id
            .as_deref()
            .ok_or_else(|| anyhow!("telegram api_id is not configured"))?
            .parse::<i32>()
            .context("parse telegram api_id")?;
        let api_hash = config
            .api_hash
            .as_deref()
            .ok_or_else(|| anyhow!("telegram api_hash is not configured"))?;
        if config.bots.is_empty() {
            return Err(anyhow!("telegram bots are not configured"));
        }

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let media_cache = Arc::new(Mutex::new(HashMap::new()));
        let mut bots = Vec::with_capacity(config.bots.len());
        let session_dir = session_dir(config)?;
        let upload_directory = config
            .upload_directory
            .clone()
            .context("grammers requires an explicit managed upload directory")?;
        for bot in &config.bots {
            let token = bot.bot_token.as_deref().ok_or_else(|| {
                anyhow!("telegram bot token is not configured for {}", bot.bot_name)
            })?;
            bots.push(
                Self::connect_bot(
                    api_id,
                    api_hash,
                    &bot.bot_name,
                    token,
                    session_file_path(&session_dir, &bot.bot_name)?,
                    inbound_tx.clone(),
                    Arc::clone(&media_cache),
                )
                .await?,
            );
        }

        Ok(Self {
            upload_directory,
            bots,
            inbound_rx,
            media_cache,
        })
    }

    async fn connect_bot(
        api_id: i32,
        api_hash: &str,
        bot_name: &str,
        token: &str,
        session_file: PathBuf,
        inbound_tx: mpsc::UnboundedSender<Result<TelegramInboundEnvelope>>,
        media_cache: Arc<Mutex<HashMap<String, CachedDownloadable>>>,
    ) -> Result<GrammersBotRuntime> {
        let (client, handle, runner_task, updates) =
            Self::connect_authorized_bot(api_id, api_hash, bot_name, token, &session_file).await?;

        let mut updates = match client
            .stream_updates(updates, inbound_updates_configuration())
            .await
        {
            Ok(updates) => updates,
            Err(error) => {
                runner_task.abort();
                return Err(anyhow!("initialize Telegram updates: {error}"));
            }
        };
        let bot_name_string = bot_name.to_string();
        let updates_task = tokio::spawn(async move {
            loop {
                let update = match updates.next().await {
                    Ok(update) => update,
                    Err(err) => {
                        let _ = inbound_tx.send(Err(anyhow!(
                            "read grammers update for bot {}: {err:#}",
                            bot_name_string
                        )));
                        break;
                    }
                };
                match envelope_from_update(&bot_name_string, update, &media_cache) {
                    Ok(Some(envelope)) => {
                        if inbound_tx.send(Ok(envelope)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = inbound_tx.send(Err(err));
                        break;
                    }
                }
            }
        });

        let me = client
            .get_me()
            .await
            .with_context(|| format!("grammers get_me failed for {bot_name}"))?;
        let session_present = tokio::fs::metadata(&session_file).await.is_ok();
        let runtime = TelegramBotRuntimeView {
            bot_name: bot_name.to_string(),
            telegram_user_id: Some(me.id().bare_id_unchecked()),
            telegram_name: {
                let name = me.full_name();
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            },
            telegram_username: me.username().map(str::to_owned),
            authorized: true,
            session_path: Some(session_file.display().to_string()),
            session_present,
        };

        Ok(GrammersBotRuntime {
            bot_name: bot_name.to_string(),
            client,
            handle,
            runner_task,
            updates_task,
            runtime,
        })
    }

    async fn connect_authorized_bot(
        api_id: i32,
        api_hash: &str,
        bot_name: &str,
        token: &str,
        session_file: &Path,
    ) -> Result<(
        Client,
        grammers_client::sender::SenderPoolFatHandle,
        JoinHandle<()>,
        tokio::sync::mpsc::Receiver<grammers_client::session::updates::UpdatesLike>,
    )> {
        let (session, client, handle, runner_task, updates) =
            open_grammers_client(api_id, session_file, bot_name).await?;
        let authorized = client
            .is_authorized()
            .await
            .context("check grammers authorization")?;
        if authorized && session_matches_expected_bot(&client, token).await? {
            return Ok((client, handle, runner_task, updates));
        }

        handle.quit();
        runner_task.abort();
        drop(updates);
        drop(client);
        drop(session);

        remove_session_file(session_file).await?;

        let (_session, client, handle, runner_task, updates) =
            open_grammers_client(api_id, session_file, bot_name).await?;
        client
            .bot_sign_in(token, api_hash)
            .await
            .with_context(|| format!("grammers bot sign-in failed for {bot_name}"))?;
        Ok((client, handle, runner_task, updates))
    }
}

#[async_trait]
impl TelegramClient for GrammersTelegramClient {
    fn is_realtime(&self) -> bool {
        true
    }

    fn runtime_bots(&self) -> Vec<TelegramBotRuntimeView> {
        self.bots.iter().map(|bot| bot.runtime.clone()).collect()
    }

    async fn poll_updates(&mut self) -> Result<Vec<TelegramInboundEnvelope>> {
        let mut inbound = Vec::new();
        loop {
            match self.inbound_rx.try_recv() {
                Ok(Ok(envelope)) => inbound.push(envelope),
                Ok(Err(err)) => return Err(err),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return Err(anyhow!("grammers inbound channel disconnected"));
                }
            }
        }
        Ok(inbound)
    }

    async fn recv_update(&mut self) -> Result<Option<TelegramInboundEnvelope>> {
        match self.inbound_rx.recv().await {
            Some(Ok(envelope)) => Ok(Some(envelope)),
            Some(Err(err)) => Err(err),
            None => Err(anyhow!("grammers inbound channel disconnected")),
        }
    }

    async fn send_message(
        &mut self,
        message: TelegramOutboundMessage,
    ) -> Result<TelegramSentMessage> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == message.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", message.bot_name))?;
        let peer_id =
            grammers_client::session::types::PeerId::from_bot_api_dialog_id(message.chat_id)
                .ok_or_else(|| anyhow!("invalid telegram chat id {}", message.chat_id))?;
        let sent = bot
            .client
            .send_message(peer_id.to_ambient_ref(), message.text)
            .await
            .with_context(|| format!("grammers send_message failed for {}", bot.bot_name))?;
        Ok(TelegramSentMessage {
            bot_name: message.bot_name,
            chat_id: message.chat_id,
            message_id: sent.id(),
        })
    }

    async fn edit_message(&mut self, message: &TelegramSentMessage, text: String) -> Result<()> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == message.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", message.bot_name))?;
        let peer_id = PeerId::from_bot_api_dialog_id(message.chat_id)
            .ok_or_else(|| anyhow!("invalid telegram chat id {}", message.chat_id))?;
        bot.client
            .edit_message(peer_id.to_ambient_ref(), message.message_id, text)
            .await
            .with_context(|| format!("grammers edit_message failed for {}", bot.bot_name))?;
        Ok(())
    }

    async fn delete_message(&mut self, message: &TelegramSentMessage) -> Result<()> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == message.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", message.bot_name))?;
        let peer_id = PeerId::from_bot_api_dialog_id(message.chat_id)
            .ok_or_else(|| anyhow!("invalid telegram chat id {}", message.chat_id))?;
        bot.client
            .delete_messages(peer_id.to_ambient_ref(), &[message.message_id])
            .await
            .with_context(|| format!("grammers delete_message failed for {}", bot.bot_name))?;
        Ok(())
    }

    async fn send_typing(&mut self, action: TelegramTypingAction) -> Result<()> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == action.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", action.bot_name))?;
        let peer_id = PeerId::from_bot_api_dialog_id(action.chat_id)
            .ok_or_else(|| anyhow!("invalid telegram chat id {}", action.chat_id))?;
        let peer: PeerRef = peer_id.to_ambient_ref();
        bot.client
            .action(peer)
            .oneshot(tl::enums::SendMessageAction::SendMessageTypingAction)
            .await
            .with_context(|| format!("grammers send_typing failed for {}", bot.bot_name))?;
        Ok(())
    }

    async fn send_photo(&mut self, photo: TelegramOutboundPhoto) -> Result<TelegramSentMessage> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == photo.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", photo.bot_name))?;
        let peer_id = PeerId::from_bot_api_dialog_id(photo.chat_id)
            .ok_or_else(|| anyhow!("invalid telegram chat id {}", photo.chat_id))?;
        let peer: PeerRef = peer_id.to_ambient_ref();
        let upload_path = materialize_payload_for_upload(&photo.payload, &self.upload_directory)?;
        let uploaded = bot
            .client
            .upload_file(upload_path.path())
            .await
            .with_context(|| format!("grammers upload photo failed for {}", bot.bot_name))?;
        let sent = bot
            .client
            .send_message(
                peer,
                InputMessage::new()
                    .text(photo.caption.unwrap_or_default())
                    .photo(uploaded),
            )
            .await
            .with_context(|| format!("grammers send_photo failed for {}", bot.bot_name))?;
        Ok(TelegramSentMessage {
            bot_name: photo.bot_name,
            chat_id: photo.chat_id,
            message_id: sent.id(),
        })
    }

    async fn send_document(
        &mut self,
        document: TelegramOutboundDocument,
    ) -> Result<TelegramSentMessage> {
        let bot = self
            .bots
            .iter_mut()
            .find(|bot| bot.bot_name == document.bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {}", document.bot_name))?;
        let peer_id = PeerId::from_bot_api_dialog_id(document.chat_id)
            .ok_or_else(|| anyhow!("invalid telegram chat id {}", document.chat_id))?;
        let peer: PeerRef = peer_id.to_ambient_ref();
        let upload_path =
            materialize_payload_for_upload(&document.payload, &self.upload_directory)?;
        let mut stream = tokio::fs::File::open(upload_path.path()).await?;
        let size = usize::try_from(stream.metadata().await?.len())?;
        let upload_name = document
            .filename
            .clone()
            .unwrap_or_else(|| "document.bin".to_string());
        let uploaded = bot
            .client
            .upload_stream(&mut stream, size, upload_name)
            .await
            .with_context(|| format!("grammers upload document failed for {}", bot.bot_name))?;
        let sent = bot
            .client
            .send_message(
                peer,
                InputMessage::new()
                    .text(document.caption.unwrap_or_default())
                    .document(uploaded),
            )
            .await
            .with_context(|| format!("grammers send_document failed for {}", bot.bot_name))?;
        Ok(TelegramSentMessage {
            bot_name: document.bot_name,
            chat_id: document.chat_id,
            message_id: sent.id(),
        })
    }

    async fn download_file(
        &mut self,
        bot_name: &str,
        file_id: &str,
    ) -> Result<TelegramDownloadedFile> {
        let cached = self
            .media_cache
            .lock()
            .expect("grammers media cache lock")
            .get(file_id)
            .cloned()
            .ok_or_else(|| {
                anyhow!("grammers cached attachment not found for {bot_name}:{file_id}")
            })?;
        let bot = self
            .bots
            .iter()
            .find(|bot| bot.bot_name == bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {bot_name}"))?;
        let mut download = bot.client.iter_download(&cached.media);
        let mut bytes = Vec::new();
        while let Some(chunk) = download
            .next()
            .await
            .with_context(|| format!("grammers download chunk failed for {bot_name}:{file_id}"))?
        {
            bytes.extend(chunk);
        }
        Ok(TelegramDownloadedFile {
            filename: cached.filename,
            mime_type: cached.mime_type,
            bytes,
        })
    }
}

impl Drop for GrammersTelegramClient {
    fn drop(&mut self) {
        for bot in &mut self.bots {
            bot.handle.quit();
            bot.updates_task.abort();
            bot.runner_task.abort();
        }
    }
}

fn envelope_from_update(
    bot_name: &str,
    update: Update,
    media_cache: &Arc<Mutex<HashMap<String, CachedDownloadable>>>,
) -> Result<Option<TelegramInboundEnvelope>> {
    let Update::NewMessage(message) = update else {
        return Ok(None);
    };
    if message.outgoing() {
        return Ok(None);
    }
    let text = message.text().to_string();
    let attachments = inbound_attachments_from_message(bot_name, &message, media_cache);
    if text.is_empty() && attachments.is_empty() {
        return Ok(None);
    }
    let chat_id = message
        .peer_id()
        .bot_api_dialog_id()
        .ok_or_else(|| anyhow!("grammers update missing bot_api_dialog_id"))?;
    let remote_user_id = message
        .sender_id()
        .and_then(|sender_id| sender_id.bot_api_dialog_id());
    let (remote_display_name, remote_username) = match message.sender() {
        Some(sender) => sender_identity(sender),
        None => (None, None),
    };
    Ok(Some(TelegramInboundEnvelope {
        update_id: message.id() as u64,
        message: TelegramInboundMessage {
            metadata: crate::TelegramMessageMetadata {
                message_id: Some(message.id()),
                reply_to_message_id: message.reply_to_message_id(),
                reply_to_message_thread_id: None,
                message_thread_id: match message.reply_header() {
                    Some(grammers_tl_types::enums::MessageReplyHeader::Header(header))
                        if header.forum_topic =>
                    {
                        header
                            .reply_to_top_id
                            .or(header.reply_to_msg_id)
                            .map(i64::from)
                    }
                    _ => None,
                },
                chat_type: message.peer().map(|peer| {
                    match peer {
                        Peer::User(_) => "private",
                        Peer::Group(_) => "group",
                        Peer::Channel(_) => "channel",
                        Peer::Community(_) => "community",
                    }
                    .to_owned()
                }),
                forwarded: message.forward_header().is_some(),
                edited: message.edit_date().is_some(),
            },
            bot_name: bot_name.to_string(),
            chat_id,
            remote_user_id,
            remote_display_name,
            remote_username,
            text,
            attachments,
        },
    }))
}

fn sender_identity(sender: &Peer) -> (Option<String>, Option<String>) {
    let display_name = match sender {
        Peer::User(user) => {
            let full_name = user.full_name();
            if full_name.trim().is_empty() {
                sender.name().map(str::to_owned)
            } else {
                Some(full_name)
            }
        }
        _ => sender.name().map(str::to_owned),
    };
    let username = sender
        .username()
        .map(str::to_owned)
        .or_else(|| sender.usernames().into_iter().next().map(str::to_owned));
    (display_name, username)
}

fn inbound_attachments_from_message(
    bot_name: &str,
    message: &grammers_client::update::Message,
    media_cache: &Arc<Mutex<HashMap<String, CachedDownloadable>>>,
) -> Vec<TelegramInboundAttachment> {
    let Some(media) = message.media() else {
        return Vec::new();
    };
    let cache_key = format!("{bot_name}/{}", message.id());
    let mut cache = media_cache.lock().expect("grammers media cache lock");
    match media {
        Media::Photo(photo) => {
            let file_id = format!("{cache_key}/photo");
            cache.insert(
                file_id.clone(),
                CachedDownloadable {
                    media: Media::Photo(photo.clone()),
                    filename: Some(format!("telegram-{}.jpg", message.id())),
                    mime_type: Some("image/jpeg".to_string()),
                },
            );
            vec![TelegramInboundAttachment::Photo {
                file_id,
                file_unique_id: None,
            }]
        }
        Media::Document(document) => {
            let file_id = format!("{cache_key}/document");
            cache.insert(
                file_id.clone(),
                CachedDownloadable {
                    media: Media::Document(document.clone()),
                    filename: document.name().map(str::to_owned),
                    mime_type: document.mime_type().map(str::to_owned),
                },
            );
            vec![TelegramInboundAttachment::Document {
                file_id,
                file_unique_id: None,
                file_name: document.name().map(str::to_owned),
                mime_type: document.mime_type().map(str::to_owned),
            }]
        }
        Media::Sticker(sticker) => {
            let file_id = format!("{cache_key}/sticker");
            cache.insert(
                file_id.clone(),
                CachedDownloadable {
                    media: Media::Sticker(sticker.clone()),
                    filename: sticker.document.name().map(str::to_owned),
                    mime_type: sticker.document.mime_type().map(str::to_owned),
                },
            );
            vec![TelegramInboundAttachment::Document {
                file_id,
                file_unique_id: None,
                file_name: sticker.document.name().map(str::to_owned),
                mime_type: sticker.document.mime_type().map(str::to_owned),
            }]
        }
        _ => Vec::new(),
    }
}

enum UploadPayload {
    Borrowed(PathBuf),
    Temporary(tempfile::NamedTempFile),
}

impl UploadPayload {
    fn path(&self) -> &Path {
        match self {
            Self::Borrowed(path) => path,
            Self::Temporary(file) => file.path(),
        }
    }
}

fn materialize_payload_for_upload(
    payload: &TelegramAttachmentPayload,
    directory: &Path,
) -> Result<UploadPayload> {
    match payload {
        TelegramAttachmentPayload::FilePath(path) => Ok(UploadPayload::Borrowed(path.clone())),
        TelegramAttachmentPayload::Bytes(bytes) => {
            use std::io::Write;
            anyhow::ensure!(directory.is_absolute(), "upload directory must be absolute");
            let mut file = tempfile::Builder::new()
                .prefix("upload-")
                .tempfile_in(directory)?;
            file.write_all(bytes)?;
            file.flush()?;
            Ok(UploadPayload::Temporary(file))
        }
    }
}



fn session_dir(config: &ResolvedTelegramRuntimeConfig) -> Result<PathBuf> {
    let session_dir = config
        .session_dir
        .as_deref()
        .ok_or_else(|| anyhow!("telegram session_dir is required for grammers provider"))?;
    Ok(PathBuf::from(session_dir))
}

fn session_file_path(session_dir: &Path, bot_name: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !bot_name.is_empty()
            && bot_name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "invalid bot name for session file"
    );
    Ok(session_dir.join(format!("{bot_name}.session.json")))
}

async fn open_grammers_client(
    api_id: i32,
    session_file: &Path,
    bot_name: &str,
) -> Result<(
    Arc<PersistedMemorySession>,
    Client,
    grammers_client::sender::SenderPoolFatHandle,
    JoinHandle<()>,
    tokio::sync::mpsc::Receiver<grammers_client::session::updates::UpdatesLike>,
)> {
    let session = Arc::new(
        PersistedMemorySession::open(session_file)
            .await
            .with_context(|| format!("open grammers session for {bot_name}"))?,
    );
    let SenderPool {
        runner,
        updates,
        handle,
    } = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(handle.clone());
    let runner_task = tokio::spawn(async move {
        let _ = runner.run().await;
    });
    Ok((session, client, handle, runner_task, updates))
}

async fn session_matches_expected_bot(client: &Client, token: &str) -> Result<bool> {
    let Some(expected_user_id) = telegram_bot_token_user_id(token) else {
        return Ok(true);
    };
    let actual_user_id = client
        .get_me()
        .await
        .context("grammers get_me failed during session/token validation")?
        .id()
        .bare_id_unchecked();
    Ok(actual_user_id == expected_user_id)
}

fn telegram_bot_token_user_id(token: &str) -> Option<i64> {
    token.split_once(':')?.0.parse().ok()
}

async fn remove_session_file(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("remove stale grammers session {}", path.display()))
        }
    }
}

fn inbound_updates_configuration() -> UpdatesConfiguration {
    UpdatesConfiguration {
        // Bot restarts must not replay historical updates. Replayed backlog can
        // retrigger old guide-thread prompts and spam Telegram chats.
        catch_up: false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSessionData {
    home_dc: i32,
    dc_options: HashMap<i32, DcOption>,
    peer_infos: HashMap<PeerId, PeerInfo>,
    updates_state: UpdatesState,
}

impl Default for PersistedSessionData {
    fn default() -> Self {
        let session = SessionData::default();
        Self {
            home_dc: session.home_dc,
            dc_options: session.dc_options,
            peer_infos: session.peer_infos,
            updates_state: session.updates_state,
        }
    }
}

struct PersistedMemorySession {
    path: PathBuf,
    data: Mutex<PersistedSessionData>,
}

impl PersistedMemorySession {
    async fn open(path: &Path) -> Result<Self> {
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "grammers session must be a regular file"
            );
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create grammers session dir {}", parent.display()))?;
        }
        let data = match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice::<PersistedSessionData>(&bytes)
                .with_context(|| format!("decode grammers session {}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                PersistedSessionData::default()
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("read grammers session {}", path.display()))
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            data: Mutex::new(data),
        })
    }

    fn persist_snapshot(
        &self,
        snapshot: PersistedSessionData,
    ) -> BoxFuture<'_, std::io::Result<()>> {
        let path = self.path.clone();
        Box::pin(async move {
            use std::io::Write;
            let bytes = serde_json::to_vec(&snapshot)?;
            let parent = path
                .parent()
                .ok_or_else(|| std::io::Error::other("session path has no parent"))?;
            let mut file = tempfile::Builder::new()
                .prefix("session-")
                .tempfile_in(parent)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path).map_err(|error| error.error)?;
            Ok(())
        })
    }
}

impl Session for PersistedMemorySession {
    type Error = std::io::Error;
    fn home_dc_id(&self) -> std::io::Result<i32> {
        Ok(self.data.lock().unwrap().home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, std::io::Result<()>> {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            data.home_dc = dc_id;
            data.clone()
        };
        self.persist_snapshot(snapshot)
    }

    fn dc_option(&self, dc_id: i32) -> std::io::Result<Option<DcOption>> {
        Ok(self.data.lock().unwrap().dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, std::io::Result<()>> {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            data.dc_options.insert(dc_option.id, dc_option.clone());
            data.clone()
        };
        self.persist_snapshot(snapshot)
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, std::io::Result<Option<PeerInfo>>> {
        Box::pin(async move { Ok(self.data.lock().unwrap().peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, std::io::Result<()>> {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            data.peer_infos
                .entry(peer.id())
                .or_insert_with(|| peer.clone())
                .extend_info(&peer);
            data.clone()
        };
        self.persist_snapshot(snapshot)
    }

    fn updates_state(&self) -> BoxFuture<'_, std::io::Result<UpdatesState>> {
        Box::pin(async move { Ok(self.data.lock().unwrap().updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, std::io::Result<()>> {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            match update {
                UpdateState::All(updates_state) => {
                    data.updates_state = updates_state;
                }
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => {
                    data.updates_state.qts = qts;
                }
                UpdateState::Channel { id, pts } => {
                    data.updates_state.channels.retain(|c| c.id != id);
                    data.updates_state.channels.push(ChannelState { id, pts });
                }
            }
            data.clone()
        };
        self.persist_snapshot(snapshot)
    }
}


// Telegram grammers provider implementation.
//
// Keep provider-specific MTProto transport details here. Do not add shared
// chat bridge/business semantics to this file; adapter and chat bridge layers are
// responsible for cross-channel meaning.
