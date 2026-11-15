//! Bounded portable primitives used by index directories and domain records.
//! The owning format supplies its magic/version and semantic validation.
use polar_cas::{ContentRef, DigestAlgorithm, Error, ObjectId, Result};

pub struct Encoder {
    bytes: Vec<u8>,
    limit: usize,
}
impl Encoder {
    pub fn new(magic: &[u8], limit: usize) -> Result<Self> {
        let mut out = Self {
            bytes: Vec::new(),
            limit,
        };
        out.raw(magic)?;
        Ok(out)
    }
    pub fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(Error::TooLarge);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub fn byte(&mut self, value: u8) -> Result<()> {
        self.raw(&[value])
    }
    pub fn u32(&mut self, value: u32) -> Result<()> {
        self.raw(&value.to_le_bytes())
    }
    pub fn u64(&mut self, value: u64) -> Result<()> {
        self.raw(&value.to_le_bytes())
    }
    pub fn count(&mut self, value: usize) -> Result<()> {
        self.u32(u32::try_from(value).map_err(|_| Error::TooLarge)?)
    }
    pub fn string(&mut self, value: &str) -> Result<()> {
        self.count(value.len())?;
        self.raw(value.as_bytes())
    }
    pub fn object(&mut self, value: &ObjectId) -> Result<()> {
        self.raw(value.digest_bytes())?;
        self.byte(u8::from(value.algorithm() == DigestAlgorithm::Blake3))
    }
    pub fn content(&mut self, value: &ContentRef) -> Result<()> {
        self.object(&value.id)?;
        self.u64(value.size_bytes)
    }
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
pub struct Decoder<'a> {
    bytes: &'a [u8],
}
impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8], magic: &[u8], limit: usize) -> Result<Self> {
        if bytes.len() > limit {
            return Err(Error::TooLarge);
        }
        Ok(Self {
            bytes: bytes.strip_prefix(magic).ok_or(Error::Integrity)?,
        })
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len()
    }
    pub fn raw(&mut self, len: usize) -> Result<&'a [u8]> {
        let out = self.bytes.get(..len).ok_or(Error::Integrity)?;
        self.bytes = &self.bytes[len..];
        Ok(out)
    }
    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.raw(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.raw(4)?.try_into().map_err(|_| Error::Integrity)?,
        ))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.raw(8)?.try_into().map_err(|_| Error::Integrity)?,
        ))
    }
    pub fn count(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > self.remaining() {
            return Err(Error::Integrity);
        }
        Ok(n)
    }
    pub fn string(&mut self) -> Result<&'a str> {
        let n = self.count()?;
        std::str::from_utf8(self.raw(n)?).map_err(|_| Error::Integrity)
    }
    pub fn object(&mut self) -> Result<ObjectId> {
        let digest = self.raw(32)?.try_into().map_err(|_| Error::Integrity)?;
        let algorithm = match self.byte()? {
            0 => DigestAlgorithm::Sha256,
            1 => DigestAlgorithm::Blake3,
            _ => return Err(Error::Integrity),
        };
        Ok(ObjectId::from_digest(algorithm, digest))
    }
    pub fn content(&mut self) -> Result<ContentRef> {
        Ok(ContentRef {
            id: self.object()?,
            size_bytes: self.u64()?,
        })
    }
    pub fn finish(self) -> Result<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(Error::Integrity)
        }
    }
}

/// Metadata stays extensible in memory, but is not serialized as JSON text.
/// Tags preserve null, boolean, numeric and string values; nesting is bounded.
pub fn encode_value(w: &mut Encoder, value: &serde_json::Value) -> Result<()> {
    write_value(w, value, 0)
}
fn write_value(w: &mut Encoder, value: &serde_json::Value, depth: usize) -> Result<()> {
    use serde_json::Value;
    if depth > 64 {
        return Err(Error::TooLarge);
    }
    match value {
        Value::Null => w.byte(0),
        Value::Bool(false) => w.byte(1),
        Value::Bool(true) => w.byte(2),
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                w.byte(3)?;
                w.u64(n as u64)
            } else if let Some(n) = n.as_u64() {
                w.byte(4)?;
                w.u64(n)
            } else {
                w.byte(5)?;
                w.u64(n.as_f64().ok_or(Error::Integrity)?.to_bits())
            }
        }
        Value::String(s) => {
            w.byte(6)?;
            w.string(s)
        }
        Value::Array(values) => {
            w.byte(7)?;
            w.count(values.len())?;
            for v in values {
                write_value(w, v, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            w.byte(8)?;
            w.count(values.len())?;
            let ordered: std::collections::BTreeMap<_, _> = values.iter().collect();
            for (k, v) in ordered {
                w.string(k)?;
                write_value(w, v, depth + 1)?;
            }
            Ok(())
        }
    }
}
pub fn decode_value(r: &mut Decoder<'_>) -> Result<serde_json::Value> {
    read_value(r, 0)
}
fn read_value(r: &mut Decoder<'_>, depth: usize) -> Result<serde_json::Value> {
    use serde_json::Value;
    if depth > 64 {
        return Err(Error::TooLarge);
    }
    Ok(match r.byte()? {
        0 => Value::Null,
        1 => Value::Bool(false),
        2 => Value::Bool(true),
        3 => Value::from(r.u64()? as i64),
        4 => Value::from(r.u64()?),
        5 => Value::Number(
            serde_json::Number::from_f64(f64::from_bits(r.u64()?)).ok_or(Error::Integrity)?,
        ),
        6 => Value::String(r.string()?.to_owned()),
        7 => {
            let n = r.count()?;
            let mut values = Vec::new();
            for _ in 0..n {
                values.push(read_value(r, depth + 1)?);
            }
            Value::Array(values)
        }
        8 => {
            let n = r.count()?;
            let mut values = serde_json::Map::new();
            let mut previous: Option<&str> = None;
            for _ in 0..n {
                let key = r.string()?;
                if previous.is_some_and(|p| p >= key) {
                    return Err(Error::Integrity);
                }
                previous = Some(key);
                values.insert(key.to_owned(), read_value(r, depth + 1)?);
            }
            Value::Object(values)
        }
        _ => return Err(Error::Integrity),
    })
}


