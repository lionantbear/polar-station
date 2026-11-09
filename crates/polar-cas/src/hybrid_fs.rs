//! Hybrid view descriptions. Content identity remains ContentRef; these records
//! contain no host paths, offsets, inode numbers or kernel device identifiers.
use crate::{
    canonical::canonical_json_bytes, projection::FileOwner, ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const HYBRID_SCHEMA: &str = "polar.cas.hybrid-projection.v2";
pub const HYBRID_ABI: &str = "erofs-hybrid-v2";
pub const MAX_HYBRID_FILES: usize = 1_000_000;

/// Readonly/COW is a mount property. The same logical inode profile serves both.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridTreeProfile {
    pub owner: FileOwner,
}
impl HybridTreeProfile {
    pub fn directory_mode(&self) -> u32 {
        0o755
    }
    pub fn file_mode(&self, executable: bool) -> u32 {
        if executable {
            0o755
        } else {
            0o644
        }
    }
    pub fn key(&self) -> Result<String> {
        fingerprint(&(HYBRID_ABI, self))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridMapping {
    pub content: ContentRef,
    pub pack: Option<ContentRef>,
}
/// Frozen compile input, not an admission capability. One decision per unique
/// content sorted by canonical content ID. None means a pathname redirect.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridCompilePlan {
    pub tree: ContentRef,
    pub profile: HybridTreeProfile,
    pub mappings: Vec<HybridMapping>,
    pub native_packs: Vec<ContentRef>,
}
impl HybridCompilePlan {
    pub fn validate(&self) -> Result<()> {
        if self.mappings.len() > MAX_HYBRID_FILES
            || self.native_packs.len() > 64
            || self.tree.size_bytes > i64::MAX as u64
        {
            return Err(Error::TooLarge);
        }
        let mut packs = BTreeSet::new();
        for p in &self.native_packs {
            image(p)?;
            if !packs.insert(p.id.to_string()) {
                return Err(Error::Conflict);
            }
        }
        let mut previous: Option<String> = None;
        let mut used = BTreeSet::new();
        for m in &self.mappings {
            m.content.id.require_sha256()?;
            if m.content.size_bytes > i64::MAX as u64 {
                return Err(Error::TooLarge);
            }
            let id = m.content.id.to_string();
            if previous.as_ref().is_some_and(|v| v >= &id) {
                return Err(Error::Conflict);
            }
            previous = Some(id);
            if let Some(p) = &m.pack {
                if !self.native_packs.iter().any(|v| v == p) {
                    return Err(Error::Integrity);
                }
                used.insert(p.id.to_string());
            }
        }
        if packs != used {
            return Err(Error::Invalid("unused native pack in Hybrid plan".into()));
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        mapping_fingerprint(
            &self.tree,
            &self.profile,
            &self.native_packs,
            &self.mappings,
        )
    }
    pub fn redirects(&self) -> impl Iterator<Item = &ContentRef> {
        self.mappings
            .iter()
            .filter(|m| m.pack.is_none())
            .map(|m| &m.content)
    }
}
/// T -> zero or many immutable variants. Changing the mapping plan changes the
/// compile key; changing the optional preferred pointer does not mutate M/P/T.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridProjection {
    pub schema: String,
    pub tree: ContentRef,
    pub meta: ContentRef,
    pub native_packs: Vec<ContentRef>,
    pub profile: HybridTreeProfile,
    pub plan_fingerprint: String,
    pub compiler: String,
    pub native_file_count: u64,
    pub redirect_file_count: u64,
}
impl HybridProjection {
    pub fn validate(&self) -> Result<()> {
        if self.schema != HYBRID_SCHEMA
            || self.compiler.is_empty()
            || self.compiler.len() > 512
            || self.compiler.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid Hybrid variant".into()));
        }
        ObjectId::try_from(self.plan_fingerprint.clone())?.require_sha256()?;
        image(&self.meta)?;
        if self.tree.size_bytes > i64::MAX as u64
            || self.native_packs.len() > 64
            || self
                .native_file_count
                .saturating_add(self.redirect_file_count)
                > MAX_HYBRID_FILES as u64
        {
            return Err(Error::TooLarge);
        }
        let mut seen = BTreeSet::new();
        for p in &self.native_packs {
            image(p)?;
            if p.id == self.meta.id || !seen.insert(p.id.to_string()) {
                return Err(Error::Conflict);
            }
        }
        if (self.native_file_count == 0) != self.native_packs.is_empty() {
            return Err(Error::Invalid(
                "native file/dependency count mismatch".into(),
            ));
        }
        Ok(())
    }
    pub fn has_redirects(&self) -> bool {
        self.redirect_file_count != 0
    }
    pub fn compilation_key(&self) -> Result<String> {
        self.validate()?;
        fingerprint(&(
            &self.schema,
            &self.tree,
            &self.profile,
            &self.plan_fingerprint,
            &self.native_packs,
            &self.compiler,
        ))
    }
    pub fn scope_key(&self) -> Result<String> {
        self.validate()?;
        fingerprint(&(HYBRID_ABI, &self.tree, &self.profile))
    }
    pub fn images(&self) -> impl Iterator<Item = &ContentRef> {
        std::iter::once(&self.meta).chain(self.native_packs.iter())
    }
}
/// A trusted adapter witness (no Deserialize). The Catalog must additionally
/// require admitted T/M/P; this is not a byte-availability or mount witness.
#[derive(Clone, Debug)]
pub struct VerifiedHybridProjection(HybridProjection);
impl VerifiedHybridProjection {
    pub fn from_trusted_adapter(record: HybridProjection) -> Result<Self> {
        record.validate()?;
        Ok(Self(record))
    }
    pub fn record(&self) -> &HybridProjection {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreferredHybridProjection {
    pub scope_key: String,
    pub compilation_key: String,
    pub revision: u64,
}
fn fingerprint<T: Serialize>(v: &T) -> Result<String> {
    let b = canonical_json_bytes(v).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(ObjectId::for_bytes(&b).to_string())
}
fn image(v: &ContentRef) -> Result<()> {
    v.id.require_sha256()?;
    if v.size_bytes < 4096 || !v.size_bytes.is_multiple_of(4096) || v.size_bytes > i64::MAX as u64 {
        return Err(Error::Invalid("image must be 4KiB aligned".into()));
    }
    Ok(())
}

/// Normalized mapping fingerprint can be reconstructed from M + Tree, without a
/// second per-file SQL map or a retained host-specific compiler request. An
/// empty native file has no encoded extent/slot: its payload binding is therefore
/// normalized to native-empty. Ordered device dependencies remain in this hash.
/// Callers validate admission/namespace separately; this is only a hash function.
pub fn mapping_fingerprint(
    tree: &ContentRef,
    profile: &HybridTreeProfile,
    packs: &[ContentRef],
    mappings: &[HybridMapping],
) -> Result<String> {
    let mut normalized = Vec::with_capacity(mappings.len());
    let mut previous: Option<String> = None;
    for m in mappings {
        m.content.id.require_sha256()?;
        let id = m.content.id.to_string();
        if previous.as_ref().is_some_and(|s| s >= &id) {
            return Err(Error::Integrity);
        }
        previous = Some(id);
        let kind = match &m.pack {
            None => "redirect",
            Some(_) if m.content.size_bytes == 0 => "native-empty",
            Some(_) => "native",
        };
        let source = if kind == "native" {
            m.pack.as_ref()
        } else {
            None
        };
        normalized.push((&m.content, kind, source));
    }
    fingerprint(&(HYBRID_ABI, tree, profile, packs, normalized))
}

pub mod transfer;
