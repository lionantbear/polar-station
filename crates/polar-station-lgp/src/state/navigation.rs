//! Station transport adapter for the existing scoped causal SDK queries.
use super::*;

#[derive(Clone)]
pub(super) struct Read { pane:String, scope:Scope, key:(String,String), children:bool, window:bool, pub(super) repair:bool }

fn preview(value:&Value)->String {
    let author=polar_lgp_presentation::message_view_author(value).0;
    let text=crate::text::body(value).split_whitespace().collect::<Vec<_>>().join(" ");
    crate::text::safe(&format!("{author} · {}",text.chars().take(160).collect::<String>()))
}
fn key_value(key:&(String,String))->Value {json!({"sender_id":key.0,"message_id":key.1})}
fn locate_action(pane:&str,key:&(String,String))->Value {json!({"type":"locate","pane_id":pane,"sender_id":key.0,"message_id":key.1})}

impl Engine {
    /// Older servers already provide exact message-context pages. Walk those
    /// pages after subscribing; do not require an upgrade to repair a long gap.
    pub(super) fn request_repair(&mut self,scope:&Scope,out:&mut Vec<Effect>)->Result<(),String>{
        let Some(key)=self.threads.get(scope).and_then(|t|t.repair_after.clone()) else{return Ok(())};
        let Some(pane)=self.views.iter().find(|(_,v)|&v.scope==scope).map(|(id,_)|id.clone()) else{return Ok(())};
        self.request_navigation(&pane,key.clone(),None,false,true,out)?;
        for read in self.navigation.values_mut().filter(|read|read.scope==*scope&&read.pane==pane&&read.key==key&&read.window){read.repair=true;}
        self.status(scope,"loading","Restoring messages…",out);Ok(())
    }
    /// Older feeds cover only the latest page. Verify each visible historical
    /// anchor once per connection through the existing context query.
    pub(super) fn request_visible_verification(&mut self,scope:&Scope,out:&mut Vec<Effect>)->Result<(),String>{
        let Some(state)=self.threads.get(scope) else{return Ok(())};
        if state.supports_resume || state.repair_after.is_some() || self.pages.values().any(|page|&page.scope==scope) {return Ok(());}
        let selected=self.views.iter().filter(|(_,view)|&view.scope==scope)
            .filter_map(|(pane,view)|view.anchor.as_ref().map(|anchor|(pane.clone(),anchor.key.clone())))
            .find(|(_,key)|!state.verified_views.contains(key));
        if let Some((pane,key))=selected {
            self.request_navigation(&pane,key.clone(),None,false,true,out)?;
            if self.navigation.values().any(|read|read.scope==*scope&&read.pane==pane&&read.key==key) {
                self.threads.get_mut(scope).ok_or("closed Thread")?.verified_views.insert(key);
            }
        }
        Ok(())
    }
    pub(super) fn reference_menu(&self,pane:&str,out:&mut Vec<Effect>)->Result<(),String>{
        let view=self.views.get(pane).ok_or("unknown Pane")?;
        let thread=self.threads.get(&view.scope).ok_or("unknown Thread")?;
        let mut rows=thread.transcript.messages.values().collect::<Vec<_>>();
        rows.sort_by_key(|row|polar_lgp_reader::order(row));
        let mut entries=Vec::new();
        if !view.backstack.is_empty(){entries.push(json!({"title":"Return to previous reading position","input":{"type":"return_to_reading","pane_id":pane}}));}
        for row in rows.into_iter().rev().take(100){
            let key=polar_lgp_reader::message_key(row)?;
            let causal=&row["presentation"]["causal"];
            let sender=causal["key"]["sender_id"].as_str().or_else(||row["causal_sender_id"].as_str());
            let message=causal["key"]["message_id"].as_str().or_else(||row["causal_message_id"].as_str());
            if let (Some(sender),Some(message))=(sender,message){
                let label=causal["label"].as_str().unwrap_or("Linked to");
                entries.push(json!({"title":format!("{} — {label}",preview(row)),"input":locate_action(pane,&(sender.into(),message.into()))}));
            }
            entries.push(json!({"title":format!("Copy source · {}",preview(row)),"input":{"type":"copy_source","pane_id":pane,"sender_id":key.0,"message_id":key.1}}));
            entries.push(json!({"title":format!("Follow-ups · {}",preview(row)),"input":{"type":"followups","pane_id":pane,"sender_id":key.0,"message_id":key.1}}));
        }
        if entries.is_empty(){entries.push(json!({"title":"No messages loaded yet"}));}
        out.push(Effect::Activity{pane_id:pane.into(),items:json!({"title":"Message references","entries":entries})});Ok(())
    }

    pub(super) fn locate(&mut self,pane:&str,key:(String,String),out:&mut Vec<Effect>)->Result<(),String>{
        let scope=self.views.get(pane).ok_or("unknown Pane")?.scope.clone();
        if self.threads.get(&scope).is_some_and(|thread|thread.transcript.messages.contains_key(&key)) {
            self.focus_reference(pane,key)?;
            Ok(())
        }else{self.request_navigation(pane,key,None,false,false,out)}
    }
    fn focus_reference(&mut self,pane:&str,key:(String,String))->Result<(),String>{
        let view=self.views.get_mut(pane).ok_or("unknown Pane")?;
        let previous=view.anchor.clone().or_else(||view.display_window.anchors.last().cloned());
        view.reading.locate(key,previous);view.scroll_pending=None;view.dirty=true;Ok(())
    }
    pub(super) fn request_navigation(&mut self,pane:&str,key:(String,String),after:Option<Value>,children:bool,window:bool,out:&mut Vec<Effect>)->Result<(),String>{
        validate_id(&key.0)?;validate_id(&key.1)?;
        let scope=self.views.get(pane).ok_or("unknown Pane")?.scope.clone();
        if window && self.pages.values().any(|page|page.scope==scope){return Ok(());}
        let lease=self.threads.get(&scope).and_then(|thread|thread.lease.clone()).ok_or("Thread is offline")?;
        // Reuse the same fragment assembler and per-target request admission as
        // history pages. Late responses for replaced navigation are ignored.
        let old=self.navigation.iter().filter(|(_,read)|read.pane==pane).map(|(id,_)|id.clone()).collect::<Vec<_>>();
        for id in old {self.navigation.remove(&id);self.pages.remove(&id);
            self.request(&scope.target_id,json!({"op":"unsubscribe","expected_node_id":scope.node_id,"subscription_id":id}),Purpose::Release,out)?;}
        let id=self.next_id("reference")?;
        let mut payload=json!({"op":if children{"children"}else{"context"},"expected_node_id":scope.node_id,"lease_id":lease,"subscription_id":id,"key":key_value(&key)});
        if children {payload["limit"]=json!(50);payload["after"]=after.unwrap_or(Value::Null);}else{payload["radius"]=json!(50);}
        self.request(&scope.target_id,payload,Purpose::Page{scope:scope.clone(),subscription:id.clone()},out)?;
        self.pages.insert(id.clone(),ReadPage{scope:scope.clone(),assembly:Assembler::default(),sequence:0,keys:Vec::new()});
        self.navigation.insert(id,Read{pane:pane.into(),scope,key,children,window,repair:false});
        if !window {out.push(Effect::Activity{pane_id:pane.into(),items:json!({"title":"Message references","entries":[{"title":"Loading…"}]})});}Ok(())
    }

    pub(super) fn navigation_result(&mut self,read:&Read,payload:&Value,out:&mut Vec<Effect>)->Result<(),String>{
        if self.views.get(&read.pane).is_none_or(|view|view.scope!=read.scope){return Ok(());}
        let result=&payload["result"];
        if read.children {
            let mut entries=Vec::new();
            for located in result["messages"].as_array().ok_or("invalid follow-up page")? {
                if located["owner_node_id_hex"].as_str()!=Some(&read.scope.node_id){return Err("foreign follow-up owner".into());}
                let row=&located["message"];let key=polar_lgp_reader::message_key(row)?;
                entries.push(json!({"title":preview(row),"input":locate_action(&read.pane,&key)}));
            }
            if entries.is_empty(){entries.push(json!({"title":if result["available"]==false{"Message unavailable"}else{"No direct follow-ups"}}));}
            if !result["next_cursor"].is_null(){entries.push(json!({"title":"More follow-ups","input":{"type":"followups","pane_id":read.pane,"sender_id":read.key.0,"message_id":read.key.1,"after":result["next_cursor"]}}));}
            out.push(Effect::Activity{pane_id:read.pane.clone(),items:json!({"title":"Direct follow-ups","entries":entries})});return Ok(());
        }
        let target=&result["target"];
        if target.is_null() && read.repair {
            if let Some(state)=self.threads.get_mut(&read.scope){state.repair_after=None;state.transcript.shared.resume_message=state.snapshot_boundary.clone();state.status="live".into();state.connection_message="Previous message anchor is unavailable; current page refreshed.".into();}
            return Ok(());
        }
        if target.is_null(){out.push(Effect::Activity{pane_id:read.pane.clone(),items:json!({"title":"Message unavailable","entries":[]})});return Ok(());}
        if target["owner_node_id_hex"].as_str()!=Some(&read.scope.node_id){return Err("foreign reference owner".into());}
        let row=&target["message"];
        let thread=row["thread_id"].as_str().ok_or("reference has no Thread")?;
        if thread!=read.scope.thread {
            out.push(Effect::Activity{pane_id:read.pane.clone(),items:json!({"title":"Reference preview · another Thread","entries":[
                {"title":preview(row)}, {"title":crate::text::body(row).chars().take(6000).collect::<String>()},
                {"title":"Open Thread in new Pane","input":{"type":"open_reference_thread","pane_id":read.pane,"thread_id":thread}}
            ]})});return Ok(());
        }
        let state=self.threads.get_mut(&read.scope).ok_or("closed Thread")?;
        for row in result["messages"].as_array().ok_or("invalid reference context")? {state.transcript.put_message(thread,row.clone())?;}
        state.transcript.shared.mark_page(result["messages"].as_array().ok_or("invalid reference context")?,None,result["has_before"].as_bool(),result["has_after"].as_bool());
        if read.repair {
            let rows=result["messages"].as_array().ok_or("invalid recovery context")?;
            let next=rows.last().map(polar_lgp_reader::message_key).transpose()?.ok_or("empty recovery page")?;
            if result["has_after"]==true && next==read.key {return Err("message recovery did not advance".into());}
            state.transcript.shared.acknowledge_message(next.clone())?;
            state.repair_after=if result["has_after"]==true {Some(next)}else{None};
            if state.repair_after.is_none(){state.status="live".into();state.connection_message.clear();}
        }
        if read.window {if let Some(view)=self.views.get_mut(&read.pane){view.dirty=true;}return Ok(());}
        state.has_more|=result["has_before"].as_bool().unwrap_or(false);
        self.focus_reference(&read.pane,read.key.clone())?;
        Ok(())
    }
}


