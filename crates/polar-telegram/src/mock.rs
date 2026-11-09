use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::client::{
    ResolvedTelegramBotConfig, ResolvedTelegramRuntimeConfig, TelegramBotRuntimeView,
    TelegramClient,
};
use crate::runtime::{
    TelegramDownloadedFile, TelegramInboundAttachment, TelegramInboundEnvelope,
    TelegramInboundMessage, TelegramOutboundDocument, TelegramOutboundMessage,
    TelegramOutboundPhoto, TelegramSentMessage, TelegramTypingAction,
};

pub struct MockTelegramClient {
    root: PathBuf,
    bots: Vec<TelegramBotRuntimeView>,
    next_update_id: u64,
    next_message_id: i32,
}

impl MockTelegramClient {
    pub(crate) async fn from_env(config: &ResolvedTelegramRuntimeConfig) -> Result<Self> {
        Self::new(config)
    }

    pub(crate) fn new(config: &ResolvedTelegramRuntimeConfig) -> Result<Self> {
        let root = config
            .session_dir
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("telegram mock session_dir is not configured"))?;
        let inbox = inbox_dir(&root);
        let processed = processed_dir(&root);
        fs::create_dir_all(&inbox)
            .with_context(|| format!("create telegram mock inbox {}", inbox.display()))?;
        fs::create_dir_all(&processed).with_context(|| {
            format!(
                "create telegram mock processed directory {}",
                processed.display()
            )
        })?;
        if config.bots.is_empty() {
            return Err(anyhow!("telegram mock bots are not configured"));
        }
        let bots = config
            .bots
            .iter()
            .enumerate()
            .map(|(index, bot)| runtime_view(&root, bot, index))
            .collect();
        Ok(Self {
            root,
            bots,
            next_update_id: 1,
            next_message_id: 0,
        })
    }

    fn outbox_path(&self) -> PathBuf {
        self.root.join("mock").join("outbox.jsonl")
    }

    fn append_outbox(&self, event: MockOutboundEvent) -> Result<()> {
        let outbox = self.outbox_path();
        if let Some(parent) = outbox.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create telegram mock outbox dir {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&outbox)
            .with_context(|| format!("open telegram mock outbox {}", outbox.display()))?;
        let line = serde_json::to_string(&event).context("serialize telegram mock outbox event")?;
        writeln!(file, "{line}").context("write telegram mock outbox event")?;
        Ok(())
    }
}

#[async_trait]
impl TelegramClient for MockTelegramClient {
    fn runtime_bots(&self) -> Vec<TelegramBotRuntimeView> {
        self.bots.clone()
    }

    async fn poll_updates(&mut self) -> Result<Vec<TelegramInboundEnvelope>> {
        let inbox = inbox_dir(&self.root);
        let processed = processed_dir(&self.root);
        fs::create_dir_all(&inbox)
            .with_context(|| format!("create telegram mock inbox {}", inbox.display()))?;
        fs::create_dir_all(&processed).with_context(|| {
            format!(
                "create telegram mock processed directory {}",
                processed.display()
            )
        })?;

        let mut entries = fs::read_dir(&inbox)
            .with_context(|| format!("read telegram mock inbox {}", inbox.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("list telegram mock inbox {}", inbox.display()))?;
        entries.sort_by_key(|entry| entry.file_name());

        let mut updates = Vec::new();
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read telegram mock inbox file {}", path.display()))?;
            let input: MockInboundFile = serde_json::from_str(&raw)
                .with_context(|| format!("parse telegram mock inbox file {}", path.display()))?;
            let update_id = match input.update_id {
                Some(update_id) => {
                    self.next_update_id = self.next_update_id.max(update_id + 1);
                    update_id
                }
                None => {
                    let update_id = self.next_update_id;
                    self.next_update_id += 1;
                    update_id
                }
            };
            updates.push(TelegramInboundEnvelope {
                update_id,
                message: TelegramInboundMessage {
                    metadata: Default::default(),
                    bot_name: input.bot_name,
                    chat_id: input.chat_id,
                    remote_user_id: input.remote_user_id,
                    remote_display_name: input.remote_display_name,
                    remote_username: input.remote_username,
                    text: input.text.unwrap_or_default(),
                    attachments: input
                        .attachments
                        .into_iter()
                        .map(TelegramInboundAttachment::from)
                        .collect(),
                },
            });
            let target = processed.join(entry.file_name());
            if target.exists() {
                fs::remove_file(&target).with_context(|| {
                    format!("replace telegram mock processed file {}", target.display())
                })?;
            }
            fs::rename(&path, &target).with_context(|| {
                format!(
                    "move telegram mock inbox file {} to {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
        Ok(updates)
    }

    async fn send_message(
        &mut self,
        message: TelegramOutboundMessage,
    ) -> Result<TelegramSentMessage> {
        self.next_message_id += 1;
        let sent = TelegramSentMessage {
            bot_name: message.bot_name.clone(),
            chat_id: message.chat_id,
            message_id: self.next_message_id,
        };
        self.append_outbox(MockOutboundEvent {
            kind: "send_message",
            bot_name: message.bot_name,
            chat_id: message.chat_id,
            message_id: Some(sent.message_id),
            text: Some(message.text),
            filename: None,
            at: current_unix_secs(),
        })?;
        Ok(sent)
    }

    async fn edit_message(&mut self, message: &TelegramSentMessage, text: String) -> Result<()> {
        self.append_outbox(MockOutboundEvent {
            kind: "edit_message",
            bot_name: message.bot_name.clone(),
            chat_id: message.chat_id,
            message_id: Some(message.message_id),
            text: Some(text),
            filename: None,
            at: current_unix_secs(),
        })
    }

    async fn delete_message(&mut self, message: &TelegramSentMessage) -> Result<()> {
        self.append_outbox(MockOutboundEvent {
            kind: "delete_message",
            bot_name: message.bot_name.clone(),
            chat_id: message.chat_id,
            message_id: Some(message.message_id),
            text: None,
            filename: None,
            at: current_unix_secs(),
        })
    }

    async fn send_typing(&mut self, action: TelegramTypingAction) -> Result<()> {
        self.append_outbox(MockOutboundEvent {
            kind: "typing",
            bot_name: action.bot_name,
            chat_id: action.chat_id,
            message_id: None,
            text: None,
            filename: None,
            at: current_unix_secs(),
        })
    }

    async fn send_photo(&mut self, photo: TelegramOutboundPhoto) -> Result<TelegramSentMessage> {
        self.next_message_id += 1;
        let sent = TelegramSentMessage {
            bot_name: photo.bot_name.clone(),
            chat_id: photo.chat_id,
            message_id: self.next_message_id,
        };
        self.append_outbox(MockOutboundEvent {
            kind: "send_photo",
            bot_name: photo.bot_name,
            chat_id: photo.chat_id,
            message_id: Some(sent.message_id),
            text: photo.caption,
            filename: None,
            at: current_unix_secs(),
        })?;
        Ok(sent)
    }

    async fn send_document(
        &mut self,
        document: TelegramOutboundDocument,
    ) -> Result<TelegramSentMessage> {
        self.next_message_id += 1;
        let sent = TelegramSentMessage {
            bot_name: document.bot_name.clone(),
            chat_id: document.chat_id,
            message_id: self.next_message_id,
        };
        self.append_outbox(MockOutboundEvent {
            kind: "send_document",
            bot_name: document.bot_name,
            chat_id: document.chat_id,
            message_id: Some(sent.message_id),
            text: document.caption,
            filename: document.filename,
            at: current_unix_secs(),
        })?;
        Ok(sent)
    }

    async fn download_file(
        &mut self,
        _bot_name: &str,
        file_id: &str,
    ) -> Result<TelegramDownloadedFile> {
        Ok(TelegramDownloadedFile {
            filename: Some(file_id.to_string()),
            mime_type: Some("application/octet-stream".to_string()),
            bytes: Vec::new(),
        })
    }
}

fn runtime_view(
    root: &Path,
    ResolvedTelegramBotConfig { bot_name, .. }: &ResolvedTelegramBotConfig,
    index: usize,
) -> TelegramBotRuntimeView {
    TelegramBotRuntimeView {
        bot_name: bot_name.clone(),
        telegram_user_id: Some(9_000_000 + index as i64),
        telegram_name: Some(format!("Mock {bot_name}")),
        telegram_username: Some(format!("mock_{bot_name}")),
        authorized: true,
        session_path: Some(root.display().to_string()),
        session_present: true,
    }
}

fn inbox_dir(root: &Path) -> PathBuf {
    root.join("mock").join("inbox")
}

fn processed_dir(root: &Path) -> PathBuf {
    root.join("mock").join("processed")
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct MockInboundFile {
    update_id: Option<u64>,
    bot_name: String,
    chat_id: i64,
    remote_user_id: Option<i64>,
    remote_display_name: Option<String>,
    remote_username: Option<String>,
    text: Option<String>,
    #[serde(default)]
    attachments: Vec<MockInboundAttachment>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MockInboundAttachment {
    Photo {
        file_id: String,
        file_unique_id: Option<String>,
    },
    Document {
        file_id: String,
        file_unique_id: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
    },
}

impl From<MockInboundAttachment> for TelegramInboundAttachment {
    fn from(value: MockInboundAttachment) -> Self {
        match value {
            MockInboundAttachment::Photo {
                file_id,
                file_unique_id,
            } => Self::Photo {
                file_id,
                file_unique_id,
            },
            MockInboundAttachment::Document {
                file_id,
                file_unique_id,
                file_name,
                mime_type,
            } => Self::Document {
                file_id,
                file_unique_id,
                file_name,
                mime_type,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct MockOutboundEvent {
    kind: &'static str,
    bot_name: String,
    chat_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    at: u64,
}
