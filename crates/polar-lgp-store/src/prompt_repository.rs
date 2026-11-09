//! Narrow prompt persistence facet. No prompt selection or mutation policy here.
//! The eventual SQLite adapter uses the existing LGP database, not another store.

use anyhow::Result;
use async_trait::async_trait;
use polar_lgp_model::{PromptAssetKey, PromptAssetRevision, PromptBindingRecord, PromptTarget};

/// Reads share one consistent repository snapshot for the entire callback.
pub trait PromptRead {
    fn asset(&self, key: &PromptAssetKey) -> Result<Option<PromptAssetRevision>>;
    fn binding(&self, id: &str) -> Result<Option<PromptBindingRecord>>;
    fn bindings_for_target(&self, target: &PromptTarget) -> Result<Vec<PromptBindingRecord>>;
    fn bindings_using_asset(&self, key: &PromptAssetKey) -> Result<Vec<PromptBindingRecord>>;
    fn bindings_replacing(&self, id: &str) -> Result<Vec<PromptBindingRecord>>;
}

/// Delegate to the target's domain; never duplicate its catalogue in prompt storage.
/// LGP targets must include changes staged in this same transaction. Other domain
/// targets come from the caller's resolved domain view, not a global transaction.
pub trait PromptTargetLookup {
    fn target_exists(&self, target: &PromptTarget) -> Result<bool>;
}

/// Resolve external domain identities afresh for one mutation, before entering
/// the storage writer. Scene existence remains transaction-local.
#[async_trait]
pub trait PromptTargetResolver: Send + Sync {
    async fn resolve_targets(
        &self,
        targets: &[PromptTarget],
    ) -> Result<std::sync::Arc<dyn PromptTargetLookup + Send + Sync>>;
}

/// Implementations must permit final-state reference validation within a batch
/// (for SQLite, deferred foreign keys). These are storage operations, not public
/// use cases: consumers call PromptService for mutation policy.
pub trait PromptTransaction: PromptRead + PromptTargetLookup {
    fn insert_asset(&mut self, asset: &PromptAssetRevision) -> Result<()>;
    fn put_binding(&mut self, binding: &PromptBindingRecord) -> Result<()>;
    fn delete_binding(&mut self, id: &str) -> Result<()>;
    fn delete_asset(&mut self, key: &PromptAssetKey) -> Result<()>;
}

/// Already materialized scene records. Prompt mutations remain owned by the
/// service callback, so this storage contract does not duplicate their DTOs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSceneBatch {
    pub channels: Vec<polar_storage_core::RecordWrite<polar_lgp_model::ChannelRecord>>,
    pub threads: Vec<polar_storage_core::RecordWrite<polar_lgp_model::ThreadRecord>>,
    /// Complete overrides for submitted threads. Omission clears an override
    /// only if its thread write is applied, never when the write is skipped.
    pub thread_attachment_policies: Vec<crate::ThreadAttachmentPolicyOverride>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSceneReceipt {
    pub channels: Vec<polar_storage_core::RecordWriteOutcome>,
    pub threads: Vec<polar_storage_core::RecordWriteOutcome>,
    pub prompts: PromptWriteReceipt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptWriteReceipt {
    pub assets: Vec<polar_storage_core::RecordWriteOutcome>,
    pub bindings: Vec<polar_storage_core::RecordWriteOutcome>,
}

pub type PromptSceneMutation =
    Box<dyn FnOnce(&mut dyn PromptTransaction) -> Result<PromptWriteReceipt> + Send + 'static>;

/// Object-safe scene transaction port for Bundle. The implementation applies
/// scene records, invokes the mutation, and commits once. Any error rolls back
/// the entire operation. External domain targets are resolved for this request.
#[async_trait]
pub trait PromptSceneStore: Send + Sync {
    async fn apply_prompt_scene(
        &self,
        scene: PromptSceneBatch,
        targets: std::sync::Arc<dyn PromptTargetLookup + Send + Sync>,
        mutation: PromptSceneMutation,
    ) -> Result<PromptSceneReceipt>;
}

/// Generic callback result avoids erased domain outputs or a second batch engine.
/// No side effects may escape a failed transaction callback or failed commit.
/// Owned callbacks/results match the existing SQLite worker's Send + 'static
/// queue contract; adapters must not borrow caller stacks or open another DB.
#[async_trait]
pub trait PromptRepository: Send + Sync {
    async fn read<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&dyn PromptRead) -> Result<T> + Send + 'static;

    async fn transaction<T, F>(&self, targets: Vec<PromptTarget>, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut dyn PromptTransaction) -> Result<T> + Send + 'static;
}

#[async_trait]
impl<R: PromptRepository + ?Sized> PromptRepository for std::sync::Arc<R> {
    async fn read<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&dyn PromptRead) -> Result<T> + Send + 'static,
    {
        (**self).read(operation).await
    }
    async fn transaction<T, F>(&self, targets: Vec<PromptTarget>, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut dyn PromptTransaction) -> Result<T> + Send + 'static,
    {
        (**self).transaction(targets, operation).await
    }
}

#[async_trait]
impl<R: PromptRepository + ?Sized> PromptRepository for &R {
    async fn read<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&dyn PromptRead) -> Result<T> + Send + 'static,
    {
        (**self).read(operation).await
    }
    async fn transaction<T, F>(&self, targets: Vec<PromptTarget>, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut dyn PromptTransaction) -> Result<T> + Send + 'static,
    {
        (**self).transaction(targets, operation).await
    }
}
