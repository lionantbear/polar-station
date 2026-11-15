use serde_json::Value;
/// Native derived layout wraps the shared, transport-neutral message cache.
#[derive(Clone,Default)]
pub(crate) struct Transcript {
    pub(crate) shared: polar_lgp_reader::ThreadMessages,
    pub(crate) cards: std::rc::Rc<std::cell::RefCell<crate::cards::Cache>>,
    pub(crate) cache_namespace: String,
}
impl std::ops::Deref for Transcript {
    type Target = polar_lgp_reader::ThreadMessages;
    fn deref(&self) -> &Self::Target { &self.shared }
}

impl Transcript {
    pub fn put_message(&mut self, thread:&str, value:Value)->Result<(),String> {
        let key=polar_lgp_reader::message_key(&value)?;
        let changed=self.shared.messages.get(&key)!=Some(&value);
        self.shared.put_message(thread,value)?;
        if changed { self.cards.borrow_mut().invalidate(&self.cache_namespace); }
        Ok(())
    }
    pub fn put_generation(&mut self,owner:&str,thread:&str,value:Value)->Result<(),String> {
        let prefix=self.cache_namespace.clone();
        if self.shared.put_generation(owner,thread,value)? { self.cards.borrow_mut().invalidate(&prefix); }
        Ok(())
    }
    pub fn put_recorded_activity(&mut self,key:&(String,String),page:&Value)->Result<(),String> {
        self.shared.put_recorded_activity(key,page)?;
        self.cards.borrow_mut().invalidate(&self.cache_namespace);
        Ok(())
    }
    pub fn merge_snapshot(&mut self,thread:&str,mut snapshot:Self)->Result<(),String> {
        let rows=snapshot.shared.sorted_messages();
        snapshot.shared.mark_range(&rows,None);
        self.shared.merge_snapshot(thread,snapshot.shared)?;
        self.cards.borrow_mut().invalidate(&self.cache_namespace);
        Ok(())
    }
    pub fn oldest(&self)->Option<Value> { self.shared.cursor(true) }

    pub fn attachment_rows(&self)->Value {
        let mut out=Vec::new();
        for ((sender,message),v) in &self.messages {for a in attachments(v){out.push(serde_json::json!({
            "sender_id":sender,"message_id":message,"attachment":a
        }));if out.len()>=512{return Value::Array(out);}}}
        Value::Array(out)
    }
}
pub(crate) fn generation_presentation(g:&Value)->polar_lgp_presentation::ActivityPresentation {
    serde_json::from_value(g["presentation"].clone()).unwrap_or_else(|_| {
        polar_lgp_presentation::ActivityPresentation::project(
            serde_json::from_value(g["state"].clone()).unwrap_or(polar_inhouse_model::ReplyGenerationState::Generating),
            g["phase"].as_str().unwrap_or(""),
            g["status_text"].as_str().unwrap_or(""),
        )
    })
}


pub(crate) fn body(v:&Value)->String {
    if let Some((body,title))=crate::state::inquiry::offer(v) {
        if let Some(metadata) = polar_station_session::routing::form_metadata(&body["package"]["metadata"]) {
            let mut text = format!("Task awaiting confirmation\n{}", metadata["draft_text"].as_str().unwrap_or(""));
            if let Some(node) = body["package"]["nodes"].as_array().and_then(|nodes|nodes.iter().find(|n|n["node_id"]=="target_session")) {
                for c in node["choices"].as_array().into_iter().flatten() {
                    text.push_str(&format!("\n• {} · {}\n  {}\n  {}",c["label"].as_str().unwrap_or("Session"),c["metadata"]["agent_type"].as_str().unwrap_or("terminal"),c["metadata"]["host"].as_str().unwrap_or("Host"),c["metadata"]["reason"].as_str().unwrap_or("")));
                }
            }
            text.push_str("\nOpen Inquiry in the Chat Bar to choose a target and edit the text.");
            return text;
        }
        return format!("Inquiry · {title}\n[Handle in Chat Bar]{}",body["summary"].as_str().map(|s|format!("\n{}",safe(s))).unwrap_or_default());
    }
    if v["payload"]["value"]["message_kind"]=="inquiry_graph.session.result" {return "Inquiry response · submitted".into();}
    polar_lgp_presentation::message_view_text(v)
}
pub(crate) fn attachments(v:&Value)->Vec<Value> { polar_lgp_presentation::message_view_attachments(v) }
/// Only this trusted projection emits display text. Untrusted ESC/C0/C1 and
/// directional override/isolate controls become visible notation, never VT.
pub(crate) fn safe(s:&str)->String{
    let mut out=String::with_capacity(s.len());for c in s.chars(){match c{
        '\n'|'\t'=>out.push(c),'\r'=>{},
        c if c.is_control()||matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')=>{use std::fmt::Write;let _=write!(out,"[U+{:04X}]",c as u32);},
        _=>out.push(c)
    }}out
}


impl Transcript {
    
    
}

/// Preserve append-only updates without repainting the previous transcript.
pub(crate) fn run_delta(old: &[polar_edge_presentation::TextRun], new: &[polar_edge_presentation::TextRun]) -> Option<Vec<polar_edge_presentation::TextRun>> {
    if old.is_empty() || new.len() < old.len() { return None; }
    let last = old.len()-1;
    if old[..last] != new[..last] || old[last].style != new[last].style || !new[last].text.starts_with(&old[last].text) { return None; }
    let mut delta = new[last..].to_vec();
    delta[0].text = delta[0].text[old[last].text.len()..].into();
    Some(delta)
}










