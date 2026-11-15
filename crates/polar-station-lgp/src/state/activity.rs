//! Historical tool metadata uses the same scoped API and bounded request queue
//! as the existing Activity menu. It is fetched only for an expanded reply.
use super::*;

impl Engine {
    pub(super) fn request_inline_activity(&mut self,pane:&str,key:(String,String),out:&mut Vec<Effect>)->Result<(),String> {
        let scope=self.views.get(pane).ok_or("unknown Pane")?.scope.clone();
        let thread=self.threads.get(&scope).ok_or("unknown Thread")?;
        let message=thread.transcript.messages.get(&key).ok_or("activity message is no longer cached")?;
        let activity=thread.transcript.message_activity(message);
        if activity.is_some_and(|value|value["history_loaded"]==true) {return Ok(());}
        if self.pending.values().any(|pending|matches!(&pending.purpose,
            Purpose::Activity{scope:s,sender,message,inline:true,..} if s==&scope && sender==&key.0 && message==&key.1)) {return Ok(());}
        let after=activity.and_then(|value|value["history_next_cursor"].as_i64());
        let lease=thread.lease.clone().ok_or("conversation is not connected")?;
        self.request(&scope.target_id,json!({"op":"activity","expected_node_id":scope.node_id,"lease_id":lease,
            "key":{"sender_id":key.0,"message_id":key.1},"after_sequence":after}),
            Purpose::Activity{scope:scope.clone(),pane:pane.into(),sender:key.0,message:key.1,inline:true,after},out).map(|_|())
    }

    pub(super) fn inline_activity_response(&mut self,scope:Scope,pane:String,key:(String,String),after:Option<i64>,value:Value,out:&mut Vec<Effect>)->Result<(),String> {
        if self.views.get(&pane).is_none_or(|view|view.scope!=scope) {return Ok(());}
        if value["node_id"].as_str()!=Some(&scope.node_id) {return Err("activity target identity mismatch".into());}
        let page=&value["page"];
        if page["next_cursor"].as_i64().is_some_and(|next|after.is_some_and(|after|next<=after)) {return Err("tool history cursor did not advance".into());}
        let Some(thread)=self.threads.get_mut(&scope) else{return Ok(());};
        let Some(id)=thread.transcript.messages.get(&key).and_then(polar_lgp_reader::generation_id).map(str::to_owned) else{return Ok(());};
        thread.transcript.put_recorded_activity(&key,page)?;
        for view in self.views.values_mut().filter(|view|view.scope==scope) {view.dirty=true;}
        out.push(Effect::NeedsRender);
        if !page["next_cursor"].is_null() && self.views.get(&pane).is_some_and(|view|view.tools.get(&id).is_some_and(|state|state.open)) {
            self.request_inline_activity(&pane,key,out)?;
        }
        Ok(())
    }
}


