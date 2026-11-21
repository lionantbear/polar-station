//! Canonical ordered message bodies under one original-identity outer signature.
use crate::{WireError, WireResult};
pub const MESSAGE_BATCH_MAX_ITEMS: usize = 32;
pub const MESSAGE_BATCH_TARGET_BYTES: usize = 64 * 1024;

pub fn encode_message_batch(items: &[Vec<u8>]) -> WireResult<Vec<u8>> {
    if items.is_empty() || items.len() > MESSAGE_BATCH_MAX_ITEMS {
        return Err(WireError::Codec("invalid message batch count".into()));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    for item in items {
        let n = u32::try_from(item.len())
            .map_err(|_| WireError::Codec("message batch item too large".into()))?;
        if n == 0 {
            return Err(WireError::Codec("empty message batch item".into()));
        }
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(item);
    }
    Ok(out)
}
pub fn decode_message_batch(bytes: &[u8]) -> WireResult<Vec<&[u8]>> {
    let invalid = || WireError::Codec("malformed message batch".into());
    let head = bytes.get(..2).ok_or_else(invalid)?;
    let count = u16::from_le_bytes([head[0], head[1]]) as usize;
    if count == 0 || count > MESSAGE_BATCH_MAX_ITEMS {
        return Err(invalid());
    }
    let mut at = 2;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let h = bytes.get(at..at + 4).ok_or_else(invalid)?;
        at += 4;
        let n = u32::from_le_bytes(h.try_into().map_err(|_| invalid())?) as usize;
        let end = at.checked_add(n).ok_or_else(invalid)?;
        if n == 0 {
            return Err(invalid());
        }
        items.push(bytes.get(at..end).ok_or_else(invalid)?);
        at = end;
    }
    if at != bytes.len() {
        return Err(invalid());
    }
    Ok(items)
}

