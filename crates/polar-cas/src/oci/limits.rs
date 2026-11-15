//! Durable OCI decode/render budgets, shared by contracts and adapters.
use crate::{manifest::bundle::BundleLimits, Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciLayerLimits {
    pub max_layers: u32,
    pub max_blob_bytes: u64,
    pub max_total_blob_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_total_expanded_bytes: u64,
    /// Base-2 log of the decoder's maximum window, separate from output size.
    pub max_zstd_window_log: u32,
}
impl Default for OciLayerLimits {
    fn default() -> Self {
        Self {
            max_layers: 1024,
            max_blob_bytes: 8 << 30,
            max_total_blob_bytes: 32 << 30,
            max_expanded_bytes: 16 << 30,
            max_total_expanded_bytes: 64 << 30,
            max_zstd_window_log: 27,
        }
    }
}
impl OciLayerLimits {
    pub fn validate(self) -> Result<()> {
        if self.max_layers == 0
            || self.max_blob_bytes == 0
            || self.max_total_blob_bytes == 0
            || self.max_expanded_bytes == 0
            || self.max_total_expanded_bytes == 0
            || !(10..=30).contains(&self.max_zstd_window_log)
        {
            return Err(Error::Invalid("invalid OCI layer limits".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciArchiveLimits {
    pub max_entries: usize,
    pub max_header_bytes_per_entry: u64,
    pub max_total_header_bytes: u64,
    pub max_file_bytes: u64,
    pub max_total_file_bytes: u64,
    pub max_path_bytes: usize,
    pub max_total_path_bytes: usize,
    pub max_depth: usize,
}
impl Default for OciArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_header_bytes_per_entry: 1024 * 1024,
            max_total_header_bytes: 64 * 1024 * 1024,
            max_file_bytes: 16 << 30,
            max_total_file_bytes: 64 << 30,
            max_path_bytes: 4096,
            max_total_path_bytes: 64 * 1024 * 1024,
            max_depth: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciRenderLimits {
    pub archives: OciArchiveLimits,
    /// Conservative logical payload bound: all input file bytes multiplied
    /// by (hardlink count + 1), covering upstream's per-promotion clones.
    /// Not allocator capacity/RSS; headers, paths and entries have own bounds.
    pub max_buffered_file_bytes: u64,
    pub max_tar_bytes: u64,
    pub tree: BundleLimits,
}
impl Default for OciRenderLimits {
    fn default() -> Self {
        Self {
            archives: Default::default(),
            max_buffered_file_bytes: 512 << 20,
            max_tar_bytes: 64 << 30,
            tree: Default::default(),
        }
    }
}

impl OciArchiveLimits {
    pub fn validate(self) -> Result<()> {
        if self.max_header_bytes_per_entry == 0
            || self.max_total_header_bytes == 0
            || self.max_path_bytes == 0
        {
            return Err(Error::Invalid("invalid OCI archive limits".into()));
        }
        Ok(())
    }
}
impl OciRenderLimits {
    pub fn validate(self) -> Result<()> {
        self.archives.validate()?;
        if self.max_tar_bytes == 0 {
            return Err(Error::Invalid(
                "explicit bounded OCI render staging required".into(),
            ));
        }
        Ok(())
    }
}
