use crate::{AudioBlock, Change, Recording, SaveReceipt};
use anyhow::Result;
use async_trait::async_trait;

/// Implementations are scoped to an authenticated owner by the Host, never by
/// caller-selected paths. All mutations atomically commit before returning.
#[async_trait]
pub trait DictateStore: Send + Sync {
    async fn create(&self, recording: Recording) -> Result<Recording>;
    async fn get(&self, id: &str) -> Result<Recording>;
    async fn usage(&self) -> Result<crate::StorageUsage>;
    async fn list(&self, offset: u32, limit: u32) -> Result<Vec<Recording>>;
    async fn append(&self, block: AudioBlock) -> Result<SaveReceipt>;
    async fn change(&self, id: &str, change: Change) -> Result<Recording>;
    async fn audio(&self, id: &str, start_sample: u64, sample_count: u64) -> Result<Vec<u8>>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn text_create(&self, record: crate::TextEditRecord) -> Result<crate::TextEditRecord>;
    async fn text_finish(
        &self,
        id: &str,
        result: Result<String, String>,
    ) -> Result<crate::TextEditRecord>;
    async fn item(&self, id: &str) -> Result<crate::HistoryItem>;
    async fn item_change(&self, id: &str, change: crate::ItemChange) -> Result<crate::HistoryItem>;
    async fn history(
        &self,
        cursor: Option<crate::HistoryCursor>,
        limit: u32,
    ) -> Result<crate::HistoryPage>;
    async fn cleanup(
        &self,
        now_ms: u64,
        policy: crate::RetentionPolicy,
        apply: bool,
    ) -> Result<crate::CleanupReport>;
    async fn import_legacy(&self, entries: Vec<crate::LegacyInput>) -> Result<crate::ImportReport>;
    async fn finish_legacy_import(&self) -> Result<()>;
    async fn import_issues(&self, offset: u32, limit: u32) -> Result<Vec<crate::LegacyInput>>;
    async fn pending(&self) -> Result<Vec<Recording>>;
    async fn recover(&self) -> Result<usize>;
    async fn expire(&self, now_ms: u64) -> Result<usize>;
}
