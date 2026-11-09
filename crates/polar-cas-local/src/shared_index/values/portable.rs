//! The bounded little-endian record area is portable; the upstream PHF tail is
//! not. Import never constructs or deserializes a native navigator.
use super::*;

pub(super) fn prefix(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() < HEADER
        || !matches!(
            &bytes[..8],
            b"PCASVAL2" | b"PCASVAL3" | b"PCASVAL4" | b"PCASVAL5"
        )
    {
        return Err(Error::Integrity);
    }
    let end = num(bytes, 32)?;
    if !(HEADER..=MAX_BYTES).contains(&end) {
        return Err(Error::TooLarge);
    }
    bytes.get(..end).ok_or(Error::Integrity)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Vec<ValueRecord>> {
    if prefix(bytes)?.len() != bytes.len()
        || bytes[if &bytes[..8] == b"PCASVAL5" { 120 } else { 88 }..HEADER]
            .iter()
            .any(|b| *b != 0)
    {
        return Err(Error::Integrity);
    }
    let count = num(bytes, 8)?;
    let slots = num(bytes, 16)?;
    let payload = num(bytes, 24)?;
    let dictionary = num(bytes, 72)?;
    let packs = num(bytes, 80)?;
    if count == 0
        || count > MAX_RECORDS
        || count > slots
        || slots > MAX_RECORDS * 2
        || payload != HEADER + slots * WIDTH
        || payload > dictionary
        || packs > count
        || dictionary
            .checked_add(packs * 40)
            .is_none_or(|end| end > bytes.len())
    {
        return Err(Error::Integrity);
    }
    if bytes[dictionary + packs * 40..].iter().any(|b| *b != 0) {
        return Err(Error::Integrity);
    }
    let mut rows = Vec::with_capacity(count);
    let mut ranges = Vec::with_capacity(count);
    for slot in 0..slots {
        let at = HEADER + slot * WIDTH;
        let flags = word(bytes, at + 40)?;
        if flags == 0 {
            if bytes[at..at + WIDTH].iter().any(|b| *b != 0) {
                return Err(Error::Integrity);
            }
            continue;
        }
        if flags & PRESENT == 0 {
            return Err(Error::Integrity);
        }
        let generic = flags & GENERIC != 0;
        let shared_key = flags & KEY_IN_VALUE != 0;
        if shared_key
            && (!matches!(&bytes[..8], b"PCASVAL3" | b"PCASVAL4" | b"PCASVAL5") || generic)
        {
            return Err(Error::Integrity);
        }
        if flags & LOCAL_ONLY != 0 && !shared_key {
            rows.push(compact_row(bytes, at, dictionary, packs)?);
            continue;
        }
        let pack = if shared_key {
            flags & PACK_MASK
        } else {
            flags & !(PRESENT | ALGORITHM | GENERIC)
        };
        if (generic && pack != 0) || pack > packs as u64 {
            return Err(Error::Integrity);
        }
        let locator = ObjectId::from_digest(
            if flags & ALGORITHM == 0 {
                DigestAlgorithm::Sha256
            } else {
                DigestAlgorithm::Blake3
            },
            bytes[at..at + 32]
                .try_into()
                .map_err(|_| Error::Integrity)?,
        );
        let key_len = if generic { num(bytes, at + 32)? } else { 33 };
        let start = num(bytes, at + 48)?;
        let key_start = if shared_key {
            start
                .checked_add(((flags >> 32) & KEY_OFFSET_MASK) as usize)
                .ok_or(Error::Integrity)?
        } else {
            start
        };
        let key_end = key_start.checked_add(key_len).ok_or(Error::Integrity)?;
        let value_start = if shared_key { start } else { key_end };
        let end = value_start
            .checked_add(num(bytes, at + 56)?)
            .ok_or(Error::Integrity)?;
        if start < payload || end > dictionary || key_end > end {
            return Err(Error::Integrity);
        }
        let key = bytes[key_start..key_end].to_vec();
        if !generic && key != identity(&locator) {
            return Err(Error::Integrity);
        }
        ranges.push((start, end));
        rows.push(ValueRecord {
            locator,
            key,
            value: bytes[value_start..end].to_vec(),
        });
    }
    if rows.len() != count {
        return Err(Error::Integrity);
    }
    ranges.sort_unstable();
    let mut previous = payload;
    for (start, end) in ranges {
        if start != previous {
            return Err(Error::Integrity);
        }
        previous = end;
    }
    if previous != dictionary {
        return Err(Error::Integrity);
    }
    rows.sort_unstable_by_key(|row| identity(&row.locator));
    if rows
        .windows(2)
        .any(|rows| rows[0].locator == rows[1].locator)
    {
        return Err(Error::Integrity);
    }
    Ok(rows)
}
