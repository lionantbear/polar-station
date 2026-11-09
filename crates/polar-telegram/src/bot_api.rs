// Telegram Bot API provider implementation.
//
// Keep provider-specific HTTP transport details here. Do not add shared
// chat bridge/business semantics to this file; adapter and chat bridge layers are
// responsible for cross-channel meaning.
//
// Current repo/testing note:
// - `telegramtest` runs with `provider = "grammers"`, not `bot_api`
// - this provider remains supported, but it is not the primary dev
//   validation path right now
// - do not treat Bot API-only success as proof that Telegram behavior is
//   correct on `telegramtest`

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{multipart, Client as HttpClient};
use serde::Deserialize;

use crate::client::{
    ResolvedTelegramBotConfig, ResolvedTelegramRuntimeConfig, TelegramBotRuntimeView,
    TelegramClient,
};
use crate::runtime::{
    TelegramAttachmentPayload, TelegramDownloadedFile, TelegramInboundAttachment,
    TelegramInboundEnvelope, TelegramInboundMessage, TelegramOutboundDocument,
    TelegramOutboundMessage, TelegramOutboundPhoto, TelegramSentMessage, TelegramTypingAction,
};

#[derive(Clone, PartialEq, Eq)]
pub struct TelegramBotApiBot {
    pub bot_name: String,
    pub token: String,
    pub runtime: TelegramBotRuntimeView,
}

impl std::fmt::Debug for TelegramBotApiBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotApiBot")
            .field("bot_name", &self.bot_name)
            .field("token", &"<redacted>")
            .field("runtime", &self.runtime)
            .finish()
    }
}

#[derive(Clone)]
pub struct TelegramBotApiClient {
    http: HttpClient,
    base_url: String,
    offsets: HashMap<String, u64>,
    bots: HashMap<String, TelegramBotApiBot>,
}

impl TelegramBotApiClient {
    /// Read-only provider chat identity; authorization decisions belong to callers.
    pub async fn chat_identity(&self, bot_name: &str, chat_id: i64) -> Result<(i64, String)> {
        let bot = self.bot(bot_name)?;
        let response = self
            .http
            .get(format!("{}/bot{}/getChat", self.base_url, bot.token))
            .query(&[("chat_id", chat_id)])
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<TelegramChat> = checked_body(response).await?;
        Ok((body.result.id, body.result.kind.unwrap_or_default()))
    }

    /// The same credential resolver as the generic provider factory, retaining
    /// the explicit-cursor and independent-send primitives for managed hosts.
    pub async fn from_config(
        config: &crate::TelegramProviderConfig,
        resolver: &dyn polar_vault::CredentialBundleResolver,
    ) -> Result<Self> {
        anyhow::ensure!(
            config.provider == crate::TelegramClientProvider::BotApi,
            "expected Bot API provider"
        );
        Self::from_env(&crate::client::resolve_runtime_config(config, resolver).await?).await
    }

    /// Read-only preflight. Hosts decide whether to consume; never delete a webhook.
    pub async fn has_webhook(&self, bot_name: &str) -> Result<bool> {
        let bot = self.bot(bot_name)?;
        let response = self
            .http
            .get(format!("{}/bot{}/getWebhookInfo", self.base_url, bot.token))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        #[derive(Deserialize)]
        struct WebhookInfo {
            url: String,
        }
        let body: TelegramApiResponse<WebhookInfo> = checked_body(response).await?;
        Ok(!body.result.url.is_empty())
    }
    /// Poll one bot using the caller's durable cursor. This never acknowledges a
    /// returned batch internally and can run concurrently with `send_text`.
    pub async fn poll_batch(
        &self,
        bot_name: &str,
        offset: Option<u64>,
        timeout_secs: u8,
    ) -> Result<crate::TelegramUpdateBatch> {
        anyhow::ensure!(
            timeout_secs <= 30,
            "Telegram poll timeout exceeds 30 seconds"
        );
        let bot = self.bot(bot_name)?;
        let response = self
            .http
            .get(format!("{}/bot{}/getUpdates", self.base_url, bot.token))
            .query(&[
                ("timeout", u64::from(timeout_secs)),
                ("offset", offset.unwrap_or(0)),
                ("limit", 100),
            ])
            .timeout(std::time::Duration::from_secs(u64::from(timeout_secs) + 10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<Vec<TelegramUpdate>> = checked_body(response).await?;
        let mut next_offset = offset;
        let mut updates = Vec::with_capacity(body.result.len());
        for update in body.result {
            let following = update
                .update_id
                .checked_add(1)
                .ok_or_else(|| anyhow!("Telegram update ID overflow"))?;
            next_offset = Some(next_offset.unwrap_or(0).max(following));
            let edited = update.message.is_none() && update.edited_message.is_some();
            let message = update.message.or(update.edited_message).map(|message| {
                let attachments = inbound_attachments_from_message(&message);
                // Telegram can include a compatibility `from` user for messages
                // sent on behalf of a chat. It is not the authenticated human.
                let user = if message.sender_chat.is_some() {
                    None
                } else {
                    message.from
                };
                TelegramInboundMessage {
                    metadata: crate::TelegramMessageMetadata {
                        message_id: message.message_id,
                        reply_to_message_id: message
                            .reply_to_message
                            .as_ref()
                            .and_then(|m| m.message_id),
                        message_thread_id: message.message_thread_id,
                        reply_to_message_thread_id: message
                            .reply_to_message
                            .as_ref()
                            .and_then(|m| m.message_thread_id),
                        chat_type: message.chat.kind,
                        forwarded: message.forward_origin.is_some()
                            || message.forward_date.is_some(),
                        edited,
                    },
                    bot_name: bot_name.to_owned(),
                    chat_id: message.chat.id,
                    remote_user_id: user.as_ref().map(|user| user.id),
                    remote_display_name: user.as_ref().map(telegram_user_display_name),
                    remote_username: user.and_then(|user| user.username),
                    text: message.text.unwrap_or_default(),
                    attachments,
                }
            });
            updates.push(crate::TelegramProviderUpdate {
                update_id: update.update_id,
                message,
            });
        }
        Ok(crate::TelegramUpdateBatch {
            bot_name: bot_name.to_owned(),
            next_offset,
            updates,
        })
    }

    /// Independent send lane; no receive mutex is held across the HTTP request.
    pub async fn send_text(&self, message: TelegramOutboundMessage) -> Result<TelegramSentMessage> {
        self.send_text_reply(message, None).await
    }

    /// Send into a native topic without falling back to the main chat.
    pub async fn send_text_in_thread(
        &self,
        message: TelegramOutboundMessage,
        thread_id: i64,
        reply_to: Option<i32>,
    ) -> Result<TelegramSentMessage> {
        anyhow::ensure!(thread_id > 0, "invalid message thread ID");
        self.send_text_target(message, reply_to, Some(thread_id))
            .await
    }

    /// Native reply reference, confined to the same chat as the outgoing message.
    pub async fn send_text_reply(
        &self,
        message: TelegramOutboundMessage,
        reply_to: Option<i32>,
    ) -> Result<TelegramSentMessage> {
        self.send_text_target(message, reply_to, None).await
    }

    async fn send_text_target(
        &self,
        message: TelegramOutboundMessage,
        reply_to: Option<i32>,
        thread_id: Option<i64>,
    ) -> Result<TelegramSentMessage> {
        let bot = self.bot(&message.bot_name)?;
        let mut body = serde_json::json!({"chat_id":message.chat_id,"text":message.text,
            "link_preview_options":{"is_disabled":true}});
        if let Some(id) = reply_to {
            anyhow::ensure!(id > 0, "invalid reply message ID");
            body["reply_parameters"] =
                serde_json::json!({"message_id":id,"allow_sending_without_reply":false});
        }
        if let Some(id) = thread_id {
            body["message_thread_id"] = id.into();
        }
        let response = self
            .http
            .post(format!("{}/bot{}/sendMessage", self.base_url, bot.token))
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<TelegramSentMessageBody> = checked_body(response).await?;
        anyhow::ensure!(body.result.message_id > 0, "invalid sent message ID");
        if let Some(id) = thread_id {
            // A successful HTTP response is insufficient: a deleted topic may
            // cause the provider to put a message in the main chat. Never retry
            // that uncertain send as an unthreaded message.
            anyhow::ensure!(
                body.result.message_thread_id == Some(id)
                    && body.result.chat.as_ref().map(|c| c.id) == Some(message.chat_id),
                "telegram topic delivery identity mismatch"
            );
        }
        Ok(TelegramSentMessage {
            bot_name: message.bot_name,
            chat_id: message.chat_id,
            message_id: body.result.message_id,
        })
    }

    /// Native provider capability; does not change BotFather settings.
    pub async fn has_private_topics(&self, bot_name: &str) -> Result<bool> {
        let bot = self.bot(bot_name)?;
        let response = self
            .http
            .get(format!("{}/bot{}/getMe", self.base_url, bot.token))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<serde_json::Value> = checked_body(response).await?;
        Ok(body
            .result
            .get("has_topics_enabled")
            .and_then(|v| v.as_bool())
            == Some(true))
    }

    /// Returns the provider topic identity; timeout does not authorize re-creation.
    pub async fn create_forum_topic(
        &self,
        bot_name: &str,
        chat_id: i64,
        name: &str,
    ) -> Result<i64> {
        anyhow::ensure!(
            chat_id != 0 && !name.trim().is_empty() && name.chars().count() <= 128,
            "invalid forum topic destination or name"
        );
        let bot = self.bot(bot_name)?;
        let response = self
            .http
            .post(format!(
                "{}/bot{}/createForumTopic",
                self.base_url, bot.token
            ))
            .json(&serde_json::json!({"chat_id":chat_id,"name":name}))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<serde_json::Value> = checked_body(response).await?;
        body.result
            .get("message_thread_id")
            .and_then(|v| v.as_i64())
            .filter(|id| *id > 0)
            .ok_or_else(|| anyhow!("invalid created topic identity"))
    }

    pub fn new(base_url: impl Into<String>, bots: Vec<TelegramBotApiBot>) -> Self {
        Self {
            http: HttpClient::new(),
            base_url: base_url.into(),
            offsets: HashMap::new(),
            bots: bots
                .into_iter()
                .map(|bot| (bot.bot_name.clone(), bot))
                .collect(),
        }
    }

    pub(crate) async fn from_env(config: &ResolvedTelegramRuntimeConfig) -> Result<Self> {
        let http = HttpClient::new();
        let mut bots = Vec::new();
        for ResolvedTelegramBotConfig {
            bot_name,
            bot_token,
        } in &config.bots
        {
            let token = bot_token
                .as_deref()
                .ok_or_else(|| anyhow!("telegram bot token is not configured for {bot_name}"))?;
            let runtime =
                fetch_bot_runtime(&http, &config.bot_api_base_url, bot_name, token).await?;
            bots.push(TelegramBotApiBot {
                bot_name: bot_name.clone(),
                token: token.to_string(),
                runtime,
            });
        }
        if bots.is_empty() {
            return Err(anyhow!("telegram bots are not configured"));
        }
        Ok(Self {
            http,
            base_url: config.bot_api_base_url.clone(),
            offsets: HashMap::new(),
            bots: bots
                .into_iter()
                .map(|bot| (bot.bot_name.clone(), bot))
                .collect(),
        })
    }

    fn bot(&self, bot_name: &str) -> Result<&TelegramBotApiBot> {
        self.bots
            .get(bot_name)
            .ok_or_else(|| anyhow!("unknown telegram bot {bot_name}"))
    }
}

/// Fixed provider errors omit Telegram's token-bearing request URL and free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramApiFailure {
    Unauthorized,
    ConsumerConflict,
    RateLimited { retry_after_secs: u64 },
    Rejected { code: u16 },
}

impl std::fmt::Display for TelegramApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Telegram provider: {self:?}")
    }
}
impl std::error::Error for TelegramApiFailure {}

async fn checked_body<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<TelegramApiResponse<T>> {
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "Telegram response exceeds limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("Telegram response is not valid JSON"))?;
    if !(200..300).contains(&status) || value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let code = value
            .get("error_code")
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(status);
        return Err(match code {
            401 | 403 => TelegramApiFailure::Unauthorized,
            409 => TelegramApiFailure::ConsumerConflict,
            429 => TelegramApiFailure::RateLimited {
                retry_after_secs: value
                    .pointer("/parameters/retry_after")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
                    .max(1),
            },
            code => TelegramApiFailure::Rejected { code },
        }
        .into());
    }
    serde_json::from_value(value).map_err(|_| anyhow!("Telegram response has invalid fields"))
}

#[async_trait]
impl TelegramClient for TelegramBotApiClient {
    fn runtime_bots(&self) -> Vec<TelegramBotRuntimeView> {
        self.bots.values().map(|bot| bot.runtime.clone()).collect()
    }

    async fn poll_updates(&mut self) -> Result<Vec<TelegramInboundEnvelope>> {
        let mut inbound = Vec::new();
        for bot_name in self.bots.keys().cloned().collect::<Vec<_>>() {
            let batch = self
                .poll_batch(&bot_name, self.offsets.get(&bot_name).copied(), 0)
                .await?;
            if let Some(offset) = batch.next_offset {
                self.offsets.insert(bot_name, offset);
            }
            inbound.extend(batch.updates.into_iter().filter_map(|update| {
                update.message.map(|message| TelegramInboundEnvelope {
                    update_id: update.update_id,
                    message,
                })
            }));
        }
        Ok(inbound)
    }

    async fn send_message(
        &mut self,
        message: TelegramOutboundMessage,
    ) -> Result<TelegramSentMessage> {
        self.send_text(message).await
    }

    async fn edit_message(&mut self, message: &TelegramSentMessage, text: String) -> Result<()> {
        let bot = self.bot(&message.bot_name)?;
        let url = format!("{}/bot{}/editMessageText", self.base_url, bot.token);
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "chat_id": message.chat_id,
                "message_id": message.message_id,
                "text": text
            }))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<serde_json::Value> = checked_body(response).await?;
        if !body.ok {
            return Err(anyhow!(
                "telegram editMessageText returned ok=false for {}",
                message.bot_name
            ));
        }
        Ok(())
    }

    async fn delete_message(&mut self, message: &TelegramSentMessage) -> Result<()> {
        let bot = self.bot(&message.bot_name)?;
        let url = format!("{}/bot{}/deleteMessage", self.base_url, bot.token);
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "chat_id": message.chat_id,
                "message_id": message.message_id
            }))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<bool> = checked_body(response).await?;
        if !body.ok || !body.result {
            return Err(anyhow!(
                "telegram deleteMessage returned ok=false for {}",
                message.bot_name
            ));
        }
        Ok(())
    }

    async fn send_typing(&mut self, action: TelegramTypingAction) -> Result<()> {
        let bot = self.bot(&action.bot_name)?;
        let url = format!("{}/bot{}/sendChatAction", self.base_url, bot.token);
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "chat_id": action.chat_id,
                "action": "typing"
            }))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<bool> = checked_body(response).await?;
        if !body.ok || !body.result {
            return Err(anyhow!(
                "telegram sendChatAction returned ok=false for {}",
                action.bot_name
            ));
        }
        Ok(())
    }

    async fn send_photo(&mut self, photo: TelegramOutboundPhoto) -> Result<TelegramSentMessage> {
        let bot = self.bot(&photo.bot_name)?;
        let url = format!("{}/bot{}/sendPhoto", self.base_url, bot.token);
        let mut form = multipart::Form::new().text("chat_id", photo.chat_id.to_string());
        if let Some(caption) = &photo.caption {
            form = form.text("caption", caption.clone());
        }
        form = form.part(
            "photo",
            multipart_part_from_payload(&photo.payload, Some("photo.jpg")).await?,
        );
        let response = self
            .http
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<TelegramSentMessageBody> = checked_body(response).await?;
        if !body.ok {
            return Err(anyhow!(
                "telegram sendPhoto returned ok=false for {}",
                photo.bot_name
            ));
        }
        Ok(TelegramSentMessage {
            bot_name: photo.bot_name,
            chat_id: photo.chat_id,
            message_id: body.result.message_id,
        })
    }

    async fn send_document(
        &mut self,
        document: TelegramOutboundDocument,
    ) -> Result<TelegramSentMessage> {
        let bot = self.bot(&document.bot_name)?;
        let url = format!("{}/bot{}/sendDocument", self.base_url, bot.token);
        let mut form = multipart::Form::new().text("chat_id", document.chat_id.to_string());
        if let Some(caption) = &document.caption {
            form = form.text("caption", caption.clone());
        }
        form = form.part(
            "document",
            multipart_part_from_payload(&document.payload, document.filename.as_deref()).await?,
        );
        let response = self
            .http
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<TelegramSentMessageBody> = checked_body(response).await?;
        if !body.ok {
            return Err(anyhow!(
                "telegram sendDocument returned ok=false for {}",
                document.bot_name
            ));
        }
        Ok(TelegramSentMessage {
            bot_name: document.bot_name,
            chat_id: document.chat_id,
            message_id: body.result.message_id,
        })
    }

    async fn download_file(
        &mut self,
        bot_name: &str,
        file_id: &str,
    ) -> Result<TelegramDownloadedFile> {
        let bot = self.bot(bot_name)?;
        let url = format!("{}/bot{}/getFile", self.base_url, bot.token);
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({ "file_id": file_id }))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let body: TelegramApiResponse<TelegramFileBody> = checked_body(response).await?;
        if !body.ok {
            return Err(anyhow!("telegram getFile returned ok=false for {bot_name}"));
        }
        let file_path = body
            .result
            .file_path
            .clone()
            .ok_or_else(|| anyhow!("telegram getFile missing file_path for {bot_name}"))?;
        let bytes = self
            .http
            .get(format!(
                "{}/file/bot{}/{}",
                self.base_url, bot.token, file_path
            ))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)?
            .to_vec();
        Ok(TelegramDownloadedFile {
            filename: body
                .result
                .file_name
                .or_else(|| file_path.rsplit('/').next().map(str::to_owned)),
            mime_type: body.result.mime_type,
            bytes,
        })
    }
}

async fn fetch_bot_runtime(
    http: &HttpClient,
    base_url: &str,
    bot_name: &str,
    token: &str,
) -> Result<TelegramBotRuntimeView> {
    let url = format!("{}/bot{}/getMe", base_url, token);
    let response = http
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    let body: TelegramApiResponse<TelegramBotIdentity> = checked_body(response).await?;
    if !body.ok {
        return Err(anyhow!("telegram getMe returned ok=false for {bot_name}"));
    }
    Ok(TelegramBotRuntimeView {
        bot_name: bot_name.to_string(),
        telegram_user_id: Some(body.result.id),
        telegram_name: Some(telegram_identity_display_name(&body.result)),
        telegram_username: body.result.username,
        authorized: true,
        session_path: None,
        session_present: false,
    })
}

#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: T,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: u64,
    message: Option<TelegramMessage>,
    edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: Option<i32>,
    reply_to_message: Option<Box<TelegramMessage>>,
    message_thread_id: Option<i64>,
    forward_origin: Option<serde_json::Value>,
    forward_date: Option<i64>,
    chat: TelegramChat,
    sender_chat: Option<TelegramChat>,
    from: Option<TelegramUser>,
    text: Option<String>,
    photo: Option<Vec<TelegramPhotoSize>>,
    document: Option<TelegramDocument>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramPhotoSize {
    file_id: String,
    file_unique_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramDocument {
    file_id: String,
    file_unique_id: Option<String>,
    file_name: Option<String>,
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramFileBody {
    file_path: Option<String>,
    file_name: Option<String>,
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    first_name: String,
    last_name: Option<String>,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramBotIdentity {
    id: i64,
    first_name: String,
    username: Option<String>,
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramSentMessageBody {
    message_id: i32,
    message_thread_id: Option<i64>,
    chat: Option<TelegramChat>,
}

fn telegram_user_display_name(user: &TelegramUser) -> String {
    match user.last_name.as_deref() {
        Some(last) if !last.is_empty() => format!("{} {}", user.first_name, last),
        _ => user.first_name.clone(),
    }
}

fn telegram_identity_display_name(user: &TelegramBotIdentity) -> String {
    match user.last_name.as_deref() {
        Some(last) if !last.is_empty() => format!("{} {}", user.first_name, last),
        _ => user.first_name.clone(),
    }
}

fn inbound_attachments_from_message(message: &TelegramMessage) -> Vec<TelegramInboundAttachment> {
    let mut attachments = Vec::new();
    if let Some(photo_sizes) = &message.photo {
        if let Some(photo) = photo_sizes.last() {
            attachments.push(TelegramInboundAttachment::Photo {
                file_id: photo.file_id.clone(),
                file_unique_id: photo.file_unique_id.clone(),
            });
        }
    }
    if let Some(document) = &message.document {
        attachments.push(TelegramInboundAttachment::Document {
            file_id: document.file_id.clone(),
            file_unique_id: document.file_unique_id.clone(),
            file_name: document.file_name.clone(),
            mime_type: document.mime_type.clone(),
        });
    }
    attachments
}

async fn multipart_part_from_payload(
    payload: &TelegramAttachmentPayload,
    file_name: Option<&str>,
) -> Result<multipart::Part> {
    let (bytes, inferred_name) = match payload {
        TelegramAttachmentPayload::Bytes(bytes) => (bytes.clone(), None),
        TelegramAttachmentPayload::FilePath(path) => {
            let bytes = tokio::fs::read(path).await?;
            let inferred_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            (bytes, inferred_name)
        }
    };
    let mut part = multipart::Part::bytes(bytes);
    if let Some(name) = file_name.map(str::to_owned).or(inferred_name) {
        part = part.file_name(name);
    }
    Ok(part)
}


