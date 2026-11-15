//! The original fixed physical-position row codec, shared by both navigators.
use polar_cas::{
    shared::{LocatedContent, PhysicalCandidate},
    ContentRef, Error, ObjectId, Result,
};
pub(super) const PRESENT: u64 = 1 << 62;
pub(super) const ALGORITHM: u64 = 1 << 63;

#[inline(always)]
pub(super) fn decode(
    id: ObjectId,
    size: u64,
    pack: u64,
    dictionary: &[u8],
) -> Result<LocatedContent> {
    let location = if pack == 0 {
        PhysicalCandidate::Loose
    } else {
        let offset = usize::try_from(pack - 1)
            .map_err(|_| Error::Integrity)?
            .checked_mul(40)
            .ok_or(Error::Integrity)?;
        let row = dictionary
            .get(offset..offset.checked_add(40).ok_or(Error::Integrity)?)
            .ok_or(Error::Integrity)?;
        PhysicalCandidate::PackMember {
            pack: ContentRef {
                id: ObjectId::from_sha256(row[..32].try_into().map_err(|_| Error::Integrity)?),
                size_bytes: u64::from_le_bytes(row[32..].try_into().map_err(|_| Error::Integrity)?),
            },
        }
    };
    Ok(LocatedContent {
        content: ContentRef {
            id,
            size_bytes: size,
        },
        location,
    })
}

/// Canonical complete value for exactly one local position, with no remote or
/// writer extensions. Both Catalog publication and compact-slot hydration use
/// this existing PCASPOS1 representation.
pub(crate) fn single_local_value(row: &LocatedContent) -> Result<Vec<u8>> {
    let mut writer = super::binary::Encoder::new(b"PCASPOS1", 103)?;
    writer.count(1)?;
    writer.content(&row.content)?;
    match &row.location {
        PhysicalCandidate::Loose => writer.byte(0)?,
        PhysicalCandidate::PackMember { pack } => {
            writer.byte(1)?;
            writer.content(pack)?;
        }
    }
    writer.count(0)?;
    writer.count(0)?;
    Ok(writer.finish())
}

/// A fixed opaque value with an exact context. The domain defines its meaning.
pub(crate) type FixedExtension = ([u8; 32], [u8; 16]);

pub(crate) fn single_local_extended(
    row: &LocatedContent,
    (context, value): &FixedExtension,
) -> Result<Vec<u8>> {
    let mut bytes = single_local_value(row)?;
    bytes[..8].copy_from_slice(b"PCASPOS2");
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(context);
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(value);
    bytes.extend_from_slice(&offset.to_le_bytes());
    Ok(bytes)
}

/// Recognize only the exact canonical single-position/single-extension form.
/// All other values retain their full portable representation.
pub(crate) fn compact_extension(
    row: &LocatedContent,
    bytes: &[u8],
) -> Result<Option<FixedExtension>> {
    if !bytes.starts_with(b"PCASPOS2") || bytes.len() < 64 {
        return Ok(None);
    }
    let base = single_local_value(row)?;
    if bytes.len() != base.len() + 64
        || bytes[8..base.len()] != base[8..]
        || bytes[base.len()..base.len() + 4] != 1u32.to_le_bytes()
        || bytes[base.len() + 36..base.len() + 40] != 16u32.to_le_bytes()
        || bytes[bytes.len() - 8..] != (base.len() as u64).to_le_bytes()
    {
        return Ok(None);
    }
    Ok(Some((
        bytes[base.len() + 4..base.len() + 36]
            .try_into()
            .map_err(|_| Error::Integrity)?,
        bytes[base.len() + 40..base.len() + 56]
            .try_into()
            .map_err(|_| Error::Integrity)?,
    )))
}
