use std::future::Future;

pub mod cas_files;
pub mod prompt_repository;

use anyhow::{bail, Result as AnyhowResult};
use async_trait::async_trait;
use polar_identity::{NodeId, UnixTimestamp};
use polar_lgp_model::{
    ChannelKind, ChannelMemberRecord, ChannelRecord, ChatRouteRecord, ContextCompactionRecord,
    InhousePolarInstanceRecord, InhousePolarRecord, InstanceStatus, LgpMessageKey,
    ManagedCausalMetadata, ManagementPolicyRecordSet, MessageAppendStatus, MessageAttachment,
    MessageAttachmentStorage, MessageKind, MessagePayload, MessageRecord, PromptActivationRecord,
    PromptAssetRecord, PromptBackend, SpeechRequestRecord, SpeechRequestStatus,
    TelegramPendingChatBridgeRequestRecord, TelegramSessionRecord, ThreadMessageAuthorKind,
    ThreadMessageCausalKind, ThreadMessageIngressKind, ThreadParticipantKind,
    ThreadParticipantRecord, ThreadRecord, ThreadRuntimeBindingRecord, TransportPlane,
};
use polar_storage_core::StorageResult as Result;

#[derive(Debug, Clone)]
pub enum ManagementPolicyWrite {
    Transport(polar_storage_core::RecordWrite<polar_lgp_model::ManagementTransportPolicyRecord>),
    ActorClass(polar_storage_core::RecordWrite<Vec<polar_lgp_model::ManagementActorClassRecord>>),
    Channel(polar_storage_core::RecordWrite<Vec<polar_lgp_model::ManagementChannelPolicyRecord>>),
    Thread(polar_storage_core::RecordWrite<Vec<polar_lgp_model::ManagementThreadPolicyRecord>>),
    Tool(polar_storage_core::RecordWrite<Vec<polar_lgp_model::ManagementToolPolicyRecord>>),
}

#[derive(Debug, Clone, Default)]
pub struct LgpBundlePromptBatch {
    pub channels: Vec<polar_lgp_model::ChannelRecord>,
    pub threads: Vec<polar_lgp_model::ThreadRecord>,
    pub prompt_assets: Vec<PromptAssetRecord>,
    pub prompt_activations: Vec<PromptActivationRecord>,
}

#[derive(Debug, Clone)]
pub struct PromptActivationQuery {
    pub inhouse_polar_id: String,
    pub system_role: Option<polar_lgp_model::InhousePolarSystemRole>,
    pub management_class: Option<String>,
    pub channel_id: String,
    pub thread_id: String,
    pub backend: PromptBackend,
}
use polar_tools_model::{ToolReplacementPolicyAuditRecord, ToolReplacementPolicyRecord};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LgpDbPerfContext {
    pub caller_span: String,
    pub query_scope: String,
}

impl LgpDbPerfContext {
    pub fn new(caller_span: impl Into<String>) -> Self {
        Self {
            caller_span: caller_span.into(),
            query_scope: String::new(),
        }
    }

    pub fn with_query_scope(mut self, query_scope: impl Into<String>) -> Self {
        self.query_scope = query_scope.into();
        self
    }
}

tokio::task_local! {
    static LGP_DB_PERF_CONTEXT: LgpDbPerfContext;
}

pub async fn with_lgp_db_perf_context<F>(context: LgpDbPerfContext, future: F) -> F::Output
where
    F: Future,
{
    LGP_DB_PERF_CONTEXT.scope(context, future).await
}

pub fn current_lgp_db_perf_context() -> Option<LgpDbPerfContext> {
    LGP_DB_PERF_CONTEXT.try_with(Clone::clone).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageIngressPersistDisposition {
    Existing,
    CreatedThread,
    CreatedChannelAndThread,
}

#[derive(Debug, Clone)]
pub struct MessageIngressPersistSource {
    pub sender_id: NodeId,
    pub sender_inhouse_polar_id: Option<String>,
    pub by_inhouse_polar_id: Option<String>,
    pub by_human_label: Option<String>,
    pub rewritten_by_owner: bool,
    pub author_kind: ThreadMessageAuthorKind,
    pub author_ref: Option<String>,
    pub ingress_kind: ThreadMessageIngressKind,
    pub ingress_ref: Option<String>,
    pub ingress_metadata_json: String,
    pub causal_kind: Option<ThreadMessageCausalKind>,
    pub causal_sender_id: Option<NodeId>,
    pub causal_message_id: Option<String>,
    pub trace_id: Option<String>,
    pub origin_thread_id: Option<String>,
    pub origin_message_id: Option<String>,
    pub previous_thread_id: Option<String>,
    pub previous_message_id: Option<String>,
    pub source_thread_id: Option<String>,
    pub source_message_id: Option<String>,
    pub chat_bridge_id: Option<String>,
    pub chat_route_id: Option<String>,
    pub hop_count: u32,
    pub causal_metadata_json: ManagedCausalMetadata,
}

#[derive(Debug, Clone)]
pub enum MessageIngressPersistTarget {
    ExistingThread {
        owner_id: NodeId,
        thread_id: String,
    },
    DirectMessage {
        owner_id: NodeId,
        channel_owner_id: NodeId,
        partner_id: NodeId,
        thread_id: String,
        access_plane: TransportPlane,
        channel_title: Option<String>,
        thread_tag: Option<String>,
        allow_create: bool,
    },
    ChannelThread {
        owner_id: NodeId,
        channel_id: String,
        thread_id: String,
        channel_kind: ChannelKind,
        access_plane: TransportPlane,
        partner_id: Option<NodeId>,
        partner_inhouse_polar_id: Option<String>,
        channel_title: Option<String>,
        thread_tag: Option<String>,
        allow_create: bool,
        require_new: bool,
    },
}

#[derive(Debug, Clone)]
pub struct MessageIngressPersistMessage {
    pub message_id: String,
    pub intent: String,
    pub payload: MessagePayload,
    pub attachments: Vec<MessageAttachment>,
    pub message_kind: MessageKind,
    pub created_at: UnixTimestamp,
}

#[derive(Debug, Clone)]
pub struct MessageIngressPersistRequest {
    pub source: MessageIngressPersistSource,
    pub target: MessageIngressPersistTarget,
    pub message: MessageIngressPersistMessage,
}

#[derive(Debug, Clone)]
pub struct MessageIngressPersistResult {
    pub append_status: MessageAppendStatus,
    pub disposition: MessageIngressPersistDisposition,
    pub channel: ChannelRecord,
    pub thread: ThreadRecord,
    pub message: MessageRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedMessageAttachment {
    pub attachment_id: String,
    pub blob_object_id: Option<String>,
    pub storage: MessageAttachmentStorage,
    pub kind: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentKeyGrantRecord {
    pub grant_id: String,
    pub grant_scope: String,
    pub key_reference: String,
    pub created_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobTransferRecord {
    pub transfer_id: String,
    pub direction: String,
    pub status: String,
    pub bytes_total: Option<u64>,
    pub bytes_transferred: u64,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobObjectReplicaRecord {
    pub source_hint: Option<polar_lgp_model::AttachmentSourceHint>,
    pub replica_id: String,
    pub storage_family: String,
    pub replica_role: String,
    pub acquisition_mode: String,
    pub locator: String,
    pub status: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachmentUseState {
    pub first_opened_at: Option<u64>,
    pub last_opened_at: Option<u64>,
    pub last_failed_at: Option<u64>,
    pub last_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAttachmentMetadata {
    pub attachment_id: String,
    pub attachment_order: usize,
    pub storage: MessageAttachmentStorage,
    pub kind: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub inline_data_url_present: bool,
    pub local_path: Option<String>,
    pub blob_object_id: Option<String>,
    pub blob_storage_family: Option<String>,
    pub blob_locator: Option<String>,
    pub blob_content_digest: Option<String>,
    pub blob_size_bytes: Option<u64>,
    pub key_grants: Vec<AttachmentKeyGrantRecord>,
    pub transfers: Vec<BlobTransferRecord>,
    pub replicas: Vec<BlobObjectReplicaRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAttachmentPolicy {
    pub inline_max_bytes: u64,
    pub inline_zip_max_bytes: u64,
    pub large_attachment_mode: String,
    pub hot_cache_retention_secs: u64,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
}

impl Default for ThreadAttachmentPolicy {
    fn default() -> Self {
        Self {
            inline_max_bytes: 1_048_576,
            inline_zip_max_bytes: 262_144,
            large_attachment_mode: "external_blob".to_string(),
            hot_cache_retention_secs: 3_600,
            created_at: UnixTimestamp::from_secs(0),
            updated_at: UnixTimestamp::from_secs(0),
        }
    }
}

/// Per-thread attachment policy override (partial, allowing selective overrides)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAttachmentPolicyOverride {
    pub owner_id: String,
    pub thread_id: String,
    pub inline_max_bytes: Option<u64>,
    pub inline_zip_max_bytes: Option<u64>,
    pub large_attachment_mode: Option<String>,
    pub external_durable_mode: Option<String>,
    pub default_s3_backend: Option<String>,
    pub hot_cache_retention_secs: Option<u64>,
}

/// Record of an attachment upload for tracking and reference generation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentUploadRecord {
    pub attachment_ref: String,
    pub blob_id: String,
    pub upload_by_owner_id: String,
    pub upload_by_thread_id: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub storage_family: String,
    pub locator: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadReadStateRecord {
    pub viewer_key: String,
    pub owner_id: NodeId,
    pub thread_id: String,
    pub latest_unread_message_id: Option<String>,
    pub latest_unread_message_created_at: Option<UnixTimestamp>,
    pub read_at: Option<UnixTimestamp>,
    pub unread: bool,
    pub updated_at: UnixTimestamp,
}

/// Result of atomically admitting canonical compaction truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextCompactionAppendStatus {
    Appended,
    Existing,
}

/// Focused canonical compaction persistence contract.
///
/// `LgpStore` extends this trait so every full LGP store exposes the same APIs.
/// Focused durable adapters, such as the current Postgres boundary crate, can
/// implement this contract without fabricating unrelated LGP capabilities.
#[async_trait]
pub trait LgpContextCompactionStore: Send + Sync {
    async fn append_context_compaction(
        &self,
        record: &ContextCompactionRecord,
    ) -> Result<ContextCompactionAppendStatus>;

    async fn get_context_compaction(
        &self,
        compaction_id: &str,
    ) -> Result<Option<ContextCompactionRecord>>;

    async fn get_latest_context_compaction(
        &self,
        thread_id: &str,
    ) -> Result<Option<ContextCompactionRecord>>;

    async fn list_context_compactions(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ContextCompactionRecord>>;
}

/// Shared chain/CAS validation used by every durable implementation while its
/// database transaction holds the per-thread writer lock.
pub fn validate_context_compaction_append(
    candidate: &ContextCompactionRecord,
    existing_by_id: Option<&ContextCompactionRecord>,
    latest_for_thread: Option<&ContextCompactionRecord>,
) -> AnyhowResult<ContextCompactionAppendStatus> {
    candidate.validate()?;
    if let Some(existing) = existing_by_id {
        if existing == candidate {
            return Ok(ContextCompactionAppendStatus::Existing);
        }
        bail!(
            "context compaction {} already exists with a conflicting payload",
            candidate.compaction_id
        );
    }

    match latest_for_thread {
        None => {
            if candidate.context_compaction_epoch != 1
                || candidate.previous_compaction_ref.is_some()
            {
                bail!(
                    "context compaction epoch CAS failed for thread {}: first record must have epoch 1 and no previous compaction",
                    candidate.thread_id
                );
            }
        }
        Some(latest) => {
            if latest.thread_id != candidate.thread_id {
                bail!("latest context compaction belongs to a different thread");
            }
            let expected_epoch = latest
                .context_compaction_epoch
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("context compaction epoch overflow"))?;
            let expected_previous = format!("lgp:compaction:{}", latest.compaction_id);
            if candidate.context_compaction_epoch != expected_epoch
                || candidate.previous_compaction_ref.as_deref() != Some(expected_previous.as_str())
            {
                bail!(
                    "context compaction epoch CAS failed for thread {}: expected epoch {} chained to {}, got epoch {} chained to {:?}",
                    candidate.thread_id,
                    expected_epoch,
                    expected_previous,
                    candidate.context_compaction_epoch,
                    candidate.previous_compaction_ref
                );
            }
        }
    }
    Ok(ContextCompactionAppendStatus::Appended)
}

/// Durable Thread membership and default-target operations.
#[async_trait]
pub trait ThreadParticipantStore: Send + Sync {
    async fn add_thread_participant(&self, record: &ThreadParticipantRecord) -> Result<()>;

    async fn list_thread_participants(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Vec<ThreadParticipantRecord>>;

    #[allow(clippy::too_many_arguments)]
    async fn set_thread_participant_active(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        participant_kind: ThreadParticipantKind,
        participant_id: &str,
        active: bool,
        default_inhouse_polar_id: Option<Option<&str>>,
        updated_at: UnixTimestamp,
    ) -> Result<()>;

    async fn set_thread_default_inhouse_polar_id(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        inhouse_polar_id: Option<&str>,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
}

#[async_trait]
pub trait LgpStore:
    LgpContextCompactionStore
    + ThreadParticipantStore
    + prompt_repository::PromptSceneStore
    + Send
    + Sync
{
    async fn persist_message_ingress(
        &self,
        request: &MessageIngressPersistRequest,
    ) -> Result<MessageIngressPersistResult>;

    async fn create_channel(&self, record: &ChannelRecord) -> Result<()>;
    async fn update_channel_updated_at(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn list_channels(&self, owner_id: &NodeId) -> Result<Vec<ChannelRecord>>;
    async fn list_all_channels(&self) -> Result<Vec<ChannelRecord>>;
    async fn get_channel(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
    ) -> Result<Option<ChannelRecord>>;
    async fn add_channel_member(&self, record: &ChannelMemberRecord) -> Result<()>;
    async fn list_channel_members(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
    ) -> Result<Vec<ChannelMemberRecord>>;
    async fn remove_channel_member(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
        member_id: &NodeId,
    ) -> Result<()>;
    async fn create_thread(&self, record: &ThreadRecord) -> Result<()>;
    async fn update_thread_updated_at(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn set_thread_partner_navigation_enabled(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        enabled: bool,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn set_thread_partner_inhouse_polar_id(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        partner_inhouse_polar_id: Option<&str>,
        partner_navigation_enabled: bool,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn get_thread(&self, owner_id: &NodeId, thread_id: &str) -> Result<Option<ThreadRecord>>;

    async fn mark_thread_unread(
        &self,
        viewer_key: &str,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
        message_created_at: UnixTimestamp,
        updated_at: UnixTimestamp,
    ) -> Result<ThreadReadStateRecord>;

    async fn mark_thread_read(
        &self,
        viewer_key: &str,
        owner_id: &NodeId,
        thread_id: &str,
        read_at: UnixTimestamp,
    ) -> Result<ThreadReadStateRecord>;

    async fn get_thread_read_state(
        &self,
        viewer_key: &str,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Option<ThreadReadStateRecord>>;

    async fn thread_has_unread(
        &self,
        viewer_key: &str,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<bool>;

    async fn channel_has_unread(
        &self,
        viewer_key: &str,
        channel_owner_id: &NodeId,
        channel_id: &str,
    ) -> Result<bool>;

    async fn list_direct_message_threads_for_partner_on_access_plane(
        &self,
        owner_id: &NodeId,
        partner_id: &NodeId,
        access_plane: TransportPlane,
    ) -> Result<Vec<ThreadRecord>>;

    async fn count_direct_messages_to_partner_between(
        &self,
        owner_id: &NodeId,
        partner_id: &NodeId,
        access_plane_id: &str,
        previous_message_id: &str,
        acked_message_id: &str,
    ) -> Result<u32>;

    async fn count_direct_messages_to_partner_after(
        &self,
        owner_id: &NodeId,
        partner_id: &NodeId,
        access_plane_id: &str,
        after_message_id: &str,
    ) -> Result<u32>;

    async fn list_recent_direct_messages_to_partner_after(
        &self,
        owner_id: &NodeId,
        partner_id: &NodeId,
        access_plane_id: &str,
        after_message_id: &str,
        limit: u32,
    ) -> Result<Vec<MessageRecord>>;
    async fn list_channel_threads(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
    ) -> Result<Vec<ThreadRecord>>;
    async fn set_direct_message_transport_plane(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        access_plane: TransportPlane,
        updated_at: UnixTimestamp,
    ) -> Result<bool>;
    async fn list_thread_messages(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Vec<MessageRecord>>;
    async fn list_thread_messages_recent(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    async fn list_thread_messages_oldest(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    /// Narrow projection for cache reconciliation. Adapters should avoid loading
    /// payloads; the default preserves the same ordering for non-SQL stores.
    async fn list_thread_message_keys(
        &self,
        owner: &NodeId,
        thread: &str,
        anchor: Option<&polar_lgp_model::LgpMessageCursor>,
        before: bool,
        limit: usize,
    ) -> Result<Vec<LgpMessageKey>> {
        let rows = if let Some(anchor) = anchor {
            self.list_thread_messages_around(owner, thread, anchor, before, limit).await?
        } else if before {
            self.list_thread_messages_recent(owner, thread, limit).await?
        } else {
            self.list_thread_messages_oldest(owner, thread, limit).await?
        };
        Ok(rows.iter().map(MessageRecord::primary_key).collect())
    }
    async fn list_thread_messages_after(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        after_created_at: UnixTimestamp,
        after_message_id: &str,
    ) -> Result<Vec<MessageRecord>>;
    async fn list_thread_messages_after_bounded(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        after_created_at: UnixTimestamp,
        after_message_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    async fn list_thread_messages_before(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        before_created_at: UnixTimestamp,
        before_message_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    async fn get_thread_message(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Option<MessageRecord>>;
    /// Reads the immutable message fact by its actual database primary key.
    async fn get_message_by_primary_key(
        &self,
        key: &LgpMessageKey,
    ) -> Result<Option<MessageRecord>>;
    /// Reads direct Reply facts by the complete causal parent key. The query is
    /// backed by the causal-parent index and never falls back to message_id.
    async fn list_direct_replies(
        &self,
        parent: &LgpMessageKey,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    /// All direct causal kinds, across local placements; not a transitive tree.
    async fn list_causal_children(
        &self,
        parent: &LgpMessageKey,
        after: Option<&polar_lgp_model::LgpMessageCursor>,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    /// Strictly before/after an exact cursor, returned in ascending total order.
    async fn list_thread_messages_around(
        &self,
        owner: &NodeId,
        thread: &str,
        anchor: &polar_lgp_model::LgpMessageCursor,
        before: bool,
        limit: usize,
    ) -> Result<Vec<MessageRecord>>;
    async fn get_message_by_key(
        &self,
        sender_id: &NodeId,
        message_id: &str,
    ) -> Result<Option<MessageRecord>>;
    async fn find_thread_message_by_previous_message_id(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        previous_message_id: &str,
    ) -> Result<Option<MessageRecord>>;
    async fn list_message_attachments(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Vec<MessageAttachment>>;
    async fn list_message_attachment_metadata(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Vec<MessageAttachmentMetadata>>;
    /// Current domain membership plus an exact attachment already in a message.
    async fn may_serve_attachment(
        &self,
        _owner: &NodeId,
        _peer: &NodeId,
        _locator: &str,
    ) -> Result<bool> {
        Ok(false)
    }
    /// A delivery receipt does not mark the attachment opened. Only a later
    /// authenticated client acknowledgement does. Old adapters return None.
    async fn issue_attachment_open_receipt(
        &self,
        _owner: &NodeId,
        _thread: &str,
        _message: &str,
        _attachment: &str,
        _mode: &str,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    async fn attachment_use_state(
        &self,
        _owner: &NodeId,
        _thread: &str,
        _message: &str,
        _attachment: &str,
    ) -> Result<Option<AttachmentUseState>> {
        Ok(None)
    }
    async fn confirm_attachment_use(
        &self,
        _owner: &NodeId,
        _thread: &str,
        _message: &str,
        _attachment: &str,
        _receipt: &str,
    ) -> Result<AttachmentUseState> {
        Err(anyhow::anyhow!("attachment usage is unsupported by this store").into())
    }
    async fn record_attachment_use_failure(
        &self,
        _owner: &NodeId,
        _thread: &str,
        _message: &str,
        _attachment: &str,
        _mode: &str,
    ) -> Result<()> {
        Ok(())
    }
    /// Local acquisition evidence, scoped to a message the caller resolved.
    async fn record_attachment_transfer(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
        attachment_id: &str,
        transfer: &BlobTransferRecord,
    ) -> Result<()>;
    async fn get_thread_attachment_policy(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Option<ThreadAttachmentPolicy>>;
    async fn materialize_message_attachment_by_id(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Option<MaterializedMessageAttachment>>;
    async fn get_channel_message(
        &self,
        owner_id: &NodeId,
        channel_id: &str,
        message_id: &str,
    ) -> Result<Option<MessageRecord>>;
    async fn create_inhouse_polar(&self, record: &InhousePolarRecord) -> Result<()>;
    async fn update_inhouse_polar(&self, record: &InhousePolarRecord) -> Result<()>;
    async fn get_inhouse_polar(&self, inhouse_polar_id: &str)
        -> Result<Option<InhousePolarRecord>>;
    async fn list_inhouse_polars(&self) -> Result<Vec<InhousePolarRecord>>;
    async fn apply_bundle_prompt_batch(&self, batch: LgpBundlePromptBatch) -> Result<()>;
    async fn upsert_prompt_asset(&self, record: &PromptAssetRecord) -> Result<()>;
    async fn get_prompt_asset(&self, asset_id: &str) -> Result<Option<PromptAssetRecord>>;
    async fn list_prompt_assets(&self) -> Result<Vec<PromptAssetRecord>>;
    async fn upsert_prompt_activation(&self, record: &PromptActivationRecord) -> Result<()>;
    async fn delete_prompt_activations_for_assets(&self, asset_ids: &[String]) -> Result<u64>;
    async fn list_prompt_activations(&self) -> Result<Vec<PromptActivationRecord>>;
    async fn list_prompt_activations_for_context(
        &self,
        query: &PromptActivationQuery,
    ) -> Result<Vec<PromptActivationRecord>>;
    async fn get_tool_replacement_policy(
        &self,
        policy_id: &str,
    ) -> Result<Option<ToolReplacementPolicyRecord>>;
    async fn list_tool_replacement_policies(&self) -> Result<Vec<ToolReplacementPolicyRecord>>;
    async fn upsert_tool_replacement_policy(
        &self,
        record: &ToolReplacementPolicyRecord,
        audit: &ToolReplacementPolicyAuditRecord,
    ) -> Result<()>;
    async fn upsert_telegram_session(&self, record: &TelegramSessionRecord) -> Result<()>;
    async fn list_telegram_sessions(&self) -> Result<Vec<TelegramSessionRecord>>;
    async fn get_telegram_session(&self, session_id: &str)
        -> Result<Option<TelegramSessionRecord>>;
    async fn upsert_telegram_pending_chat_bridge_request(
        &self,
        record: &TelegramPendingChatBridgeRequestRecord,
    ) -> Result<()>;
    async fn list_telegram_pending_chat_bridge_requests(
        &self,
    ) -> Result<Vec<TelegramPendingChatBridgeRequestRecord>>;
    async fn get_active_telegram_pending_chat_bridge_request_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<TelegramPendingChatBridgeRequestRecord>>;
    async fn upsert_thread_chat_bridge_route(&self, record: &ChatRouteRecord) -> Result<()>;
    async fn get_thread_chat_bridge_route(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Option<ChatRouteRecord>>;
    async fn list_thread_chat_bridge_routes(
        &self,
        owner_id: &NodeId,
    ) -> Result<Vec<ChatRouteRecord>>;
    async fn delete_thread_chat_bridge_route(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<()>;
    async fn create_instance(&self, record: &InhousePolarInstanceRecord) -> Result<()>;
    async fn get_instance(&self, instance_id: &str) -> Result<Option<InhousePolarInstanceRecord>>;
    async fn list_instances_for_inhouse_polar(
        &self,
        inhouse_polar_id: &str,
    ) -> Result<Vec<InhousePolarInstanceRecord>>;
    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: InstanceStatus,
        current_session_id: Option<&str>,
        last_error: Option<&str>,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn delete_instances_by_ids(&self, instance_ids: &[String]) -> Result<()>;
    async fn upsert_thread_binding(&self, record: &ThreadRuntimeBindingRecord) -> Result<()>;
    async fn get_thread_binding(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Option<ThreadRuntimeBindingRecord>>;
    async fn list_thread_bindings_for_inhouse_polar(
        &self,
        inhouse_polar_id: &str,
    ) -> Result<Vec<ThreadRuntimeBindingRecord>>;
    async fn detach_thread_binding(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
        detached_at: UnixTimestamp,
    ) -> Result<()>;
    async fn replace_management_policy(&self, policy: &ManagementPolicyRecordSet) -> Result<()>;
    async fn write_management_policy_objects(
        &self,
        _writes: Vec<ManagementPolicyWrite>,
    ) -> Result<Vec<polar_storage_core::RecordWriteOutcome>> {
        Err(polar_storage_core::StorageError::new(
            polar_storage_core::StorageErrorKind::InvalidInput,
            "atomic management configuration writes are unsupported by this adapter",
            None,
            None,
            polar_storage_core::RetryDisposition::Never,
        ))
    }
    async fn get_management_policy(&self) -> Result<ManagementPolicyRecordSet>;
    async fn create_speech_request(&self, record: &SpeechRequestRecord) -> Result<()>;
    async fn get_speech_request(&self, request_id: &str) -> Result<Option<SpeechRequestRecord>>;
    async fn list_speech_requests(&self) -> Result<Vec<SpeechRequestRecord>>;
    async fn update_speech_request_decision(
        &self,
        request_id: &str,
        status: SpeechRequestStatus,
        resolved_message_id: Option<&str>,
        updated_at: UnixTimestamp,
    ) -> Result<()>;
    async fn ensure_dm_thread_on_plane(
        &self,
        thread_id: &str,
        owner_id: &NodeId,
        channel_owner_id: &NodeId,
        partner_id: &NodeId,
        access_plane: TransportPlane,
        now: UnixTimestamp,
    ) -> Result<ThreadRecord>;

    // Attachment policy and upload management
    async fn get_thread_attachment_policy_override(
        &self,
        owner_id: &NodeId,
        thread_id: &str,
    ) -> Result<Option<ThreadAttachmentPolicyOverride>>;
    async fn upsert_thread_attachment_policy_override(
        &self,
        owner_id: &NodeId,
        override_policy: &ThreadAttachmentPolicyOverride,
    ) -> Result<()>;
    async fn create_attachment_upload(&self, record: &AttachmentUploadRecord) -> Result<()>;
    async fn find_registered_files(
        &self,
        _query: &cas_files::RegisteredFileQuery,
    ) -> Result<Vec<cas_files::RegisteredFile>> {
        Err(anyhow::anyhow!("file discovery is unsupported by this LGP store").into())
    }
    /// Opt-in metadata snapshot fence. Every continuation requires the token
    /// returned by the same adapter; callers must also validate before publishing
    /// an assembled result. The token does not freeze or authorize permissions.
    async fn find_registered_files_fenced(
        &self,
        query: &cas_files::RegisteredFileQuery,
        expected: Option<&cas_files::RegisteredFileMetadataToken>,
    ) -> Result<cas_files::FencedRegisteredFilePage> {
        query.validate_fenced(expected)?;
        Err(cas_files::unsupported_registration_fence())
    }
    /// Checks metadata afresh without retaining a connection or read transaction.
    async fn validate_registered_file_metadata(
        &self,
        expected: &cas_files::RegisteredFileMetadataToken,
    ) -> Result<()> {
        expected.validate()?;
        Err(cas_files::unsupported_registration_fence())
    }
    async fn get_attachment_upload(
        &self,
        attachment_ref: &str,
    ) -> Result<Option<AttachmentUploadRecord>>;
}
