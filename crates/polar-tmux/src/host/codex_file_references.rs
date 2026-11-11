//! Exact original-message reads for Resource references; no terminal reconstruction.
use super::codex_control::ControlResult;
use super::codex_endpoint::{CodexEndpoint, RpcFailure};
use polar_tmux_resources::{
    complete_source_file_reference, source_file_references, source_file_search_terms,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn lookup(
    client: &CodexEndpoint,
    thread: &Value,
    fragment: &str,
) -> ControlResult<Option<String>> {
    // Leave time for origin/mapping projection within the existing8s desktop
    // request budget. This is a read deadline, never a retry/polling timer.
    tokio::time::timeout(
        std::time::Duration::from_secs(6),
        lookup_inner(client, thread, fragment),
    )
    .await
    .map_err(|_| "file_reference_lookup_timeout")?
}

async fn lookup_inner(
    client: &CodexEndpoint,
    thread: &Value,
    fragment: &str,
) -> ControlResult<Option<String>> {
    if fragment.ends_with('/') {
        return Ok(None);
    }
    let id = thread["id"].as_str().ok_or("invalid_thread_response")?;
    let mut references = Vec::new();
    if thread["historyMode"] == "paginated" {
        let mut items = BTreeMap::<String, BTreeSet<String>>::new();
        for term in source_file_search_terms(fragment) {
            let mut cursor: Option<String> = None;
            let mut consumed = BTreeSet::new();
            loop {
                let page = client
                    .request(
                        "thread/searchOccurrences",
                        json!({
                            "threadId":id,"searchTerm":term,"cursor":cursor,"limit":100,
                        }),
                    )
                    .await
                    .map_err(RpcFailure::code)?;
                for item in page["data"]
                    .as_array()
                    .ok_or("invalid_file_reference_response")?
                {
                    let turn = item["turnId"]
                        .as_str()
                        .ok_or("invalid_file_reference_response")?;
                    let item = item["itemId"]
                        .as_str()
                        .ok_or("invalid_file_reference_response")?;
                    items.entry(turn.into()).or_default().insert(item.into());
                }
                match page["nextCursor"].as_str() {
                    Some(next) if consumed.insert(next.to_owned()) => cursor = Some(next.into()),
                    Some(_) => return Err("invalid_file_reference_cursor"),
                    None => break,
                }
            }
        }
        for (turn, ids) in items {
            let ids = ids.into_iter().collect::<Vec<_>>();
            // Upstream ThreadItemsReadParams accepts1..=100 IDs, not a history cap.
            for batch in ids.chunks(100) {
                let response = client
                    .request(
                        "thread/items/read",
                        json!({
                            "threadId":id,"turnId":turn,"itemIds":batch,
                        }),
                    )
                    .await
                    .map_err(RpcFailure::code)?;
                let mut seen = BTreeSet::new();
                for entry in response["data"]
                    .as_array()
                    .ok_or("invalid_file_reference_response")?
                {
                    let item = &entry["item"];
                    let item_id = item["id"]
                        .as_str()
                        .ok_or("invalid_file_reference_response")?;
                    if entry["turnId"] != turn || !batch.iter().any(|id| id == item_id) {
                        return Err("file_reference_source_changed");
                    }
                    seen.insert(item_id.to_owned());
                    append(item, &mut references)?;
                }
                if seen.len() != batch.len() {
                    return Err("file_reference_source_changed");
                }
            }
        }
    } else {
        // Legacy histories have no indexed occurrence/item API. Reuse their
        // original read contract; paginated histories never take this branch.
        let value = client
            .request("thread/read", json!({"threadId":id,"includeTurns":true}))
            .await
            .map_err(RpcFailure::code)?;
        if value["thread"]["id"] != id {
            return Err("thread_mismatch");
        }
        for turn in value["thread"]["turns"]
            .as_array()
            .ok_or("invalid_thread_response")?
        {
            for item in turn["items"].as_array().ok_or("invalid_thread_response")? {
                append(item, &mut references)?;
            }
        }
    }
    complete_source_file_reference(fragment, thread["cwd"].as_str(), &references)
        .map_err(|_| "file_reference_ambiguous")
}

fn append(
    item: &Value,
    references: &mut Vec<polar_tmux_resources::FileReference>,
) -> ControlResult<()> {
    if item["type"] == "agentMessage" {
        let text = item["text"]
            .as_str()
            .ok_or("invalid_file_reference_response")?;
        references.extend(source_file_references(text));
    }
    Ok(())
}
