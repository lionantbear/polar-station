//! Stable active-directory identity. This description never hashes live upper
//! bytes and contains no Host paths, mount IDs, credentials or branch heads.
use crate::{invalid, reference::valid_key, NodeRef, Result};
use polar_cas::ContentRef;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveBinding {
    pub instance: Uuid,
    pub owner: Uuid,
    pub generation: u64,
    pub source: NodeRef,
    pub lower_tree: ContentRef,
    pub lower_image: ContentRef,
}
impl ActiveBinding {
    pub fn validate(&self) -> Result<()> {
        if self.instance.is_nil()
            || self.owner.is_nil()
            || self.generation == 0
            || self.generation > i64::MAX as u64
            || !valid_key(&self.source.node)
        {
            return Err(invalid(
                "active",
                "invalid instance/owner/generation/source",
            ));
        }
        for c in [&self.source.document, &self.lower_tree, &self.lower_image] {
            c.id.require_sha256()?;
            if c.size_bytes > i64::MAX as u64 {
                return Err(invalid("active", "content length out of range"));
            }
        }
        if self.lower_image.size_bytes < 4096 || !self.lower_image.size_bytes.is_multiple_of(4096) {
            return Err(invalid("active", "standalone lower image alignment"));
        }
        Ok(())
    }
}
