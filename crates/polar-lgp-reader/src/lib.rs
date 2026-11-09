//! Shared in-memory LGP reader. Canonical message views are retained unchanged.
//! No transport, persistence, OS geometry, timer or authorization lives here.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub mod bridge;
mod coverage;
mod tools;
mod policy;
pub use policy::CachePolicy;
pub use tools::{tools_collapsed, merge_activity};
mod reading;
pub use reading::{ReadingHint, ReadingState, SourceAnchor};
#[cfg(target_arch = "wasm32")]
mod wasm;

pub const MESSAGE_CACHE_BYTES: usize = 512 * 1024 * 1024;
pub const PRESENTATION_CACHE_BYTES: usize = 32 * 1024 * 1024;
pub type MessageWindow = (Vec<Value>, Option<(String,String)>, Option<(String,String)>);

pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// An index into canonical JSON, not a second message DTO. Both components of
/// identity are required; created_at and generation sequence are never keys.
pub fn message_key(value: &Value) -> Result<(String, String), String> {
    let sender = value["sender_node_id_hex"].as_str()
        .or_else(|| value["sender_node_id"].as_str()).ok_or("message has no sender identity")?;
    let message = value["message_id"].as_str().ok_or("message has no identity")?;
    if sender.is_empty() || message.is_empty() || sender.len() > 128 || message.len() > 128 {
        return Err("invalid Message key".into());
    }
    Ok((sender.to_ascii_lowercase(), message.to_owned()))
}

pub fn order(value: &Value) -> (u64, &str, &str) {
    (value["created_at"].as_u64().unwrap_or(0), value["message_id"].as_str().unwrap_or(""),
        value["sender_node_id_hex"].as_str().or_else(||value["sender_node_id"].as_str()).unwrap_or(""))
}

/// Conservative retained-allocation accounting, not wire/serialized size.
/// Include value/map slots and capacity, including strings hidden by projection.
pub fn retained_bytes(value: &Value) -> usize {
    std::mem::size_of::<Value>() + match value {
        Value::String(text) => text.capacity(),
        Value::Array(rows) => rows.capacity() * std::mem::size_of::<Value>()
            + rows.iter().map(retained_bytes).sum::<usize>(),
        Value::Object(fields) => fields.iter().map(|(key, value)|
            128 + key.capacity() + retained_bytes(value)).sum(),
        _ => 0,
    }
}

#[derive(Clone)]
pub struct ThreadMessages {
    pub messages: BTreeMap<(String, String), Value>,
    pub generations: BTreeMap<String, Value>,
    /// Tool-bearing records outlive transient answer previews; memory-only.
    pub activities: BTreeMap<String, Value>,
    activity_bytes: usize,
    coverage: coverage::Coverage,
    bytes: usize,
    generation_bytes: usize,
    generation_messages: BTreeMap<String,(String,String)>,
    /// Last admitted canonical message boundary, scoped by the caller.
    pub resume_message: Option<(String, String)>,
    pub last_used: u64,
    /// An eviction makes coverage incomplete; it must never masquerade as a
    /// fully loaded page merely because older and newer keys remain present.
    pub evicted: bool,
    admission_limit: usize,
    presentation_limit: usize,
    pub admission_shortfall: usize,
}
impl Default for ThreadMessages {
    fn default()->Self {Self{activities:BTreeMap::new(),activity_bytes:0,messages:BTreeMap::new(),generations:BTreeMap::new(),coverage:Default::default(),bytes:0,generation_bytes:0,generation_messages:BTreeMap::new(),
        resume_message:None,last_used:0,evicted:false,admission_limit:MESSAGE_CACHE_BYTES,presentation_limit:PRESENTATION_CACHE_BYTES,admission_shortfall:0}}
}

impl ThreadMessages {
    /// Only advance to an admitted message; a late smaller identity never
    /// rewinds the boundary, and receipt is independent of display acknowledgement.
    pub fn acknowledge_message(&mut self, key: (String, String)) -> Result<(), String> {
        let next = self.messages.get(&key).ok_or("cannot acknowledge an absent message")?;
        if self.resume_message.as_ref().and_then(|key|self.messages.get(key))
            .is_none_or(|previous| order(next) >= order(previous)) {
            self.resume_message = Some(key);
        }
        Ok(())
    }
    pub fn invalidate_coverage(&mut self) { self.coverage = Default::default(); }

    pub fn retained_bytes(&self) -> usize { self.bytes + self.generation_bytes + self.activity_bytes + self.coverage.bytes() }
    pub fn set_presentation_limit(&mut self,bytes:usize){self.presentation_limit=bytes;}
    pub fn set_admission_limit(&mut self, limit:usize) {self.admission_limit=limit;self.admission_shortfall=0;}

    pub fn additional_message_bytes(&self, value: &Value) -> Result<usize, String> {
        let key = message_key(value)?;
        let previous = self.messages.get(&key).map_or(0, |old|entry_bytes(&key,old));
        Ok(entry_bytes(&key,value).saturating_sub(previous))
    }

    pub fn put_message(&mut self, thread: &str, value: Value) -> Result<bool, String> {
        if value["thread_id"].as_str() != Some(thread) { return Err("message belongs to another Thread".into()); }
        let key = message_key(&value)?;
        if serde_json::to_vec(&value).map_err(|_| "invalid Message")?.len() > MAX_RECORD_BYTES {
            return Err("message exceeds canonical record limit".into());
        }
        if self.messages.get(&key) == Some(&value) { return Ok(false); }
        self.admission_shortfall=self.retained_bytes().saturating_add(self.additional_message_bytes(&value)?).saturating_sub(self.admission_limit);
        if self.admission_shortfall>0 {
            return Err("message cache budget exhausted".into());
        }
        let fresh = !self.messages.contains_key(&key);
        // A canonical acknowledgement replaces its optimistic row even when
        // the server assigned a different message ID. Trace is only a local
        // optimistic correlation; it is never protocol message identity.
        if value["optimistic"] != true {
            if let Some(trace) = value["trace_id"].as_str().filter(|id| !id.is_empty()) {
                let obsolete = self.messages.iter().filter(|(_, old)| old["optimistic"] == true
                    && old["trace_id"].as_str() == Some(trace)).map(|(key,_)|key.clone()).collect::<Vec<_>>();
                for key in obsolete { self.remove(&key); }
            }
        }
        if let Some(previous)=self.messages.get(&key).and_then(generation_id) {self.generation_messages.remove(previous);}
        if let Some(id)=generation_id(&value) { self.generation_messages.insert(id.to_owned(),key.clone()); }
        let size = entry_bytes(&key, &value);
        if let Some(old) = self.messages.insert(key.clone(), value) {
            self.bytes = self.bytes.saturating_sub(entry_bytes(&key, &old));
        }
        self.bytes += size;
        let finished = self.generations.iter().filter(|(_, generation)| final_present(&self.messages, &self.generation_messages, generation))
            .map(|(id,_)|id.clone()).collect::<Vec<_>>();
        for id in finished { self.retire_generation(&id); }
        Ok(fresh)
    }

    pub fn put_generation(&mut self, owner: &str, thread: &str, mut value: Value) -> Result<bool, String> {
        if value["owner"].as_str() != Some(owner) || value["thread"].as_str() != Some(thread) {
            return Err("generation scope mismatch".into());
        }
        let id = value["generation_id"].as_str().filter(|id| !id.is_empty()).ok_or("missing generation ID")?.to_owned();
        let sequence = value["sequence"].as_u64().ok_or("missing generation sequence")?;
        let old = self.generations.get(&id).or_else(||self.activities.get(&id));
        if self.generations.get(&id).is_some_and(|old| old["sequence"].as_u64().unwrap_or(0) >= sequence) { return Ok(false); }
        if let Some(old)=self.activities.get(&id).filter(|old|old["sequence"].as_u64().unwrap_or(0)>=sequence) {value=old.clone();}
        if let Some(old)=old.filter(|old|tools_collapsed(old)) { value["state"]=old["state"].clone(); }
        tools::merge(old, &mut value)?;
        let finished=final_present(&self.messages, &self.generation_messages, &value);
        if !finished && !self.generations.contains_key(&id) && self.generations.len() >= 64 { return Err("too many active generations".into()); }
        if serde_json::to_vec(&value).map_err(|_|"invalid generation")?.len() > MAX_RECORD_BYTES { return Err("generation record limit".into()); }
        let previous=self.generations.get(&id).or_else(||self.activities.get(&id)).map_or(0,|old|retained_bytes(old)+id.len()+128);
        self.admission_shortfall=self.retained_bytes().saturating_sub(previous).saturating_add(retained_bytes(&value)+id.len()+128).saturating_sub(self.admission_limit);
        if self.admission_shortfall>0 {
            return Err("message cache budget exhausted".into());
        }
        self.remove_generation(&id);
        self.remove_activity(&id);
        self.generation_bytes += retained_bytes(&value) + id.capacity() + 128;
        self.generations.insert(id.clone(), value);
        if finished {self.retire_generation(&id);}
        Ok(true)
    }

    /// Complete snapshots replace active generations, never retained history.
    /// Partial snapshots must be kept outside this committed cache by callers.
    pub fn merge_snapshot(&mut self, thread: &str, snapshot: Self) -> Result<(), String> {
        let additional=snapshot.messages.values().try_fold(0usize,|total,value|
            self.additional_message_bytes(value).map(|bytes|total.saturating_add(bytes)))?;
        if snapshot.messages.values().any(|value|value["thread_id"].as_str()!=Some(thread)) {return Err("message belongs to another Thread".into());}
        // Build only the small coverage index ahead of admission; canonical
        // message bodies are not cloned. Include its allocations in preflight.
        let mut coverage = self.coverage.clone();
        coverage.union(snapshot.coverage);
        let mut updates=Vec::new();
        let mut generation_extra=0usize;
        for (id,mut value) in snapshot.generations.into_iter().chain(snapshot.activities) {
            let previous=self.generations.get(&id).or_else(||self.activities.get(&id));
            if let Some(old)=previous.filter(|old|old["sequence"].as_u64()>=value["sequence"].as_u64()) {value=old.clone();}
            if let Some(old)=previous.filter(|old|tools_collapsed(old)) {value["state"]=old["state"].clone();}
            tools::merge(previous,&mut value)?;
            let old_bytes=previous.map_or(0,|old|retained_bytes(old)+id.len()+128);
            generation_extra=generation_extra.saturating_add((retained_bytes(&value)+id.len()+128).saturating_sub(old_bytes));
            updates.push(value);
        }
        self.admission_shortfall=self.bytes.saturating_add(self.generation_bytes).saturating_add(self.activity_bytes).saturating_add(additional)
            .saturating_add(generation_extra).saturating_add(coverage.bytes())
            .saturating_sub(self.admission_limit);
        if self.admission_shortfall>0 {
            return Err("message cache budget exhausted".into());
        }
        self.coverage = coverage;
        for id in self.generations.keys().cloned().collect::<Vec<_>>() {self.retire_generation(&id);}
        for value in snapshot.messages.into_values() { self.put_message(thread, value)?; }
        for value in updates {
            if value["sequence"].is_null() {
                let id=value["generation_id"].as_str().ok_or("missing activity generation")?.to_owned();
                if !self.generation_messages.contains_key(&id) {return Err("activity has no canonical reply".into());}
                self.remove_activity(&id);
                self.activity_bytes+=retained_bytes(&value)+id.len()+128;
                self.activities.insert(id,value);
                continue;
            }
            let owner=value["owner"].as_str().ok_or("missing generation owner")?.to_owned();
            self.put_generation(&owner,thread,value)?;
        }
        if snapshot.resume_message.is_some() { self.resume_message = snapshot.resume_message; }
        Ok(())
    }

    pub fn cursor(&self, oldest: bool) -> Option<Value> {
        let row = if oldest { self.messages.values().min_by_key(|row|order(row)) }
            else { self.messages.values().max_by_key(|row|order(row)) }?;
        let (sender, message) = message_key(row).ok()?;
        Some(json!({"created_at_secs":row["created_at"],"message_id":message,"sender_id":sender}))
    }

    pub fn mark_range(&mut self, rows:&[Value], anchor:Option<&(String,String)>) {
        self.mark_page(rows,anchor,None,None);
    }
    pub fn mark_page(&mut self,rows:&[Value],anchor:Option<&(String,String)>,before:Option<bool>,after:Option<bool>) {
        self.coverage.include(rows.iter().chain(anchor.and_then(|key|self.messages.get(key))),before,after);
    }
    pub fn range_edges(&self,key:&(String,String))->(Option<bool>,Option<bool>) {
        self.messages.get(key).map_or((None,None),|row|self.coverage.edges(row))
    }
    pub fn mark_append(&mut self,rows:&[Value],previous:Option<&(String,String)>) {
        let previous=previous.and_then(|key|self.messages.get(key));
        self.coverage.include(rows.iter().filter(|row|previous.is_none_or(|old|order(row).0>=order(old).0)).chain(previous),None,Some(false));
    }
    pub fn mark_keys(&mut self,keys:&[(String,String)]) {
        self.coverage.include(keys.iter().filter_map(|key|self.messages.get(key)),None,None);
    }
    pub fn connected(&self,left:&(String,String),right:&(String,String))->bool {
        if left==right {return true;}
        match (self.messages.get(left),self.messages.get(right)) {
            (Some(a),Some(b))=>self.coverage.contains(a,b),_=>false,
        }
    }
    /// A contiguous canonical view; presentation limits never evict history.
    pub fn message_window(&self,anchor:Option<&(String,String)>)->MessageWindow {
        let mut ordered=self.messages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_,row)|order(row));
        if ordered.is_empty(){return (Vec::new(),None,None);}
        let center=anchor.and_then(|key|ordered.iter().position(|(candidate,_)|*candidate==key)).unwrap_or(ordered.len()-1);
        let mut begin=center;let mut end=center+1;
        let mut bytes=retained_bytes(ordered[center].1);
        let before_budget=if anchor.is_some(){self.presentation_limit/8}else{self.presentation_limit/4};
        while begin>0 && self.connected(ordered[begin-1].0,ordered[begin].0)
            && bytes+retained_bytes(ordered[begin-1].1)<=before_budget {
            begin-=1;bytes+=retained_bytes(ordered[begin].1);
        }
        while end<ordered.len() && self.connected(ordered[end-1].0,ordered[end].0)
            && bytes+retained_bytes(ordered[end].1)<=self.presentation_limit/4 {
            bytes+=retained_bytes(ordered[end].1);end+=1;
        }
        let before=(begin>0 || self.coverage.edges(ordered[begin].1).0!=Some(false)).then(||ordered[begin].0.clone());
        let after=(end<ordered.len() || self.coverage.edges(ordered[end-1].1).1!=Some(false)).then(||ordered[end-1].0.clone());
        (ordered[begin..end].iter().map(|(_,row)|(*row).clone()).collect(),before,after)
    }
    pub fn sorted_messages(&self) -> Vec<Value> {
        let mut rows = self.messages.values().collect::<Vec<_>>();
        rows.sort_by_key(|row|order(row));
        rows.into_iter().cloned().collect()
    }

    pub fn evict_oldest(&mut self, protected: &BTreeSet<(String, String)>) -> bool {
        let key = self.messages.iter().filter(|(key,_)| !protected.contains(*key))
            .min_by_key(|(_,value)|order(value)).map(|(key,_)|key.clone());
        let Some(key) = key else { return false; };
        self.remove(&key);
        self.evicted = true;
        true
    }

    /// Sort candidates once per pressure pass, not once per evicted record.
    fn evict_to(&mut self, limit:usize, protected:&BTreeSet<(String,String)>) {
        let mut candidates=self.messages.iter().filter(|(key,_)|!protected.contains(*key))
            .map(|(key,value)|(order(value).0,key.clone())).collect::<Vec<_>>();
        candidates.sort();
        for (_,key) in candidates {
            if self.retained_bytes()<=limit {break;}
            self.remove(&key);self.evicted=true;
        }
    }

    fn remove(&mut self, key: &(String, String)) {
        if let Some(value) = self.messages.remove(key) {
            self.coverage.invalidate(&value);
            self.bytes = self.bytes.saturating_sub(entry_bytes(key, &value));
            if let Some(id)=generation_id(&value) {self.generation_messages.remove(id);self.remove_activity(id);}
        }
    }
    pub fn message_activities(&self) -> Value {
        let mut result=serde_json::Map::new();
        for (id,activity) in &self.activities {
            let key=self.generation_messages.get(id).cloned().or_else(|| {
                let reference=&activity["final_message"];
                let sender=reference["sender_id"].as_str().or_else(||reference["sender_node_id_hex"].as_str())?;
                Some((sender.to_ascii_lowercase(),reference["message_id"].as_str()?.to_owned()))
            });
            if let Some(key)=key.filter(|key|self.messages.contains_key(key)) {
                result.insert(serde_json::to_string(&key).expect("string pair"),serde_json::json!({"generation_id":id,"tools":activity["tools"]}));
            }
        }
        Value::Object(result)
    }
    pub fn message_activity(&self, message: &Value) -> Option<&Value> {
        if let Some(id)=generation_id(message) { if let Some(value)=self.activities.get(id) {return Some(value);} }
        let key=message_key(message).ok()?;
        self.activities.values().find(|value| {
            let reference=&value["final_message"];
            reference["message_id"].as_str()==Some(key.1.as_str()) &&
                reference["sender_id"].as_str().or_else(||reference["sender_node_id_hex"].as_str())
                    .is_some_and(|sender|sender.eq_ignore_ascii_case(&key.0))
        })
    }
    /// Hydrate a canonical reply from the existing scoped durable metadata API.
    /// Navigation observation sequences are not live generation sequences.
    pub fn put_recorded_activity(&mut self, key:&(String,String), page:&Value)->Result<(),String> {
        let message=self.messages.get(key).ok_or("activity message is no longer cached")?;
        let id=generation_id(message).ok_or("message has no generation identity")?.to_owned();
        let observations=page["observations"].as_array().ok_or("invalid activity page")?;
        if observations.iter().any(|row|row["generation_id"].as_str()!=Some(&id)) {
            return Err("activity generation identity mismatch".into());
        }
        let rows=merge_activity(&[],observations,self.presentation_limit)?;
        let old=self.activities.get(&id);
        let mut value=old.cloned().unwrap_or_else(||serde_json::json!({"generation_id":id,"state":"completed"}));
        value["tools"]=serde_json::json!(rows.iter().map(|row|row["tool"].clone()).collect::<Vec<_>>());
        tools::merge(old,&mut value)?;
        value["history_loaded"]=serde_json::json!(page["next_cursor"].is_null());
        value["history_next_cursor"]=page["next_cursor"].clone();
        let bytes=retained_bytes(&value)+id.len()+128;
        let previous=old.map_or(0,|old|retained_bytes(old)+id.len()+128);
        self.admission_shortfall=self.retained_bytes().saturating_sub(previous).saturating_add(bytes).saturating_sub(self.admission_limit);
        if self.admission_shortfall>0 {return Err("message cache budget exhausted".into());}
        self.remove_activity(&id);
        self.activity_bytes+=bytes;
        self.activities.insert(id,value);
        Ok(())
    }
    fn remove_activity(&mut self,id:&str) {
        if let Some(value)=self.activities.remove(id) {self.activity_bytes=self.activity_bytes.saturating_sub(retained_bytes(&value)+id.len()+128);}
    }
    fn retire_generation(&mut self,id:&str) {
        if let Some(value)=self.generations.remove(id) {
            let bytes=retained_bytes(&value)+id.len()+128;
            self.generation_bytes=self.generation_bytes.saturating_sub(bytes);
            if value["tools"].as_array().is_some_and(|rows|!rows.is_empty()) {
                self.remove_activity(id);
                self.activity_bytes+=bytes;
                self.activities.insert(id.to_owned(),value);
            }
        }
    }
    fn remove_generation(&mut self, id: &str) {
        if let Some(value) = self.generations.remove(id) { self.generation_bytes = self.generation_bytes.saturating_sub(retained_bytes(&value) + id.len() + 128); }
    }
}

fn entry_bytes(key: &(String,String), value: &Value) -> usize {
    generation_id(value).map_or(0,|id|128+id.len()+key.0.len()+key.1.len()) + 192 + key.0.capacity() + key.1.capacity() + retained_bytes(value)
}

pub fn generation_id(value:&Value)->Option<&str> {
    value["payload"]["value"]["reply_generation_id"].as_str()
        .or_else(||value["payload"]["reply_generation_id"].as_str())
        .or_else(||value["body"]["reply_generation_id"].as_str()).filter(|id|!id.is_empty())
}
fn final_present(messages: &BTreeMap<(String,String),Value>, generations:&BTreeMap<String,(String,String)>, generation: &Value) -> bool {
    if generation["generation_id"].as_str().is_some_and(|id|generations.contains_key(id)) {return true;}
    let reference = &generation["final_message"];
    let sender = reference["sender_id"].as_str().or_else(||reference["sender_node_id_hex"].as_str());
    match (sender, reference["message_id"].as_str()) {
        (Some(sender),Some(id)) => messages.contains_key(&(sender.to_ascii_lowercase(),id.to_owned())),
        _ => false,
    }
}

/// Reclaim inactive scopes first, then ranges outside actual presented windows.
/// An active scope without a reported window remains protected during startup.
pub type ProtectedCache<'a> = (bool, BTreeSet<(String,String)>, &'a mut ThreadMessages);
pub fn trim_ranges(mut threads: Vec<ProtectedCache<'_>>, limit: usize) -> bool {
    let mut used=threads.iter().map(|(_,_,cache)|cache.retained_bytes()).sum::<usize>();
    threads.sort_by_key(|(active,_,cache)|(*active,cache.last_used));
    for (active,protected,cache) in threads {
        if used<=limit {return true;}
        if active && protected.is_empty(){continue;}
        let before=cache.retained_bytes();
        if !active {cache.generations.clear();cache.generation_bytes=0;cache.activities.clear();cache.activity_bytes=0;}
        let mut ordered=cache.messages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_,row)|order(row));
        let positions=ordered.iter().enumerate().filter(|(_, (key,_))|protected.contains(*key)).map(|(index,_)|index).collect::<Vec<_>>();
        let mut candidates=ordered.iter().enumerate().filter(|(_, (key,_))|!protected.contains(*key)).map(|(index,(key,_))|{
            let at=positions.partition_point(|position|*position<index);
            let distance=positions.get(at).map(|value|value.abs_diff(index)).into_iter()
                .chain(at.checked_sub(1).map(|at|positions[at].abs_diff(index))).min().unwrap_or(usize::MAX);
            (std::cmp::Reverse(distance), index, (*key).clone())
        }).collect::<Vec<_>>();
        candidates.sort();
        for (_,_,key) in candidates {
            if used.saturating_sub(before-cache.retained_bytes())<=limit {break;}
            cache.remove(&key);cache.evicted=true;
        }
        used=used.saturating_sub(before-cache.retained_bytes());
    }
    used<=limit
}

/// The host supplies activity/visibility, not another eviction algorithm.
/// Active or selected views are protected until their source ranges are known.
pub fn trim_inactive(mut threads: Vec<(bool, &mut ThreadMessages)>, limit: usize) -> bool {
    let mut used = threads.iter().map(|(_,thread)|thread.retained_bytes()).sum::<usize>();
    threads.sort_by_key(|(active, thread)| (*active, thread.last_used));
    for (active, thread) in threads {
        if used <= limit { return true; }
        if active { continue; }
        let before = thread.retained_bytes();
        thread.generations.clear(); thread.generation_bytes = 0; thread.activities.clear(); thread.activity_bytes=0;
        used = used.saturating_sub(before - thread.retained_bytes());
        let protected = BTreeSet::new();
        let before=thread.retained_bytes();
        thread.evict_to(before.saturating_sub(used.saturating_sub(limit)),&protected);
        used=used.saturating_sub(before-thread.retained_bytes());
    }
    used <= limit
}




