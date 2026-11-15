//! Optional additive Catalog capability; no second replica/member store.
use crate::{
    hybrid_fs::{HybridProjection, PreferredHybridProjection, VerifiedHybridProjection},
    ContentRef, Result,
};
use async_trait::async_trait;
#[async_trait]
pub trait HybridFsCatalog: Send + Sync {
    async fn register_hybrid_projection(
        &self,
        witness: &VerifiedHybridProjection,
    ) -> Result<String>;
    async fn hybrid_projection(&self, key: &str) -> Result<HybridProjection>;
    async fn list_hybrid_variants(
        &self,
        scope: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<String>>;
    async fn preferred_hybrid_projection(
        &self,
        scope: &str,
    ) -> Result<Option<PreferredHybridProjection>>;
    /// None expects no pointer; Some(n) expects exactly revision n. Scoped to
    /// this Catalog, not a claim of cross-host mutable-pointer consensus.
    async fn prefer_hybrid_projection(
        &self,
        key: &str,
        expected: Option<u64>,
    ) -> Result<PreferredHybridProjection>;
    async fn list_hybrid_pack_dependents(
        &self,
        pack: &ContentRef,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<String>>;
}
