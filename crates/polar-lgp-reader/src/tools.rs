//! Merge existing generation tool records by invocation identity, never name.
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Preserve first-observed call order while accepting newer durable observations.
/// The cursor belongs to the source page, never to this presentation order.
pub fn merge_activity(previous: &[Value], incoming: &[Value], budget: usize) -> Result<Vec<Value>, String> {
    let mut rows = Vec::<Value>::new();
    let mut positions = BTreeMap::new();
    for row in previous.iter().chain(incoming) {
        let generation = row["generation_id"].as_str().filter(|id|!id.is_empty()).ok_or("missing activity generation")?;
        let call = row["tool"]["call_id"].as_str().filter(|id|!id.is_empty()).ok_or("missing activity call")?;
        let sequence = row["sequence"].as_u64().ok_or("invalid activity sequence")?;
        let key = (generation.to_owned(),call.to_owned());
        if let Some(index) = positions.get(&key).copied() {
            let old: &Value = &rows[index];
            if old["sequence"].as_u64().unwrap_or(0) < sequence {rows[index]=row.clone();}
        } else {
            positions.insert(key,rows.len());
            rows.push(row.clone());
        }
    }
    let bytes=rows.iter().map(crate::retained_bytes).sum::<usize>() + rows.capacity()*std::mem::size_of::<Value>();
    if bytes>budget {return Err("Tool history cache budget exhausted; earlier records were retained".into());}
    Ok(rows)
}

pub fn tools_collapsed(generation: &Value) -> bool {
    matches!(generation["state"].as_str(), Some("completed" | "failed" | "cancelled"))
}

pub(super) fn merge(previous: Option<&Value>, next: &mut Value) -> Result<(), String> {
    let mut rows = previous.and_then(|value| value["tools"].as_array()).cloned().unwrap_or_default();
    let mut positions = rows.iter().enumerate().filter_map(|(index, row)|
        row["call_id"].as_str().map(|id| (id.to_owned(), index))).collect::<BTreeMap<_, _>>();
    if !next["tools"].is_null() {
        for row in next["tools"].as_array().ok_or("invalid tool activity")? {
            let id = row["call_id"].as_str().filter(|id| !id.is_empty()).ok_or("missing tool call identity")?;
            if let Some(index) = positions.get(id).copied() {
                // A repeated start or incomplete snapshot cannot reopen a call.
                if rows[index]["state"] == "running" { rows[index] = row.clone(); }
            } else {
                positions.insert(id.to_owned(), rows.len());
                rows.push(row.clone());
            }
        }
    }
    next["tools"] = json!(rows);
    next["tools_collapsed"] = json!(tools_collapsed(next));
    Ok(())
}
