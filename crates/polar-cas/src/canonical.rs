//! Stable JSON encoding, extracted unchanged from Delivery's existing codec.
use serde::Serialize;
use serde_json::{Map, Value};

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let value = serde_json::to_value(value)?;
    serde_json::to_vec(&normalize_value(value))
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut normalized = Map::new();
            for (key, value) in entries {
                normalized.insert(key, normalize_value(value));
            }
            Value::Object(normalized)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_value).collect()),
        other => other,
    }
}
