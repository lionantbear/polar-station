//! Shared existing PtrHash navigator; record formats keep their own semantics.
use super::search::perfect::DiskHasher;
use polar_cas::{DigestAlgorithm, ObjectId};
use ptr_hash::FastPtrHash;
pub(super) type Key = [u8; 33];
pub(super) type Phf = FastPtrHash<DiskHasher, Key>;

/// The serialized type/layout header is identical for every local navigator.
/// Generate it once using epserde itself, rather than hashing the same Rust
/// type and layout twice for every partition we open.
pub(super) fn open(bytes: &[u8]) -> polar_cas::Result<Phf> {
    use epserde::{
        deser::{check_header, DeserInner, ReadNoStd, ReaderWithPos},
        ser::{write_header, SerInner, WriterWithPos},
    };
    static HEADER: std::sync::OnceLock<std::result::Result<Vec<u8>, String>> =
        std::sync::OnceLock::new();
    let header = HEADER
        .get_or_init(|| {
            let mut bytes = Vec::new();
            write_header::<<Phf as SerInner>::SerType>(&mut WriterWithPos::new(&mut bytes))
                .map_err(|e| e.to_string())?;
            Ok(bytes)
        })
        .as_ref()
        .map_err(|e| polar_cas::Error::Backend(e.clone()))?;
    let mut cursor = std::io::Cursor::new(bytes);
    let mut input = ReaderWithPos::new(&mut cursor);
    let decoded = (|| {
        if bytes.starts_with(header) {
            // Preserve epserde's absolute body alignment without allocating a
            // temporary type-name String for each matching partition.
            let mut discard = [0u8; 256];
            let mut remaining = header.len();
            while remaining > 0 {
                let count = remaining.min(discard.len());
                ReadNoStd::read_exact(&mut input, &mut discard[..count])?;
                remaining -= count;
            }
        } else {
            check_header::<Phf>(&mut input)?;
        }
        // SAFETY: same native publication and ownership contract as the prior
        // deserialize_full call; the identical upstream header is checked above.
        unsafe { Phf::_deser_full_inner(&mut input) }
    })();
    decoded.map_err(|e| polar_cas::Error::Backend(format!("PHF format: {e}")))
}
pub(super) fn identity(id: &ObjectId) -> Key {
    let mut key = [0; 33];
    key[..32].copy_from_slice(id.digest_bytes());
    key[32] = u8::from(id.algorithm() == DigestAlgorithm::Blake3);
    key
}


