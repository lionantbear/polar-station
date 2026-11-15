//! Native EROFS *logical* contracts. No block addresses, inode numbers or
//! mount paths belong here. Serialized records are descriptions, not witnesses.
use crate::{
    canonical::canonical_json_bytes,
    projection::FileOwner,
    source::{LocationId, LocationSpec, PublicBinding},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const NATIVE_PROJECTION_SCHEMA: &str = "polar.cas.native-projection.v1";
pub const EROFS_ADAPTER: &str = "erofs";
pub const NATIVE_PACK_PROFILE: &str = "polar.cas.erofs-pack.native.v1";
pub const MAX_NATIVE_PACKS: usize = 64;

/// An immutable namespace backed by a CAS object. Not proof of local existence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErofsPackBinding {
    pub image: ContentRef,
    pub profile: String,
}
impl ErofsPackBinding {
    pub fn native(image: ContentRef) -> Result<Self> {
        let value = Self {
            image,
            profile: NATIVE_PACK_PROFILE.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        image_descriptor(&self.image)?;
        if self.profile != NATIVE_PACK_PROFILE {
            return Err(Error::UnsupportedCapability);
        }
        Ok(())
    }
    pub fn to_location(&self, id: LocationId) -> Result<LocationSpec> {
        self.validate()?;
        let binding = PublicBinding::from([
            ("image".into(), self.image.id.to_string()),
            ("size_bytes".into(), self.image.size_bytes.to_string()),
            ("profile".into(), self.profile.clone()),
        ]);
        let location = LocationSpec {
            id,
            adapter: EROFS_ADAPTER.into(),
            storage_identity: format!("erofs:{}", self.image.id),
            binding,
            credential_ref: None,
        };
        location.validate()?;
        Ok(location)
    }
    pub fn from_location(location: &LocationSpec) -> Result<Self> {
        location.validate()?;
        if location.adapter != EROFS_ADAPTER
            || location.credential_ref.is_some()
            || location.binding.len() != 3
        {
            return Err(Error::Invalid("invalid EROFS location binding".into()));
        }
        let get = |key: &str| {
            location
                .binding
                .get(key)
                .ok_or_else(|| Error::Invalid(format!("missing EROFS binding {key}")))
        };
        let id = ObjectId::try_from(get("image")?.clone())?;
        let size_text = get("size_bytes")?;
        let size_bytes = size_text
            .parse::<u64>()
            .map_err(|_| Error::Invalid("invalid image size".into()))?;
        if size_text != &size_bytes.to_string()
            || location.storage_identity != format!("erofs:{id}")
        {
            return Err(Error::Invalid("noncanonical EROFS binding".into()));
        }
        let value = Self {
            image: ContentRef { id, size_bytes },
            profile: get("profile")?.clone(),
        };
        value.validate()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeViewMode {
    ReadOnly,
    CopyOnWrite,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTreeProfile {
    pub owner: FileOwner,
    pub mode: NativeViewMode,
}
impl NativeTreeProfile {
    pub fn directory_mode(&self) -> u32 {
        match self.mode {
            NativeViewMode::ReadOnly => 0o555,
            NativeViewMode::CopyOnWrite => 0o755,
        }
    }
    pub fn file_mode(&self, executable: bool) -> u32 {
        match (self.mode, executable) {
            (NativeViewMode::ReadOnly, false) => 0o444,
            (NativeViewMode::ReadOnly, true) => 0o555,
            (NativeViewMode::CopyOnWrite, false) => 0o644,
            (NativeViewMode::CopyOnWrite, true) => 0o755,
        }
    }
}

/// Durable, portable dependency closure. Packs are ordered; NEVER normalize
/// the order after M was compiled. No host-local state or physical mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProjection {
    pub schema: String,
    /// OUTPUT TreeManifest root after ProjectionLayout selection/renaming.
    pub tree: ContentRef,
    pub meta: ContentRef,
    pub packs: Vec<ContentRef>,
    pub profile: NativeTreeProfile,
    pub compiler: String,
}
impl NativeProjection {
    pub fn validate(&self) -> Result<()> {
        if self.schema != NATIVE_PROJECTION_SCHEMA
            || self.compiler.is_empty()
            || self.compiler.len() > 512
            || self.compiler.chars().any(char::is_control)
            || self.tree.size_bytes > i64::MAX as u64
        {
            return Err(Error::Invalid(
                "invalid native projection descriptor".into(),
            ));
        }
        image_descriptor(&self.meta)?;
        if self.packs.len() > MAX_NATIVE_PACKS {
            return Err(Error::TooLarge);
        }
        let mut seen = BTreeSet::new();
        for pack in &self.packs {
            image_descriptor(pack)?;
            if pack.id == self.meta.id || !seen.insert(pack.id.to_string()) {
                return Err(Error::Invalid("duplicate or recursive native pack".into()));
            }
        }
        Ok(())
    }
    /// Compilation lookup key, NOT a new CAS object or a mount identity.
    /// Excludes the result M; identical compile inputs cannot bind another M.
    pub fn compilation_key(&self) -> Result<String> {
        self.validate()?;
        let bytes = canonical_json_bytes(&(
            &self.schema,
            &self.tree,
            &self.packs,
            &self.profile,
            &self.compiler,
        ))
        .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(ObjectId::for_bytes(&bytes).to_string())
    }
    /// Immutable storage closure, excluding semantic Tree history. Does not
    /// create a lease; the Host must acquire guards before preparing any image.
    pub fn images(&self) -> impl Iterator<Item = &ContentRef> {
        std::iter::once(&self.meta).chain(self.packs.iter())
    }
}

/// In-process adapter witness. No Deserialize; never accept it from a DTO.
#[derive(Clone, Debug)]
pub struct VerifiedNativeProjection(NativeProjection);
impl VerifiedNativeProjection {
    /// Only a compiler/verifier that checked the exact Tree, metadata and ALL
    /// member bytes against the ordered immutable packs may call this method.
    /// ContentRef hashes alone do not prove that a Meta represents a Tree.
    pub fn from_trusted_adapter(record: NativeProjection) -> Result<Self> {
        record.validate()?;
        Ok(Self(record))
    }
    pub fn record(&self) -> &NativeProjection {
        &self.0
    }
    pub fn into_record(self) -> NativeProjection {
        self.0
    }
}

fn image_descriptor(content: &ContentRef) -> Result<()> {
    if content.id.algorithm() != DigestAlgorithm::Sha256 {
        return Err(Error::MissingDigest);
    }
    if content.size_bytes < 4096
        || content.size_bytes > i64::MAX as u64
        || !content.size_bytes.is_multiple_of(4096)
    {
        return Err(Error::Invalid(
            "native image must be nonempty 4KiB aligned".into(),
        ));
    }
    Ok(())
}

/// Verified membership facts produced by the native adapter. Not a network DTO.
#[derive(Clone, Debug)]
pub struct VerifiedPackedMembers {
    binding: ErofsPackBinding,
    members: Vec<crate::verification::VerifiedContent>,
}
impl VerifiedPackedMembers {
    /// A trusted adapter must either decode and verify every member in this
    /// exact image, or consume completed construction evidence that the same
    /// verified source bytes were delivered to its encoder, with namespace
    /// revalidation and the finalized image identity. A trusted Catalog adapter
    /// may also restore those previously admitted exact (Pack, member) facts,
    /// after checking current Ready membership and immutable backing bindings.
    /// Ordinary opaque image admission, serialized claims, or hashing unrelated
    /// bytes never prove pack membership.
    pub fn from_trusted_adapter(
        binding: ErofsPackBinding,
        members: Vec<crate::verification::VerifiedContent>,
    ) -> Result<Self> {
        binding.validate()?;
        if members.len() > crate::catalog::MAX_REGISTER_SOURCES {
            return Err(Error::TooLarge);
        }
        let mut seen = BTreeSet::new();
        for content in &members {
            let id = content
                .ids()
                .iter()
                .find(|id| id.algorithm() == DigestAlgorithm::Sha256)
                .ok_or(Error::MissingDigest)?;
            if !seen.insert(id.to_string()) || id == &binding.image.id {
                return Err(Error::Conflict);
            }
        }
        Ok(Self { binding, members })
    }
    pub fn binding(&self) -> &ErofsPackBinding {
        &self.binding
    }
    pub fn members(&self) -> &[crate::verification::VerifiedContent] {
        &self.members
    }
}

pub mod transfer;

/// Ordinary-path standalone EROFS representation. It deliberately does NOT use
/// the digest-pack profile or the shared digest-member index format.
pub const FILESYSTEM_EROFS_ADAPTER: &str = "erofs-filesystem";
pub const FILESYSTEM_EROFS_PROFILE: &str = "polar.cas.erofs-filesystem.v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemPackBinding {
    pub image: ContentRef,
    pub tree: ContentRef,
}
impl FilesystemPackBinding {
    pub fn to_location(&self) -> Result<LocationSpec> {
        image_descriptor(&self.image)?;
        self.tree.id.require_sha256()?;
        let value = LocationSpec {
            id: LocationId::new(format!("filesystem-{}", self.image.id.hex()))?,
            adapter: FILESYSTEM_EROFS_ADAPTER.into(),
            storage_identity: format!("erofs-filesystem:{}", self.image.id),
            binding: PublicBinding::from([
                ("image".into(), self.image.id.to_string()),
                ("image_bytes".into(), self.image.size_bytes.to_string()),
                ("tree".into(), self.tree.id.to_string()),
                ("tree_bytes".into(), self.tree.size_bytes.to_string()),
                ("profile".into(), FILESYSTEM_EROFS_PROFILE.into()),
            ]),
            credential_ref: None,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_location(location: &LocationSpec) -> Result<Self> {
        location.validate()?;
        let get = |key: &str| location.binding.get(key).ok_or(Error::Integrity);
        let number = |key: &str| -> Result<u64> {
            let text = get(key)?;
            let n = text.parse::<u64>().map_err(|_| Error::Integrity)?;
            if text != &n.to_string() {
                return Err(Error::Integrity);
            }
            Ok(n)
        };
        if location.adapter != FILESYSTEM_EROFS_ADAPTER
            || location.credential_ref.is_some()
            || location.binding.len() != 5
            || get("profile")? != FILESYSTEM_EROFS_PROFILE
        {
            return Err(Error::UnsupportedCapability);
        }
        let value = Self {
            image: ContentRef {
                id: ObjectId::try_from(get("image")?.clone())?,
                size_bytes: number("image_bytes")?,
            },
            tree: ContentRef {
                id: ObjectId::try_from(get("tree")?.clone())?,
                size_bytes: number("tree_bytes")?,
            },
        };
        if &value.to_location()? != location {
            return Err(Error::Integrity);
        }
        Ok(value)
    }
}

/// The same legacy native compilation-key encoding, without fabricating an
/// output ContentRef before an output exists.
pub fn native_compilation_key(
    tree: &ContentRef,
    packs: &[ContentRef],
    profile: &NativeTreeProfile,
    compiler: &str,
) -> Result<String> {
    if compiler.is_empty()
        || compiler.len() > 512
        || compiler.chars().any(char::is_control)
        || packs.len() > MAX_NATIVE_PACKS
    {
        return Err(Error::Invalid("invalid compilation key inputs".into()));
    }
    let bytes = canonical_json_bytes(&(NATIVE_PROJECTION_SCHEMA, tree, packs, profile, compiler))
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(ObjectId::for_bytes(&bytes).to_string())
}

/// Adapter-owned opaque member locator. A filesystem name may contain `?` or
/// `#`, while public source locators deliberately reject URL query/fragment
/// syntax. Hashing the normalized path avoids conflating those namespaces.
/// This is NOT the member content hash: the admitted image/tree binding owns
/// the path-to-content association, and readers rebuild the same mapping.
pub fn filesystem_member_locator(path: &str) -> Result<crate::source::SourceLocator> {
    if path.is_empty() || path.len() > 8192 {
        return Err(Error::Invalid("invalid filesystem member path".into()));
    }
    for name in path.split('/') {
        crate::manifest::validate_name(name)?;
    }
    let key = format!("member/{}", ObjectId::for_bytes(path.as_bytes()).hex());
    let locator = crate::source::SourceLocator {
        key,
        revision: None,
    };
    locator.validate()?;
    Ok(locator)
}
