use super::{model::RawTree, IdeaTree, Limits};
use crate::{Error, Result};
use polar_cas::{canonical::canonical_json_bytes, ContentRef, ObjectId};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

// Recursively reject duplicate keys BEFORE serde_json::Value would collapse
// them. The decoder's recursion limit stays enabled; input bytes are bounded.
struct StrictValue(Value);
struct StrictVisitor;
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}
impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duplicate-free JSON with integer numbers")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(v)))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(v.into())))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(v.into())))
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
        Err(E::custom("non-integer JSON number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v.to_owned())))
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v)))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        self.visit_unit()
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(StrictValue(v)) = seq.next_element::<StrictValue>()? {
            values.push(v);
        }
        Ok(StrictValue(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut input: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = input.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            values.insert(key, input.next_value::<StrictValue>()?.0);
        }
        Ok(StrictValue(Value::Object(values)))
    }
}
/// Encoded candidate bytes. The hash is NOT an admitted object reference; only
/// the CAS-owning runtime may turn successful storage into a determined binding.
pub struct EncodedIdea {
    tree: IdeaTree,
    bytes: Vec<u8>,
    candidate: ContentRef,
}
impl EncodedIdea {
    pub fn tree(&self) -> &IdeaTree {
        &self.tree
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn candidate_ref(&self) -> &ContentRef {
        &self.candidate
    }
}
pub(crate) fn strict_value(bytes: &[u8], limits: &Limits) -> Result<Value> {
    if bytes.len() > limits.max_bytes {
        return Err(Error::Limit("description bytes"));
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let StrictValue(value) = StrictValue::deserialize(&mut d)?;
    d.end()?;
    Ok(value)
}
pub fn parse_idea(bytes: &[u8], limits: &Limits) -> Result<IdeaTree> {
    let value = strict_value(bytes, limits)?;
    let raw: RawTree = serde_json::from_value(value)?;
    raw.validate(limits)
}
pub fn encode_idea(tree: &IdeaTree, limits: &Limits) -> Result<EncodedIdea> {
    let tree = tree.raw.clone().validate(limits)?;
    let bytes = canonical_json_bytes(&tree)?;
    if bytes.len() > limits.max_bytes {
        return Err(Error::Limit("canonical bytes"));
    }
    let candidate = ContentRef {
        id: ObjectId::for_bytes(&bytes),
        size_bytes: bytes.len() as u64,
    };
    Ok(EncodedIdea {
        tree,
        bytes,
        candidate,
    })
}
/// Verify the RECEIVED bytes; never hash pretty JSON and subsequently pretend
/// a canonical rewrite still has that original content identity.
pub fn decode_cas(bytes: &[u8], expected: &ContentRef, limits: &Limits) -> Result<IdeaTree> {
    if bytes.len() > limits.max_bytes {
        return Err(Error::Limit("description bytes"));
    }
    if bytes.len() as u64 != expected.size_bytes
        || ObjectId::for_bytes_with(expected.id.algorithm(), bytes) != expected.id
    {
        return Err(Error::IdentityMismatch);
    }
    let tree = parse_idea(bytes, limits)?;
    if canonical_json_bytes(&tree)? != bytes {
        return Err(Error::NonCanonical);
    }
    Ok(tree)
}
