//! JSON boundary for browser/Wasm and fixture parity. All message rules remain
//! in ThreadMessages; this adapter owns no network or render scheduling.
use crate::{trim_ranges, ThreadMessages};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct Reader {
    threads: BTreeMap<String, ThreadMessages>,
    active: BTreeSet<String>,
    protected: BTreeMap<String, BTreeSet<(String,String)>>,
    reading: BTreeMap<String, crate::ReadingState>,
    clock: u64,
    policy: crate::CachePolicy,
}

impl Reader {
    pub fn dispatch(&mut self, input: Value) -> Result<Value, String> {
        let op = input["op"].as_str().ok_or("missing reader operation")?;
        if op == "merge_activity" {
            return Ok(json!(crate::merge_activity(
                input["previous"].as_array().ok_or("missing prior activity")?,
                input["incoming"].as_array().ok_or("missing activity page")?,
                self.policy.presentation_bytes(),
            )?));
        }
        if op == "policy" {return Ok(json!(self.policy));}
        if op == "configure" {
            let policy:crate::CachePolicy=serde_json::from_value(input["policy"].clone()).map_err(|_|"invalid cache policy")?;
            policy.validate()?;
            if !trim_ranges(self.threads.iter_mut().map(|(key,cache)|(self.active.contains(key),self.protected.get(key).cloned().unwrap_or_default(),cache)).collect(),policy.message_bytes()) {
                return Err("Message cache is full; currently displayed content is protected".into());
            }
            self.policy=policy;return Ok(json!(policy));
        }
        if op == "merge" {
            let rows = input["rows"].as_array().ok_or("missing rows")?;
            let mut messages = ThreadMessages::default();
            for row in rows {
                let thread = row["thread_id"].as_str().ok_or("missing Thread")?;
                messages.put_message(thread, row.clone())?;
            }
            return Ok(json!(messages.sorted_messages()));
        }
        // Reject malformed navigation metadata before any cache mutation or eviction.
        let boundary: Option<(String, String)> = serde_json::from_value(input["boundary"].clone())
            .map_err(|_| "invalid page boundary")?;
        let view_anchor: Option<(String, String)> = serde_json::from_value(input["anchor"].clone())
            .map_err(|_| "invalid view anchor")?;
        let scope = input["scope"].as_str().filter(|key|!key.is_empty() && key.len() <= 4096).ok_or("invalid scope")?.to_owned();
        self.clock = self.clock.wrapping_add(1);
        match op {
            "open" => { self.active.insert(scope.clone()); }
            "close" => { self.active.remove(&scope); }
            "purge" => { self.active.remove(&scope); self.threads.remove(&scope); self.reading.remove(&scope); self.protected.remove(&scope); return Ok(Value::Null); }
            "invalidate" | "missing" | "verify" | "read" | "retain" | "checkpoint" | "reset_checkpoint" | "viewport" | "locate" | "return" | "generation" | "generation_snapshot" => {},
            _ => return Err("unknown reader operation".into()),
        }
        if op == "retain" {
            let thread = input["thread"].as_str().ok_or("missing Thread")?;
            let rows = input["rows"].as_array().ok_or("missing rows")?;
            // Preflight and deduplicate a batch before evicting or committing.
            let mut incoming = ThreadMessages::default();
            for row in rows { incoming.put_message(thread,row.clone())?; }
            let empty = ThreadMessages::default();
            let current = self.threads.get(&scope).unwrap_or(&empty);
            let additional = incoming.messages.values().try_fold(0usize,|total,row|
                current.additional_message_bytes(row).map(|bytes|total.saturating_add(bytes)))?;
            if additional > self.policy.message_bytes() { return Err("message batch exceeds cache budget".into()); }
            // Keep both incoming replacements and the presented source range.
            // Eviction must not invalidate the preflight replacement delta.
            if !trim_ranges(self.threads.iter_mut().map(|(key,cache)| {
                let mut protected=self.protected.get(key).cloned().unwrap_or_default();
                if key==&scope {protected.extend(incoming.messages.keys().cloned());}
                (key==&scope || self.active.contains(key),protected,cache)
            }).collect(),self.policy.message_bytes()-additional) {
                return Err("protected message cache exceeds budget".into());
            }
        }
        let other_bytes=self.threads.iter().filter(|(key,_)|*key!=&scope).map(|(_,cache)|cache.retained_bytes()).sum::<usize>();
        let cache = self.threads.entry(scope.clone()).or_default();
        cache.set_admission_limit(self.policy.message_bytes().saturating_sub(other_bytes));
        cache.last_used = self.clock;
        cache.set_presentation_limit(self.policy.presentation_bytes());
        if op == "invalidate" { cache.invalidate_coverage(); }
        // A server-confirmed missing anchor invalidates the boundary, not the bodies.
        if op == "reset_checkpoint" { cache.resume_message = None; }
        if op == "missing" || op == "verify" {
            let keys: Vec<(String,String)> = serde_json::from_value(input["keys"].clone()).map_err(|_|"invalid verification keys")?;
            if keys.len()>1000 {return Err("verification page exceeds limit".into());}
            let missing=keys.iter().filter(|key|!cache.messages.contains_key(*key)).cloned().collect::<Vec<_>>();
            if op=="missing" {return Ok(json!(missing));}
            if !missing.is_empty() {return Err("verification contains unadmitted messages".into());}
            let rows=keys.iter().filter_map(|key|cache.messages.get(key)).cloned().collect::<Vec<_>>();
            cache.mark_page(&rows,boundary.as_ref(),input["has_before"].as_bool(),input["has_after"].as_bool());
        }
        if op == "retain" {
            let thread = input["thread"].as_str().ok_or("missing Thread")?;
            let rows = input["rows"].as_array().ok_or("missing rows")?;
            // Validate the whole batch before mutation. A failed message must
            // not allow the caller to checkpoint a partially admitted page.
            for row in rows {
                if row["thread_id"].as_str() != Some(thread) { return Err("message belongs to another Thread".into()); }
                crate::message_key(row)?;
                if serde_json::to_vec(row).map_err(|_|"invalid Message")?.len() > crate::MAX_RECORD_BYTES {
                    return Err("message exceeds canonical record limit".into());
                }
            }
            let previous=cache.messages.values().max_by_key(|row|crate::order(row)).and_then(|row|crate::message_key(row).ok());
            for row in rows {
                let fresh=cache.put_message(thread, row.clone())?;
                if fresh && input["live"]==true {
                    self.reading.entry(scope.clone()).or_default().arrived(crate::message_key(row)?);
                }
            }
            if input["live"]==true || rows.iter().any(|row|row["optimistic"]==true) {
                cache.mark_append(rows,previous.as_ref());
            }
        }
        if op == "retain" {
            let rows=input["rows"].as_array().ok_or("missing rows")?;
            if input["verified"]!=false {cache.mark_page(rows,boundary.as_ref(),input["has_before"].as_bool(),input["has_after"].as_bool());}
        }
        if op == "generation" {
            cache.put_generation(input["owner"].as_str().ok_or("missing owner")?,input["thread"].as_str().ok_or("missing Thread")?,input["value"].clone())?;
        }
        if op == "generation_snapshot" {
            let owner=input["owner"].as_str().ok_or("missing owner")?;
            let thread=input["thread"].as_str().ok_or("missing Thread")?;
            let mut snapshot=ThreadMessages::default();
            for row in input["values"].as_array().ok_or("missing generations")? {snapshot.put_generation(owner,thread,row.clone())?;}
            let extra=snapshot.generations.iter().map(|(id,value)| {
                let old=cache.generations.get(id).map_or(0,|row|crate::retained_bytes(row)+id.len()+128);
                (crate::retained_bytes(value)+id.len()+128).saturating_sub(old)
            }).sum::<usize>();
            if cache.retained_bytes().saturating_add(extra)>self.policy.message_bytes().saturating_sub(other_bytes) {return Err("generation snapshot exceeds cache budget".into());}
            let baseline=&input["baseline"];
            // A response may race live events. Only remove missing previews
            // whose sequence is still the one observed when the query began.
            let removed=cache.generations.iter().filter(|(id,value)| !snapshot.generations.contains_key(*id)
                && baseline.get(id.as_str()).and_then(Value::as_u64)==value["sequence"].as_u64()).map(|(id,_)|id.clone()).collect::<Vec<_>>();
            for id in removed {cache.retire_generation(&id);}
            for row in snapshot.generations.into_values() {cache.put_generation(owner,thread,row)?;}
        }
        if op == "checkpoint" {
            let key: (String,String) = serde_json::from_value(input["key"].clone()).map_err(|_|"invalid message checkpoint")?;
            cache.acknowledge_message(key)?;
        }
        let reading=self.reading.entry(scope.clone()).or_default();
        if op=="locate" {
            let target:(String,String)=serde_json::from_value(input["key"].clone()).map_err(|_|"invalid reference key")?;
            let anchor:Option<crate::SourceAnchor>=serde_json::from_value(input["previous"].clone()).map_err(|_|"invalid reading anchor")?;
            reading.locate(target,anchor);
        }
        if op=="return" {reading.return_to_reading();}
        if op=="viewport" {
            reading.at_bottom=input["at_bottom"].as_bool().ok_or("missing viewport")?;
            if reading.at_bottom {reading.tail=true;reading.anchor=None;reading.highlight=None;reading.applied_latest();}
        }
        if !trim_ranges(self.threads.iter_mut().map(|(key,cache)|(self.active.contains(key),self.protected.get(key).cloned().unwrap_or_default(),cache)).collect(),self.policy.message_bytes()) {
            return Err("protected message cache exceeds budget".into());
        }
        let cache = &self.threads[&scope];
        let mut result=json!({"resume_message":cache.resume_message,
            "evicted":cache.evicted,"cached_messages":cache.messages.len(),"bytes":cache.retained_bytes(),"hint":reading.hint(cache.generations.values().any(|row|!matches!(row["state"].as_str(),Some("completed"|"failed"|"cancelled"))),cache.generations.values().any(|row|row["state"]=="failed"),false)});
        result["anchor"]=json!(reading.anchor);
        if matches!(op,"open"|"read"|"retain") {
            let (rows,before,after)=cache.message_window(view_anchor.as_ref());
            self.protected.insert(scope.clone(),rows.iter().filter_map(|row|crate::message_key(row).ok()).collect());
            result["rows"]=json!(rows);result["before"]=json!(before);result["after"]=json!(after);
        }
        if matches!(op,"open"|"read"|"retain"|"generation"|"generation_snapshot") {result["generations"]=json!(cache.generations.values().collect::<Vec<_>>());result["tool_activity"]=cache.message_activities();}
        self.threads.retain(|key,cache|key==&scope || self.active.contains(key) || !cache.messages.is_empty() || !cache.generations.is_empty() || !cache.activities.is_empty());
        self.reading.retain(|key,_|self.threads.contains_key(key));
        self.protected.retain(|key,_|self.threads.contains_key(key));
        Ok(result)
    }
}


