//! Portable directory of immutable segment ranges. Legacy JSON is read only.
use super::*;
use crate::shared_index::binary::{decode_value, encode_value, Decoder, Encoder};
const MAGIC: &[u8] = b"PCASROT2";
const LIMIT: usize = 4 << 20;

pub(super) fn encode(root: &Root) -> Result<Vec<u8>> {
    if root.schema != "polar.cas.value-index-root.v1" {
        return Err(Error::Integrity);
    }
    let mut w = Encoder::new(MAGIC, LIMIT)?;
    w.count(root.partitions.len())?;
    for (prefix, p) in &root.partitions {
        w.byte(*prefix)?;
        w.content(&p.facts)?;
        w.content(&p.native)?;
        w.string(&p.platform)?;
        w.u64(p.records as u64)?;
        w.byte(u8::from(p.storage.is_some()))?;
        if let Some(storage) = &p.storage {
            w.content(&storage.container)?;
            w.u64(storage.facts_offset)?;
            w.u64(storage.native_offset)?;
        }
    }
    encode_value(&mut w, &root.metadata)?;
    Ok(w.finish())
}
pub(super) fn decode(bytes: &[u8]) -> Result<Root> {
    if !bytes.starts_with(MAGIC) {
        let root: Root = serde_json::from_slice(bytes).map_err(|_| Error::Integrity)?;
        if root.schema != "polar.cas.value-index-root.v1"
            || canonical_json_bytes(&root).map_err(|_| Error::Integrity)? != bytes
        {
            return Err(Error::Integrity);
        }
        return Ok(root);
    }
    let mut r = Decoder::new(bytes, MAGIC, LIMIT)?;
    let count = r.count()?;
    if count > 256 {
        return Err(Error::Integrity);
    }
    let mut partitions = BTreeMap::new();
    let mut previous = None;
    for _ in 0..count {
        let prefix = r.byte()?;
        if previous.is_some_and(|old| old >= prefix) {
            return Err(Error::Integrity);
        }
        previous = Some(prefix);
        let facts = r.content()?;
        let native = r.content()?;
        let platform = r.string()?.to_owned();
        let records = usize::try_from(r.u64()?).map_err(|_| Error::TooLarge)?;
        if records == 0 || records > 1_000_000 {
            return Err(Error::Integrity);
        }
        let storage = match r.byte()? {
            0 => None,
            1 => Some(PartitionStorage {
                container: r.content()?,
                facts_offset: r.u64()?,
                native_offset: r.u64()?,
            }),
            _ => return Err(Error::Integrity),
        };
        partitions.insert(
            prefix,
            PartitionRef {
                facts,
                native,
                platform,
                records,
                storage,
            },
        );
    }
    let metadata = decode_value(&mut r)?;
    r.finish()?;
    Ok(Root {
        schema: "polar.cas.value-index-root.v1".into(),
        partitions,
        metadata,
    })
}


