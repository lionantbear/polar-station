pub mod cas_files;
pub mod cas_operations;
mod inline_attachment;
pub use inline_attachment::InlineMessageAttachment;
mod channel;
mod chat_bridge;
mod compaction;
mod maintenance;
mod management_policy;
mod message;
mod prompt;
pub use prompt::lifecycle_wire as prompt_lifecycle_wire;
mod speech;
mod system_tools;
mod telegram;
mod thread;
mod thread_participant;
mod tool_authz;

pub use channel::{AccessPlaneId, ChannelKind, ChannelMemberRecord, ChannelRecord, TransportPlane};
pub use chat_bridge::{ChatRouteRecord, ChatRouteStatus};
pub use compaction::{
    CompactionSourceSpan, CompactionSummaryQuality, CompactionTriggerKind, ContextCompactionRecord,
    ContextCompactionRecordError,
};
pub use maintenance::{
    MaintenanceTarget, MaintenanceThreadReplyBody, MaintenanceToolEventPayload,
    MaintenanceToolPayload,
};
pub use management_policy::{
    ManagementActorClassRecord, ManagementChannelPolicyRecord, ManagementConnectorPolicyRecord,
    ManagementPolicyRecordSet, ManagementThreadPolicyRecord, ManagementToolPolicyRecord,
    ManagementTransportPolicyRecord,
};
pub use message::{
    message_payload_with_attachments, payload_message_attachment_by_id,
    payload_message_attachments, ExactCausalReference, ExactCausalReferenceError,
    LgpAuthorSignatureVersion, LgpCausalEnvelope, LgpMessageCursor, LgpMessageKey,
    LgpMessageReference, LgpSenderEvidenceV1, ManagedCausalMetadata, ManagedCausalMetadataError,
    MessageAppendStatus, MessageAttachment, MessageAttachmentReplica, MessageAttachmentStorage,
    MessageKind, MessagePayload, MessagePayloadAttachmentProjectionError, MessageRecord,
    ThreadMessageAuthorKind, ThreadMessageCausalKind, ThreadMessageIngressKind,
};
pub use polar_inhouse_model::{
    validate_role_catalog, InhouseBackendProfileRecord, InhousePolarInstanceRecord,
    InhousePolarRecord, InhousePolarRoleBinding, InhousePolarSystemRole, InhouseRoleRecord,
    InstanceStatus, RoleContractError, ThreadBindingStatus, ThreadRuntimeBindingRecord,
};
pub use prompt::{
    InhousePromptLayer, PackagedPromptMaterial, PromptActivationRecord, PromptAssetClass,
    PromptAssetInput, PromptAssetKey, PromptAssetRecord, PromptAssetRef, PromptAssetRevision,
    PromptBackend, PromptBindingRecord, PromptBindingSpec, PromptBindingTargetKind, PromptChange,
    PromptContractError, PromptLifecycle, PromptTarget,
};
pub use speech::{SpeechDecisionMode, SpeechRequestRecord, SpeechRequestStatus};
pub use system_tools::{SystemToolExecutionClass, SystemToolSpec};
pub use telegram::{
    ChatPendingRequestSnapshot, TelegramPendingChatBridgeRequestRecord, TelegramSessionRecord,
    TelegramTaskBotBindingRecord, TelegramWorkspaceRecord,
};
pub use thread::ThreadRecord;
pub use thread_participant::{ThreadParticipantKind, ThreadParticipantRecord};
pub use tool_authz::ToolFamily;

pub use polar_cas::source_hint::SourceHint as AttachmentSourceHint;
