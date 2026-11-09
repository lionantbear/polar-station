use std::collections::{BTreeMap, BTreeSet, VecDeque};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use polar_tmux_protocol::{DesktopSendError,LocalExtensionRequestV1,LocalExtensionCancelV1,LocalClientFrame,LocalHostFrame,LocalExtensionResultV1,POLAR_ACCESS_EXTENSION};
use crate::{record::Assembler,text::Transcript};
mod navigation;
mod activity;
mod composer;
mod idea;
mod idea_codex;
mod idea_document;
pub use idea_document::{IdeaNodeBlock, IdeaSelectedNode};
pub(crate) mod inquiry;
pub use inquiry::InquiryAction;
use idea::{DraftContext, IdeaView};
use polar_ideacave_presentation::controls::{Action as IdeaAction, Control as IdeaControl, ControlsState};

#[derive(Debug,Clone,PartialEq,Eq,PartialOrd,Ord,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub target_id:String,pub node_id:String,pub owner:String,pub thread:String,pub actor:String,
}
impl Scope {
    pub fn validate(&self)->Result<(),String>{
        for value in [&self.target_id,&self.node_id,&self.owner,&self.thread,&self.actor]{
            polar_tmux_protocol::validate_stable_identifier(value,"LGP scope").map_err(|e|e.to_string())?;
        }Ok(())
    }
}
/// Correlates a native picker request without making its candidate selection
/// the active conversation. Empty context is the legacy standalone browser.
#[derive(Debug,Clone,Default,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowseContext { pub window_id:String, pub generation:u64 }
#[derive(Debug,Deserialize)]
#[serde(tag="type",rename_all="snake_case",deny_unknown_fields)]
pub enum Input {
    Connected{generation:u64,#[serde(default)] extensions:Vec<String>},
    Disconnected{generation:u64},
    RequestAdmitted{generation:u64,request_id:String},
    RequestRejected{generation:u64,request_id:String,reason:DesktopSendError},
    InquiryAction{window_id:String,action:InquiryAction,#[serde(default)] token:Option<String>,#[serde(default)] value:Option<String>},
    InquiryEnvironment{window_id:String,foreground:bool,composing:bool},
    TerminalViewResult{window_id:String,token:String,ready:bool,#[serde(default)] error:Option<String>},
    ListTargets,
    CachePolicy{policy:polar_lgp_reader::CachePolicy},
    Browse{target_id:String,#[serde(default)] context:BrowseContext},
    Threads{target_id:String,node_id:String,channel_id:String,#[serde(default)] context:BrowseContext},
    Open{pane_id:String,window_id:String,scope:Scope},
    OpenIdea{pane_id:String,window_id:String,view_id:String},
    IdeaHost{host_id:String,supported:bool},
    IdeaPickSession{pane_id:String,view_id:String,owner_host_id:String,endpoint_id:String,thread_id:String,#[serde(default)] title:String},
    IdeaBind{window_id:String,#[serde(default)] binding_id:Option<String>},
    IdeaReference{window_id:String,operation:String,#[serde(default)] index:usize},
    IdeaNodeInstruction{window_id:String,#[serde(default)] view_id:Option<String>,node_uuid:String,title:String,text:String,#[serde(default)] remove:bool},
    IdeaSessions,
    IdeaShowSession{window_id:String},
    IdeaCheckReceipt{window_id:String},
    IdeaState{pane_id:String,view_id:String,state:Box<ControlsState>},
    IdeaAction{window_id:String,action:IdeaAction,#[serde(default)] query:String},
    SelectionChanged{pane_id:String},
    ActivateComposer{pane_id:String},
    Deselect{window_id:String},
    WindowClosed{window_id:String},
    Close{pane_id:String},
    Draft{window_id:String,text:String},
    Send{window_id:String,created_at_secs:u64},
    Frame{generation:u64,frame:Value},
    Older{pane_id:String},
    Retry{pane_id:String},
    ResetDisplay{pane_id:String},
    Tick{now_ms:u64},
    Flush,
    RenderGuard{pane_id:String,blocked:bool},
    RenderResult{pane_id:String,revision:u64,accepted:bool},
    RetryDisplay{pane_id:String},
    Viewport{pane_id:String,at_bottom:bool,revision:u64,visible:bool,#[serde(default)] top_row:Option<usize>,#[serde(default)] visible_rows:Option<usize>},
    JumpToLatest{pane_id:String},
    RenderMetrics{pane_id:String,columns:usize},
    ExportDrafts,
    RestoreDrafts{drafts:Value},
    Search{pane_id:String,query:String},
    References{pane_id:String},
    ActivateRow{pane_id:String,revision:u64,row:usize,column:usize},
    CopySource{pane_id:String,sender_id:String,message_id:String},
    Locate{pane_id:String,sender_id:String,message_id:String},
    Followups{pane_id:String,sender_id:String,message_id:String,#[serde(default)] after:Option<Value>},
    ReturnToReading{pane_id:String},
    OpenReferenceThread{pane_id:String,thread_id:String},
    GenerationActivity{pane_id:String,generation_id:String},
    Activity{pane_id:String,#[serde(default)] sender_id:Option<String>,#[serde(default)] message_id:Option<String>,#[serde(default)] after_sequence:Option<i64>},
}
#[derive(Debug,Clone,Serialize)]
#[serde(tag="type",rename_all="snake_case")]
pub enum Effect {
    Request{frame:Value},
    Render{pane_id:String,base_revision:u64,next_revision:u64,operation:u32,text:String},
    NeedsRender,
    ReferenceControls{pane_id:String,revision:u64,items:Value,can_return:bool},
    ReadingHint{pane_id:String,visible:bool,label:String,retry:bool},
    ScrollLatest{pane_id:String},
    CopySource{pane_id:String,text:String},
    ScrollToRow{pane_id:String,row:usize},
    Inquiry{window_id:String,pane_id:String,view:Value,focus:bool},
    EnsureTerminalVisible{window_id:String,pane_id:String,token:String,target:Value},
    ReconnectHost,
    Composer{window_id:String,pane_id:String,scope:Scope,text:String,draft_revision:u64,can_send:bool,pending:bool,status:String,connection:String,can_load_older:bool,can_reconnect:bool,target_title:String,channel_id:String,channel_title:String,thread_title:String},
    IdeaComposer{window_id:String,pane_id:String,view_id:String,text:String,node_blocks:Vec<IdeaNodeBlock>,selected_node:Option<IdeaSelectedNode>,document_text:String,draft_revision:u64,mode_label:&'static str,placeholder:&'static str,can_send:bool,status:String,selection:String,controls:Vec<IdeaControl>,can_search:bool,binding_id:Option<String>,target_label:String,references:Value,can_open_session:bool,can_check_receipt:bool},
    IdeaLocateReference{pane_id:String,view_id:String,reference:Value},
    IdeaCommand{pane_id:String,view_id:String,action:IdeaAction,query:String},
    FocusComposer{window_id:String,pane_id:String},
    OpenReferenceThread{window_id:String,scope:Scope},
    Status{pane_id:String,state:String,message:String},
    Targets{items:Value},Channels{target_id:String,node_id:String,context:BrowseContext,items:Value},Threads{target_id:String,node_id:String,channel_id:String,context:BrowseContext,items:Value},
    BrowseError{context:BrowseContext,message:String},
    Attachments{pane_id:String,scope:Scope,items:Value},
    Drafts{value:Value},
    SearchResults{pane_id:String,items:Value},
    Activity{pane_id:String,items:Value},
    ComposerClosed{window_id:String},
    Error{message:String},
}
#[derive(Default,Clone,Serialize,Deserialize)]
struct Draft {#[serde(default)] node_blocks:Vec<IdeaNodeBlock>,#[serde(default)] idea_target:Option<idea_codex::Target>,#[serde(default)] idea_submission:Option<idea_codex::Submission>,#[serde(default)] idea_context:Option<idea_codex::Context>,text:String,revision:u64,#[serde(skip)] pending:Option<String>,#[serde(skip)] status:String}
struct PendingRender { revision:u64, text:String, window:crate::window::Window }
struct View {tools:BTreeMap<String,crate::window::Disclosure>,window:String,scope:Scope,display_window:crate::window::Window,scroll_pending:Option<usize>,visible_rows:usize,revision:u64,rendered:String,reading:polar_lgp_reader::ReadingState,dirty:bool,blocked:bool,columns:usize,pending_render:Option<PendingRender>,render_failed:bool,visible:bool,hint:String}
fn reference_controls(pane:&str,view:&View)->Effect {
    Effect::ReferenceControls{pane_id:pane.into(),revision:view.revision,items:json!(view.display_window.references),can_return:!view.backstack.is_empty()}
}

impl std::ops::Deref for View {type Target=polar_lgp_reader::ReadingState;fn deref(&self)->&Self::Target{&self.reading}}
impl std::ops::DerefMut for View {fn deref_mut(&mut self)->&mut Self::Target{&mut self.reading}}
struct ThreadState {
    lease:Option<String>,renew_at:u64,subscription:Option<String>,sequence:u64,boot:Option<String>,supports_resume:bool,repair_after:Option<(String,String)>,snapshot_boundary:Option<(String,String)>,verified_views:BTreeSet<(String,String)>,
    new_message:Option<(String,String)>,record:Assembler,transcript:Transcript,staging:Option<Transcript>,has_more:bool,status:String,connection_message:String,resync_attempts:u8,
}
impl Default for ThreadState {
    fn default()->Self{Self{lease:None,renew_at:0,subscription:None,sequence:0,boot:None,supports_resume:false,repair_after:None,snapshot_boundary:None,verified_views:BTreeSet::new(),new_message:None,record:Assembler::default(),
        transcript:Transcript::default(),staging:None,has_more:false,status:"offline".into(),connection_message:String::new(),resync_attempts:0}}
}
#[derive(Clone,PartialEq,Eq)]
enum Purpose {IdeaCodex(idea_codex::Operation),ListTargets,Describe{target:String,context:BrowseContext},Channels{target:String,node:String,context:BrowseContext},Threads{target:String,node:String,channel:String,context:BrowseContext},
    Catalog{target:String,node:String,subscription:String},
    Bind(Scope),Subscribe{scope:Scope,subscription:String},Renew{scope:Scope,lease:String},
    Send{scope:Scope,window:String,revision:u64,text:String},Release,ReleaseScope(Scope),
    Inquiry{window:String,key:inquiry::Key,operation:inquiry::Operation,view_token:Option<String>},
    Page{scope:Scope,subscription:String},Activity{scope:Scope,pane:String,sender:String,message:String,inline:bool,after:Option<i64>},}
impl Purpose {
    fn target(&self) -> Option<&str> {
        match self {
            Self::Describe{target,..}|Self::Channels{target,..}|Self::Threads{target,..}|Self::Catalog{target,..} => Some(target),
            Self::Bind(scope)|Self::ReleaseScope(scope)|Self::Subscribe{scope,..}|Self::Renew{scope,..}|
            Self::Send{scope,..}|Self::Page{scope,..}|Self::Activity{scope,..} => Some(&scope.target_id),
            Self::Inquiry{key,..}=>Some(&key.scope.target_id),
            Self::IdeaCodex(_)|Self::ListTargets|Self::Release => None,
        }
    }
}
#[derive(Clone)]
struct CatalogView {target:String,node:String,channel:Option<String>,context:BrowseContext}
struct CatalogFeed {subscription:String,sequence:u64,boot:Option<String>}
struct ReadPage {scope:Scope,assembly:Assembler,sequence:u64,keys:Vec<(String,String)>}
struct Pending {purpose:Purpose,deadline:u64,target:String,sent:bool}
// Leave headroom in the shared 16-slot Host executor. One unavailable Polar
// cannot consume every native request slot while its SSH connection recovers.
const MAX_QUEUED_REQUESTS: usize = 128;
const MAX_ACTIVE_REQUESTS: usize = 12;
const MAX_ACTIVE_PER_TARGET: usize = 4;
/// One state owner shared by native adapters. Multiple Panes in the same
/// instance/actor/Thread share one canonical cache and one live subscription.
pub struct Engine {
    idea_codex:idea_codex::Codex,
    inquiries:inquiry::InquiryState,
    activity_selection:Option<(String,String,String)>,activity_rows:Vec<Value>,
    cache_policy:polar_lgp_reader::CachePolicy,
    id:String,counter:u64,generation:u64,ready:bool,now_ms:u64,
    card_cache:std::rc::Rc<std::cell::RefCell<crate::cards::Cache>>,threads:BTreeMap<Scope,ThreadState>,views:BTreeMap<String,View>,
    selected:BTreeMap<String,String>,drafts:BTreeMap<(String,DraftContext),Draft>,ideas:BTreeMap<String,IdeaView>,
    pending:BTreeMap<String,Pending>,queued:VecDeque<(String,Value)>,composer_cache:BTreeMap<String,String>,
    pages:BTreeMap<String,ReadPage>,navigation:BTreeMap<String,navigation::Read>,
    catalogs:BTreeMap<(String,String),CatalogFeed>,catalog_views:BTreeMap<String,CatalogView>,catalog_dirty:Vec<Purpose>,
    target_titles:BTreeMap<String,String>,channel_titles:BTreeMap<(String,String,String),String>,
    thread_titles:BTreeMap<(String,String,String),(String,String)>,
}
impl Default for Engine {fn default()->Self{Self::new()}}
impl Engine {
    pub fn new()->Self{Self{idea_codex:Default::default(),inquiries:Default::default(),cache_policy:Default::default(),activity_selection:None,activity_rows:Vec::new(),id:polar_identity::generation::new_uuid_v7().to_string(),counter:0,generation:0,ready:false,now_ms:0,
        card_cache:Default::default(),threads:BTreeMap::new(),views:BTreeMap::new(),selected:BTreeMap::new(),drafts:BTreeMap::new(),ideas:BTreeMap::new(),
        pending:BTreeMap::new(),queued:VecDeque::new(),composer_cache:BTreeMap::new(),pages:BTreeMap::new(),navigation:BTreeMap::new(),catalogs:BTreeMap::new(),catalog_views:BTreeMap::new(),catalog_dirty:Vec::new(),target_titles:BTreeMap::new(),channel_titles:BTreeMap::new(),thread_titles:BTreeMap::new()}}
    pub fn dispatch(&mut self,input:Input)->Result<Vec<Effect>,String>{
        let mut effects=Vec::new();
        match input {
            Input::TerminalViewResult{window_id,token,ready,error}=>{
                if token.starts_with("idea-session:") {if !ready {effects.push(Effect::Error{message:error.unwrap_or_else(||"Original session view is unavailable".into())});}}
                else {self.inquiry_terminal_result(&window_id,&token,ready,error,&mut effects);}
            },
            Input::InquiryEnvironment{window_id,foreground,composing}=>self.inquiry_environment(window_id,foreground,composing),
            Input::InquiryAction{window_id,action,token,value}=>self.inquiry_action(window_id,action,token,value,&mut effects)?,
            Input::CachePolicy{policy}=>{policy.validate()?;self.cache_policy=policy;self.card_cache.borrow_mut().set_limit(policy.presentation_bytes());},
            Input::Connected{generation,extensions}=>{
                if generation<self.generation || (generation==self.generation && self.ready){return Ok(effects);}
                self.disconnect(&mut effects);
                self.generation=generation;self.ready=extensions.iter().any(|e|e==POLAR_ACCESS_EXTENSION);
                if self.ready {
                    self.request("embedded",json!({"op":"list_targets"}),Purpose::ListTargets,&mut effects)?;
                    for scope in self.views.values().map(|view|view.scope.clone()).collect::<BTreeSet<_>>(){self.bind(&scope,&mut effects)?;}
                    for view in self.catalog_views.values().cloned().collect::<Vec<_>>() {
                        self.request(&view.target,json!({"op":"describe"}),Purpose::Describe{target:view.target.clone(),context:view.context},&mut effects)?;
                    }
                }
            },
            Input::Disconnected{generation}=>{if generation>=self.generation{self.disconnect(&mut effects);}},
            Input::RequestAdmitted{generation,request_id}=>{
                if generation==self.generation {
                    if let Some(Pending{purpose:Purpose::Send{scope,window,..},..})=self.pending.get(&request_id) {
                        if let Some(pane)=self.selected.get(window) {
                            if let Some(view)=self.views.get_mut(pane).filter(|v|&v.scope==scope) {
                                view.tail=true;view.jump_pending=true;view.dirty=true;
                            }
                        }
                    }
                }
            },
            Input::RequestRejected{generation,request_id,reason}=>{
                if generation!=self.generation{return Ok(effects);}
                if let Some(pending)=self.pending.remove(&request_id){
                    self.queued.retain(|(id,_)|id!=&request_id);
                    // Local admission failed: no frame was sent. Preserve the
                    // real transport state and every unrelated conversation.
                    if let Purpose::Send{scope,window,..}=pending.purpose {
                        if let Some(draft)=self.drafts.get_mut(&(window,scope.clone().into())){draft.pending=None;draft.status=format!("Not sent — {reason}");}
                        self.status(&scope,"send_rejected",&reason.to_string(),&mut effects);
                    }else{if let Purpose::IdeaCodex(op)=&pending.purpose { self.idea_codex_not_sent(op); } self.failed(pending.purpose,&reason.to_string(),&mut effects);}
                }
            },
            Input::ListTargets=>{self.request("embedded",json!({"op":"list_targets"}),Purpose::ListTargets,&mut effects)?;},
            Input::Browse{target_id,context}=>{
                self.catalog_views.insert(context.window_id.clone(),CatalogView{target:target_id.clone(),node:String::new(),channel:None,context:context.clone()});
                self.request(&target_id,json!({"op":"describe"}),Purpose::Describe{target:target_id.clone(),context},&mut effects)?;},
            Input::Threads{target_id,node_id,channel_id,context}=>{
                self.catalog_views.insert(context.window_id.clone(),CatalogView{target:target_id.clone(),node:node_id.clone(),channel:Some(channel_id.clone()),context:context.clone()});
                self.catalog_query(Purpose::Threads{target:target_id,node:node_id,channel:channel_id,context},&mut effects)?;
            },
            Input::Open{pane_id,window_id,scope}=>{
                scope.validate()?;validate_id(&pane_id)?;validate_id(&window_id)?;
                if self.ideas.contains_key(&pane_id){return Err("changing content identity requires a new Pane binding".into());}
                if scope.actor!=scope.node_id || scope.owner!=scope.node_id{return Err("the current local-owner access profile cannot impersonate a remote owner or actor".into());}
                if self.views.len()>=64&&!self.views.contains_key(&pane_id){return Err("too many LGP Panes".into());}
                if let Some(old)=self.views.get(&pane_id){if old.scope!=scope{return Err("changing content identity requires a new native Surface and Pane binding".into());}}
                let first=!self.views.values().any(|view|view.scope==scope);
                let thread=self.threads.entry(scope.clone()).or_default();
                thread.transcript.cards=self.card_cache.clone();
                thread.transcript.cache_namespace=format!("{}:",serde_json::to_string(&scope).map_err(|e|e.to_string())?);
                thread.transcript.shared.last_used=self.now_ms;
                // A legacy unscoped draft is adopted once, never copied into every window.
                let restored=self.drafts.remove(&(String::new(),scope.clone().into())).unwrap_or_default();
                self.drafts.entry((window_id.clone(),scope.clone().into())).or_insert(restored);
                self.views.entry(pane_id.clone()).or_insert(View{tools:Default::default(),window:window_id.clone(),scope:scope.clone(),display_window:Default::default(),scroll_pending:None,visible_rows:24,revision:1,rendered:String::new(),reading:Default::default(),dirty:true,blocked:false,columns:80,pending_render:None,render_failed:false,visible:true,hint:String::new()});
                // Registration is never an implicit user activation.
                if first&&self.ready{self.bind(&scope,&mut effects)?;}
            },
            Input::IdeaHost{host_id,supported}=>{validate_id(&host_id)?;self.idea_codex.host=supported.then_some(host_id);},
            Input::IdeaShowSession{window_id}=>self.idea_show_session(&window_id)?,
            Input::IdeaCheckReceipt{window_id}=>self.idea_check_receipt(&window_id)?,
            Input::IdeaNodeInstruction{window_id,view_id,node_uuid,title,text,remove}=>self.idea_node_instruction(&window_id,view_id.as_deref(),node_uuid,title,text,remove)?,
            Input::IdeaSessions=>{},
            Input::IdeaPickSession{pane_id,view_id,owner_host_id,endpoint_id,thread_id,title}=>self.idea_pick_session(pane_id,view_id,idea_codex::Target{owner_host_id,endpoint_id,thread_id,title})?,
            Input::IdeaBind{window_id,binding_id}=>self.idea_bind(&window_id,binding_id)?,
            Input::IdeaReference{window_id,operation,index}=>self.idea_reference(&window_id,&operation,index,&mut effects)?,
            Input::OpenIdea{pane_id,window_id,view_id}=>self.open_idea(pane_id,window_id,view_id)?,
            Input::IdeaState{pane_id,view_id,state}=>{
                if state.selection.len()>4096 || state.status.len()>8192 {return Err("Idea status is too large".into());}
                if let Some(view)=self.ideas.get_mut(&pane_id).filter(|v|v.identity==view_id){view.state = *state;}
            },
            Input::IdeaAction{window_id,action,query}=>self.idea_action(&window_id,action,query,&mut effects)?,
            Input::SelectionChanged{pane_id}=>self.observe_composer_selection(&pane_id,&mut effects)?,
            Input::ActivateComposer{pane_id}=>self.activate_composer(&pane_id,&mut effects),
            Input::Deselect{window_id}=>self.clear_composer_selection(window_id,&mut effects),
            Input::Close{pane_id}=>{self.inquiry_cancel_source(&pane_id);self.close_pane(&pane_id,&mut effects)?;},
            Input::WindowClosed{window_id}=>{
                self.inquiry_close_window(&window_id);
                self.catalog_views.remove(&window_id);
                self.ideas.retain(|_,v|v.window!=window_id);
                for pane in self.views.iter().filter(|(_,v)|v.window==window_id).map(|(p,_)|p.clone()).collect::<Vec<_>>() {
                    self.inquiry_cancel_source(&pane);self.close_pane(&pane,&mut effects)?;
                }
                self.selected.remove(&window_id);self.composer_cache.remove(&window_id);
            },
            Input::Draft{window_id,text}=>{
                if text.len()>32*1024{return Err("draft exceeds 32 KiB".into());}
                let key=self.draft_key(&window_id)?;
                if matches!(key.1,DraftContext::Idea(_)) {if let Some(d)=self.drafts.get(&key) {idea_document::validate_document(&text,&d.node_blocks)?;}}
                let draft=self.drafts.entry(key).or_default();
                if text.is_empty() && draft.idea_context.as_ref().is_some_and(|c|c.references.is_empty()) {draft.idea_context=None;}
                if draft.text!=text{draft.text=text;draft.revision=draft.revision.checked_add(1).ok_or("draft version exhausted")?;}
            },
            Input::Send{window_id,created_at_secs}=>{
                if self.idea_key(&window_id).is_ok() {self.idea_send(&window_id)?;self.idea_composer_effects(&mut effects);self.flush_requests(&mut effects);return Ok(effects);}
                if self.composer_scope(&window_id).ok().is_some_and(|(_,scope)|self.inquiries.draft_context(&window_id,&scope).is_some()||self.inquiries.can_send(&window_id,&scope,"").is_some()) {
                    self.inquiry_send(&window_id,&mut effects)?;self.composer_effects(&mut effects);self.inquiry_effects(&mut effects);self.flush_requests(&mut effects);return Ok(effects);
                }
                let (_,scope)=self.composer_scope(&window_id)?;
                let lease=self.threads.get(&scope).filter(|t|t.status=="live"&&t.subscription.is_some()).and_then(|t|t.lease.clone()).filter(|_|self.ready).ok_or("Thread is not connected")?;
                let draft=self.drafts.get(&(window_id.clone(),scope.clone().into())).cloned().unwrap_or_default();
                if draft.pending.is_some()||draft.text.trim().is_empty()||created_at_secs==0{return Err("send is not enabled".into());}
                let intent=polar_identity::generation::new_uuid_v7().to_string();
                let request=self.request(&scope.target_id,json!({"op":"send","expected_node_id":scope.node_id,"lease_id":lease,
                    "client_intent_id":intent,"created_at_secs":created_at_secs,"text":draft.text}),
                    Purpose::Send{scope:scope.clone(),window:window_id.clone(),revision:draft.revision,text:draft.text},&mut effects)?;
                let draft=self.drafts.get_mut(&(window_id,scope.clone().into())).ok_or("missing draft")?;draft.pending=Some(request);draft.status="Sending".into();
            },
            Input::Frame{generation,frame}=>{if generation==self.generation&&(self.ready||self.idea_codex.host.is_some()){self.frame(frame,&mut effects)?;}},
            Input::Older{pane_id}=>{self.load_older(&pane_id,&mut effects)?;},
            Input::Retry{pane_id}=>{let scope=self.views.get(&pane_id).ok_or("unknown Pane")?.scope.clone();
                if let Some(t)=self.threads.get_mut(&scope){t.resync_attempts=0;}
                if self.ready{self.bind(&scope,&mut effects)?;}else{effects.push(Effect::ReconnectHost);}},
            Input::ResetDisplay{pane_id}=>{if let Some(view)=self.views.get_mut(&pane_id){view.revision=1;view.display_window=Default::default();view.rendered.clear();view.pending_render=None;view.render_failed=false;view.dirty=true;}},
            Input::Tick{now_ms}=>{
                self.inquiry_expire_native(now_ms);
                self.now_ms=self.now_ms.max(now_ms);
                let expired=self.pending.iter().filter(|(_,p)|p.deadline<=self.now_ms).map(|(k,_)|k.clone()).collect::<Vec<_>>();
                for id in expired{if let Some(pending)=self.pending.remove(&id){self.failed(pending.purpose,"Operation timed out; check before retrying",&mut effects);}}
                let renew=self.threads.iter().filter(|(_,t)|t.lease.is_some()&&t.renew_at<=self.now_ms).map(|(s,t)|(s.clone(),t.lease.clone().unwrap_or_default())).collect::<Vec<_>>();
                if self.ready{for (scope,lease) in renew{
                    self.request(&scope.target_id,json!({"op":"renew","expected_node_id":scope.node_id,"lease_id":lease}),Purpose::Renew{scope:scope.clone(),lease:lease.clone()},&mut effects)?;
                    if let Some(t)=self.threads.get_mut(&scope){t.renew_at=self.now_ms.saturating_add(60_000);}
                }}
            },
            Input::RenderMetrics{pane_id,columns}=>{if let Some(v)=self.views.get_mut(&pane_id){let columns=columns.clamp(12,512);if v.columns!=columns {v.columns=columns;v.dirty=true;}}},
            Input::RenderGuard{pane_id,blocked}=>{if let Some(v)=self.views.get_mut(&pane_id){v.blocked=blocked;}},
            Input::RenderResult{pane_id,revision,accepted}=>{
                if let Some(view)=self.views.get_mut(&pane_id) {
                    if view.pending_render.as_ref().is_some_and(|pending|pending.revision==revision) {
                        let pending=view.pending_render.take().ok_or("missing render proposal")?;
                        if accepted {view.revision=revision;view.rendered=pending.text;view.render_failed=false;
                            view.display_window=pending.window;
                            effects.push(reference_controls(&pane_id,view));
                            if view.jump_pending {view.anchor=None;effects.push(Effect::ScrollLatest{pane_id:pane_id.clone()});}
                            else if let Some(row)=view.display_window.restore_row {
                                let row=row.min(view.display_window.anchors.len().saturating_sub(view.visible_rows));
                                view.scroll_pending=Some(row);effects.push(Effect::ScrollToRow{pane_id:pane_id.clone(),row});
                            }}
                        else {view.render_failed=true;view.dirty=true;
                            effects.push(Effect::Status{pane_id,state:"display_failed".into(),message:"Display update failed; the previous content is preserved. Retry the display.".into()});}
                    }
                }
            },
            Input::RetryDisplay{pane_id}=>{if let Some(view)=self.views.get_mut(&pane_id){view.render_failed=false;view.dirty=true;}},
            Input::Viewport{pane_id,at_bottom,revision,visible,top_row,visible_rows}=>{
                let mut load_older=false;let mut gap=None;
                if let Some(view)=self.views.get_mut(&pane_id){
                    view.visible=visible;
                    if let Some(rows)=visible_rows {view.visible_rows=rows.max(1);}
                    if revision==view.revision && view.pending_render.is_none() {
                        let scroll_settled=view.scroll_pending.is_none_or(|expected|top_row.is_some_and(|row|row.abs_diff(expected)<=1));
                        if scroll_settled {
                            view.scroll_pending=None;
                            let logical_bottom=at_bottom&&!view.display_window.more_after&&view.display_window.gap_after.is_none();
                            view.at_bottom=logical_bottom;
                            if let Some(top)=top_row {
                                if !logical_bottom&&!view.jump_pending {
                                    view.anchor=view.display_window.anchors.get(top).cloned();view.tail=false;
                                    let near_start=top<8;
                                    let near_end=top.saturating_add(view.visible_rows+8)>=view.display_window.anchors.len();
                                    if !view.blocked&&((near_start&&view.display_window.more_before)||(near_end&&view.display_window.more_after)) {view.dirty=true;}
                                    if !view.blocked {
                                        if near_start {gap=view.display_window.gap_before.clone();}
                                        if gap.is_none()&&near_end {gap=view.display_window.gap_after.clone();}
                                    }
                                    load_older=!view.blocked&&near_start&&!view.display_window.more_before&&gap.is_none();
                                }else if logical_bottom {view.anchor=None;view.tail=true;}
                            }
                            if let Some(top)=top_row {if top==0 && view.display_window.anchors.len()<=view.visible_rows && !view.blocked {gap=view.display_window.gap_before.clone();}}
                            if logical_bottom&&view.tail&&(!view.new_messages.is_empty()||view.unknown_new)&&!view.jump_pending {
                                view.dirty=true;view.jump_pending=true;
                            }
                            if logical_bottom&&view.tail&&!view.dirty {view.reading.applied_latest();}
                        }
                    }
                }
                if let Some(key)=gap {self.request_navigation(&pane_id,key,None,false,true,&mut effects)?;}
                else if load_older {self.load_older(&pane_id,&mut effects)?;}
            },
            Input::JumpToLatest{pane_id}=>{if let Some(view)=self.views.get_mut(&pane_id){
                view.anchor=None;view.scroll_pending=None;view.tail=true;view.jump_pending=true;view.dirty=true;view.render_failed=false;
            }},
            Input::Flush=>{self.render(&mut effects)?;},
            Input::ExportDrafts=>{effects.push(Effect::Drafts{value:self.export_drafts()});},
            Input::RestoreDrafts{drafts}=>{self.restore_drafts(drafts)?;},
            Input::ActivateRow{pane_id,revision,row,column}=>{
                let view=self.views.get_mut(&pane_id).ok_or("unknown Pane")?;
                if view.revision==revision && !view.blocked && view.visible && column>=2 {
                    if let Some(action)=view.display_window.references.iter().find(|action|action.row==row && column<action.column_end).cloned() {
                        if action.kind=="inquiry" {
                            let window=view.window.clone();let scope=view.scope.clone();
                            let key=inquiry::Key{scope,sender:action.target.0,message:action.target.1};
                            self.inquiry_open_reference(window,key,&mut effects)?;
                        } else if let Some(generation)=action.generation {
                            if let Some(state)=view.tools.get_mut(&generation) {
                                state.open = !state.open;view.dirty=true;effects.push(Effect::NeedsRender);
                                if state.open && !action.target.0.is_empty() {
                                    self.request_inline_activity(&pane_id,action.target,&mut effects)?;
                                }
                            }
                        } else {self.locate(&pane_id,action.target,&mut effects)?;}
                    }
                }
            },
            Input::References{pane_id}=>self.reference_menu(&pane_id,&mut effects)?,
            Input::CopySource{pane_id,sender_id,message_id}=>{
                let view=self.views.get(&pane_id).ok_or("unknown Pane")?;
                let row=self.threads.get(&view.scope).and_then(|state|state.transcript.messages.get(&(sender_id.to_ascii_lowercase(),message_id))).ok_or("Message is no longer cached")?;
                effects.push(Effect::CopySource{pane_id,text:crate::text::body(row)});
            },
            Input::Locate{pane_id,sender_id,message_id}=>self.locate(&pane_id,(sender_id,message_id),&mut effects)?,
            Input::Followups{pane_id,sender_id,message_id,after}=>self.request_navigation(&pane_id,(sender_id,message_id),after,true,false,&mut effects)?,
            Input::ReturnToReading{pane_id}=>{
                if let Some(view)=self.views.get_mut(&pane_id){if view.reading.return_to_reading(){view.scroll_pending=None;view.dirty=true;}}
            },
            Input::OpenReferenceThread{pane_id,thread_id}=>{
                validate_id(&thread_id)?;
                let view=self.views.get(&pane_id).ok_or("unknown Pane")?;
                let mut scope=view.scope.clone();scope.thread=thread_id;
                effects.push(Effect::OpenReferenceThread{window_id:view.window.clone(),scope});
            },
            Input::GenerationActivity{pane_id,generation_id}=>{
                let view=self.views.get(&pane_id).ok_or("unknown Pane")?;
                let generation=self.threads.get(&view.scope).and_then(|state|state.transcript.generations.get(&generation_id).or_else(||state.transcript.activities.get(&generation_id))).ok_or("Generation activity is no longer cached")?;
                let entries=generation["tools"].as_array().into_iter().flatten().map(|tool| {
                    let state=serde_json::from_value::<polar_inhouse_model::ReplyToolState>(tool["state"].clone()).map(polar_lgp_presentation::tool_state_label).unwrap_or("Unknown");
                    json!({"title":format!("{} · {state}",crate::text::safe(tool["tool_name"].as_str().unwrap_or("Tool")))})
                }).collect::<Vec<_>>();
                effects.push(Effect::Activity{pane_id,items:json!({"title":"Turn tool activity","entries":entries})});
            },
            Input::Activity{pane_id,sender_id,message_id,after_sequence}=>{
                let view=self.views.get(&pane_id).ok_or("unknown Pane")?;
                let scope=view.scope.clone();let t=self.threads.get(&scope).ok_or("unknown Thread")?;
                if let (Some(sender),Some(message))=(sender_id,message_id){
                    if !t.transcript.messages.contains_key(&(sender.clone(),message.clone())){return Err("message is not in this conversation view".into());}
                    let lease=t.lease.clone().ok_or("conversation is not connected")?;
                    let selection=(pane_id.clone(),sender.clone(),message.clone());
                    if after_sequence.is_none()||self.activity_selection.as_ref()!=Some(&selection){self.activity_rows.clear();}
                    self.activity_selection=Some(selection);
                    self.request(&scope.target_id,json!({"op":"activity","expected_node_id":scope.node_id,"lease_id":lease,
                        "key":{"sender_id":sender,"message_id":message},"after_sequence":after_sequence}),
                        Purpose::Activity{scope:scope.clone(),pane:pane_id,sender,message,inline:false,after:after_sequence},&mut effects)?;
                }else{
                    self.activity_selection=None;self.activity_rows.clear();
                    let mut rows=t.transcript.messages.values().collect::<Vec<_>>();
                    rows.sort_by_key(|row|std::cmp::Reverse(row["created_at"].as_u64().unwrap_or(0)));
                    let entries=rows.into_iter().take(100).map(|row|json!({
                        "title":format!("{} · {}",row["presentation"]["author_label"].as_str().unwrap_or("Message"),crate::text::body(row).chars().take(100).collect::<String>()),
                        "input":{"type":"activity","pane_id":pane_id,"sender_id":row["sender_node_id_hex"],"message_id":row["message_id"]}
                    })).collect::<Vec<_>>();
                    let mut entries=entries;
                    entries.extend(t.transcript.generations.iter().chain(t.transcript.activities.iter()).filter(|(_,generation)|generation["tools"].as_array().is_some_and(|rows|!rows.is_empty())).map(|(id,generation)|json!({
                        "title":format!("{} · Turn tool activity ({})",crate::text::safe(generation["actor_name"].as_str().unwrap_or("Agent")),generation["tools"].as_array().map_or(0,Vec::len)),
                        "input":{"type":"generation_activity","pane_id":pane_id,"generation_id":id}
                    })));
                    effects.push(Effect::Activity{pane_id,items:json!({"title":"Tool activity — choose a message or turn","entries":entries})});
                }
            },
            Input::Search{pane_id,query}=>{
                if query.len()>256{return Err("search query too long".into());}
                let view=self.views.get(&pane_id).ok_or("unknown Pane")?;let t=self.threads.get(&view.scope).ok_or("unknown Thread")?;
                let rows=t.transcript.messages.iter().filter(|(_,v)|crate::text::body(v).contains(&query)).take(100)
                    .map(|((sender,message),v)|json!({"sender_id":sender,"message_id":message,"text":crate::text::safe(&crate::text::body(v)).chars().take(240).collect::<String>()})).collect::<Vec<_>>();
                effects.push(Effect::SearchResults{pane_id,items:json!({"scope":"loaded_messages","items":rows})});
            },
        }
        for thread in self.threads.values_mut(){thread.transcript.shared.set_presentation_limit(self.cache_policy.presentation_bytes());}
        if self.threads.values().map(|thread|thread.transcript.retained_bytes()).sum::<usize>()>self.cache_policy.message_bytes() {
            self.card_cache.borrow_mut().clear();
            let active=self.views.values().map(|view|view.scope.clone()).collect::<BTreeSet<_>>();
            let protected=self.protected_ranges();
            let within_budget=polar_lgp_reader::trim_ranges(self.threads.iter_mut().map(|(scope,thread)|
                (active.contains(scope),protected.get(scope).cloned().unwrap_or_default(),&mut thread.transcript.shared)).collect(),self.cache_policy.message_bytes());
            if !within_budget {effects.push(Effect::Error{message:"Message cache is full; currently displayed content is protected".into()});}
        }
        self.navigation.retain(|id,_|self.pages.contains_key(id));
        self.prune_catalogs(&mut effects)?;
        self.flush_catalog(&mut effects)?;
        self.flush_requests(&mut effects);
        self.composer_effects(&mut effects);
        self.inquiry_effects(&mut effects);
        self.reading_hints(&mut effects);
        if self.views.values().any(|v|v.dirty&&v.visible&&!v.blocked&&!v.render_failed&&v.pending_render.is_none())&&!matches!(effects.last(),Some(Effect::NeedsRender)){effects.push(Effect::NeedsRender);}
        effects.sort_by_key(|effect| match effect{Effect::Composer{..}=>0,Effect::FocusComposer{..}=>2,_=>1});
        Ok(effects)
    }
    fn load_older(&mut self,pane_id:&str,out:&mut Vec<Effect>)->Result<(),String>{
        let scope=self.views.get(pane_id).ok_or("unknown Pane")?.scope.clone();
        if self.pages.values().any(|page|page.scope==scope){return Ok(());}
        let t=self.threads.get(&scope).ok_or("unknown Thread")?;
        if !t.has_more||!self.ready{return Ok(());}
        let Some(lease)=t.lease.clone()else{return Ok(());};
        let Some(before)=t.transcript.oldest()else{return Ok(());};
        let subscription=self.next_id("page")?;
        self.pages.insert(subscription.clone(),ReadPage{scope:scope.clone(),assembly:Assembler::default(),sequence:0,keys:vec![(before["sender_id"].as_str().unwrap_or("").into(),before["message_id"].as_str().unwrap_or("").into())]});
        self.request(&scope.target_id,json!({"op":"subscribe","expected_node_id":scope.node_id,"lease_id":lease,"subscription_id":subscription,"before":before}),
            Purpose::Page{scope:scope.clone(),subscription},out)?;Ok(())
    }
    fn close_pane(&mut self,pane_id:&str,out:&mut Vec<Effect>)->Result<(),String>{
                if self.ideas.remove(pane_id).is_some(){
                    let windows=self.selected.iter().filter(|(_,p)|p.as_str()==pane_id).map(|(w,_)|w.clone()).collect::<Vec<_>>();
                    self.selected.retain(|_,p|p.as_str()!=pane_id);
                    for window in windows{self.composer_cache.remove(&window);out.push(Effect::ComposerClosed{window_id:window});}
                }
                if let Some(view)=self.views.remove(pane_id){
                    let windows=self.selected.iter().filter(|(_,p)|p.as_str()==pane_id).map(|(w,_)|w.clone()).collect::<Vec<_>>();
                    self.selected.retain(|_,p|p.as_str()!=pane_id);
                    for window in windows{self.composer_cache.remove(&window);out.push(Effect::ComposerClosed{window_id:window});}
                    if !self.views.values().any(|v|v.scope==view.scope){
                        self.pages.retain(|_,page|page.scope!=view.scope);
                        let lease=self.threads.get_mut(&view.scope).and_then(|t| {
                            t.subscription=None;t.sequence=0;t.boot=None;t.record.clear();t.staging=None;t.status="offline".into();
                            t.transcript.shared.last_used=self.now_ms;
                            t.lease.take()
                        });
                        if self.ready {if let Some(lease)=lease {
                            self.request(&view.scope.target_id,json!({"op":"release","expected_node_id":view.scope.node_id,"lease_id":lease}),Purpose::ReleaseScope(view.scope.clone()),out)?;
                        }}
                    }
                }
        Ok(())
    }
    fn current_catalog_context(&self,target:&str,context:&BrowseContext)->bool {
        context.window_id.is_empty() || self.catalog_views.get(&context.window_id).is_some_and(|v|v.target==target&&v.context==*context)
    }
    fn prune_catalogs(&mut self,out:&mut Vec<Effect>)->Result<(),String>{
        // Fence obsolete reads before their responses can recreate a picker.
        let stale=self.pending.iter().filter_map(|(id,p)|match &p.purpose {
            Purpose::Describe{target,context}|Purpose::Channels{target,context,..}|Purpose::Threads{target,context,..}
                if !self.current_catalog_context(target,context)=>Some(id.clone()),
            _=>None,
        }).collect::<Vec<_>>();
        for id in stale {
            if self.pending.remove(&id).is_some_and(|p|p.sent) {
                let request=LocalExtensionCancelV1{extension:POLAR_ACCESS_EXTENSION.into(),version:1,request_id:id};
                out.push(Effect::Request{frame:serde_json::to_value(LocalClientFrame::ExtensionCancel{request}).map_err(|e|e.to_string())?});
            }
        }
        self.queued.retain(|(id,_)|self.pending.contains_key(id));
        let dirty=std::mem::take(&mut self.catalog_dirty);
        self.catalog_dirty=dirty.into_iter().filter(|p|match p {
            Purpose::Channels{target,context,..}|Purpose::Threads{target,context,..}=>self.current_catalog_context(target,context),
            _=>false,
        }).collect();
        let unused=self.catalogs.iter().filter(|((target,node),feed)|
            !self.catalog_views.values().any(|v|v.target==*target&&(v.node.is_empty()||v.node==*node)) &&
            !self.views.values().any(|v|v.scope.target_id==*target&&v.scope.node_id==*node) &&
            // Wait for subscribe acknowledgement before unsubscribe so an
            // unordered Host executor cannot create the feed after cleanup.
            !self.pending.values().any(|p|matches!(&p.purpose,Purpose::Catalog{subscription,..} if subscription==&feed.subscription))
        ).map(|(key,_)|key.clone()).collect::<Vec<_>>();
        for key in unused {if let Some(feed)=self.catalogs.remove(&key){if self.ready{
            self.request(&key.0,json!({"op":"unsubscribe","expected_node_id":key.1,"subscription_id":feed.subscription}),Purpose::Release,out)?;
        }}}
        Ok(())
    }
    fn next_id(&mut self,prefix:&str)->Result<String,String>{self.counter=self.counter.checked_add(1).ok_or("request ID exhausted")?;Ok(format!("{prefix}-{}-{}",self.id,self.counter))}
    fn request(&mut self,target:&str,mut payload:Value,purpose:Purpose,_out:&mut Vec<Effect>)->Result<String,String>{
        if !self.ready{return Err("Host has not negotiated Polar access".into());}
        if self.pending.len()>=MAX_QUEUED_REQUESTS{return Err("too many queued requests".into());}
        payload["target_id"]=json!(target);let id=self.next_id("lgp")?;
        let request=LocalExtensionRequestV1{extension:POLAR_ACCESS_EXTENSION.into(),version:1,request_id:id.clone(),timeout_ms:30_000,payload};
        request.validate().map_err(|e|e.to_string())?;
        let frame=serde_json::to_value(LocalClientFrame::ExtensionRequest{request}).map_err(|e|e.to_string())?;
        self.pending.insert(id.clone(),Pending{purpose,deadline:self.now_ms.saturating_add(120_000),target:target.into(),sent:false});
        self.queued.push_back((id.clone(),frame));Ok(id)
    }
    fn flush_requests(&mut self,out:&mut Vec<Effect>){
        if !self.ready&&self.idea_codex.host.is_none(){return;}
        let mut active=0;
        let mut counts=BTreeMap::<String,usize>::new();
        for pending in self.pending.values().filter(|p|p.sent){active+=1;*counts.entry(pending.target.clone()).or_default()+=1;}
        // Scan each queued entry once. A saturated target is rotated, not a
        // head-of-line barrier for another target. Never synthesize retries.
        for _ in 0..self.queued.len(){
            let Some((id,frame))=self.queued.pop_front()else{break;};
            let Some(p)=self.pending.get_mut(&id)else{continue;};
            if active>=MAX_ACTIVE_REQUESTS||counts.get(&p.target).copied().unwrap_or(0)>=MAX_ACTIVE_PER_TARGET{
                self.queued.push_back((id,frame));continue;
            }
            p.sent=true;p.deadline=self.now_ms.saturating_add(31_000);
            active+=1;*counts.entry(p.target.clone()).or_default()+=1;
            out.push(Effect::Request{frame});
        }
    }
    fn catalog_query(&mut self,p:Purpose,out:&mut Vec<Effect>)->Result<(),String>{
        if self.pending.values().any(|q|q.purpose==p){
            if !self.catalog_dirty.contains(&p){self.catalog_dirty.push(p);}
            return Ok(());
        }
        let (target,payload)=match &p {
            Purpose::Channels{target,node,..}=>(target.clone(),json!({"op":"channels","expected_node_id":node})),
            Purpose::Threads{target,node,channel,..}=>(target.clone(),json!({"op":"threads","expected_node_id":node,"channel_id":channel})),
            _=>return Err("not a catalog query".into()),
        };
        self.request(&target,payload,p,out)?;Ok(())
    }
    fn flush_catalog(&mut self,out:&mut Vec<Effect>)->Result<(),String>{
        let dirty=std::mem::take(&mut self.catalog_dirty);
        for p in dirty {self.catalog_query(p,out)?;}Ok(())
    }
    fn subscribe_catalog(&mut self,target:&str,node:&str,out:&mut Vec<Effect>)->Result<(),String>{
        let key=(target.to_owned(),node.to_owned());
        if self.catalogs.contains_key(&key){return Ok(());}
        let subscription=self.next_id("catalog")?;
        self.catalogs.insert(key,CatalogFeed{subscription:subscription.clone(),sequence:0,boot:None});
        self.request(target,json!({"op":"subscribe_catalog","expected_node_id":node,"subscription_id":subscription}),Purpose::Catalog{target:target.into(),node:node.into(),subscription},out)?;Ok(())
    }
    fn invalidate_catalog(&mut self,target:&str,node:&str,channel:Option<&str>,out:&mut Vec<Effect>)->Result<(),String>{
        for view in self.catalog_views.values().filter(|v|v.target==target&&v.node==node).cloned().collect::<Vec<_>>() {
            self.catalog_query(Purpose::Channels{target:target.into(),node:node.into(),context:view.context.clone()},out)?;
            if let Some(selected)=view.channel.filter(|c|channel.is_none_or(|changed|changed==c)) {
                self.catalog_query(Purpose::Threads{target:target.into(),node:node.into(),channel:selected,context:view.context},out)?;
            }
        }
        // Restore/rename labels even when no picker is currently browsing them.
        let channels=self.views.values().map(|view|&view.scope).filter(|s|s.target_id==target&&s.node_id==node)
            .filter_map(|s|self.thread_titles.get(&(s.target_id.clone(),s.node_id.clone(),s.thread.clone())))
            .map(|(c,_)|c.clone()).filter(|c|!c.is_empty()&&channel.is_none_or(|changed|changed==c))
            .collect::<std::collections::BTreeSet<_>>();
        if !channels.is_empty() {self.catalog_query(Purpose::Channels{target:target.into(),node:node.into(),context:BrowseContext::default()},out)?;}
        for channel in channels {self.catalog_query(Purpose::Threads{target:target.into(),node:node.into(),channel,context:BrowseContext::default()},out)?;}
        Ok(())
    }
    fn bind(&mut self,scope:&Scope,out:&mut Vec<Effect>)->Result<(),String>{
        if !self.views.values().any(|view|&view.scope==scope){return Ok(());}
        if self.pending.values().any(|p|matches!(&p.purpose,Purpose::Bind(s)|Purpose::ReleaseScope(s) if s==scope)){return Ok(());}
        let previous=self.threads.get(scope).map(|t|(t.lease.clone(),t.subscription.clone())).unwrap_or_default();
        if let Some(t)=self.threads.get_mut(scope){t.subscription=None;t.sequence=0;t.boot=None;t.record.clear();t.staging=None;t.repair_after=t.transcript.resume_message.clone();t.verified_views.clear();t.transcript.shared.invalidate_coverage();t.status="binding".into();t.connection_message.clear();t.lease=None;}
        self.pages.retain(|_,page|&page.scope!=scope);
        // Wait for release before rebinding. Sending both concurrently could
        // return the idempotent old lease and immediately revoke it again.
        if let Some(lease)=previous.0 {
            self.request(&scope.target_id,json!({"op":"release","expected_node_id":scope.node_id,"lease_id":lease}),Purpose::ReleaseScope(scope.clone()),out)?;
            return Ok(());
        }
        if let Some(id)=previous.1 {
            self.request(&scope.target_id,json!({"op":"unsubscribe","expected_node_id":scope.node_id,"subscription_id":id}),Purpose::ReleaseScope(scope.clone()),out)?;
            return Ok(());
        }
        self.request(&scope.target_id,json!({"op":"bind","expected_node_id":scope.node_id,"thread_id":scope.thread,"access":"read_write"}),Purpose::Bind(scope.clone()),out)?;
        self.status(scope,"binding","Binding the selected Polar Thread",out);Ok(())
    }
    fn subscribe(&mut self,scope:&Scope,out:&mut Vec<Effect>)->Result<(),String>{
        let lease=self.threads.get(scope).and_then(|t|t.lease.clone()).ok_or("missing Thread lease")?;
        if let Some(old)=self.threads.get(scope).and_then(|t|t.subscription.clone()){
            self.request(&scope.target_id,json!({"op":"unsubscribe","expected_node_id":scope.node_id,"subscription_id":old}),Purpose::Release,out)?;
        }
        let id=self.next_id("feed")?;
        if let Some(t)=self.threads.get_mut(scope){t.subscription=Some(id.clone());t.sequence=0;t.boot=None;t.record.clear();}
        let mut payload=json!({"op":"subscribe","expected_node_id":scope.node_id,"lease_id":lease,"subscription_id":id});
        if let Some(t)=self.threads.get(scope).filter(|t|t.supports_resume) {
            if let Some((sender,message))=&t.transcript.resume_message {
                payload["resume_message"]=json!({"sender_id":sender,"message_id":message});
            }
            let known=t.transcript.shared.message_window(None).0.into_iter().rev().take(100)
                .filter_map(|row|polar_lgp_reader::message_key(&row).ok()).map(|(sender,message)|json!({"sender_id":sender,"message_id":message})).collect::<Vec<_>>();
            payload["known_messages"]=json!(known);
            let anchors=self.views.values().filter(|v|&v.scope==scope).filter_map(|v|v.anchor.as_ref())
                .take(8).map(|a|json!({"sender_id":a.key.0,"message_id":a.key.1})).collect::<Vec<_>>();
            payload["verify_anchors"]=json!(anchors);
        }
        self.request(&scope.target_id,payload,Purpose::Subscribe{scope:scope.clone(),subscription:id},out)?;Ok(())
    }
    fn frame(&mut self,frame:Value,out:&mut Vec<Effect>)->Result<(),String>{
        let frame:LocalHostFrame=serde_json::from_value(frame).map_err(|_|"invalid Host frame")?;
        match frame {
            LocalHostFrame::ApplicationResponse{response:polar_tmux_protocol::HostApplicationMessage::CodexV1(response)}=>{
                let owner_matches=self.pending.get(&response.request_id).is_some_and(|p|matches!(&p.purpose,Purpose::IdeaCodex(op) if self.idea_operation_owner(op)==Some(response.owner_host_id.as_str())));
                if !owner_matches{return Ok(());}
                if let Some(p)=self.pending.remove(&response.request_id) {
                    if let Purpose::IdeaCodex(op)=p.purpose {self.idea_codex_result(op,response.result,out)?;}
                }
            },
            LocalHostFrame::ApplicationStateChanged{api:polar_tmux_protocol::ApplicationStateApiV1::CodexV1,..}=>{},
            LocalHostFrame::ExtensionResponse{response}=>{
                if response.extension!=POLAR_ACCESS_EXTENSION||response.version!=1{return Ok(());}
                if self.pending.get(&response.request_id).is_some_and(|p|!p.sent){return Err("response for an undispatched request".into());}
                let Some(p)=self.pending.remove(&response.request_id) else{return Ok(());};
                match response.result {
                    LocalExtensionResultV1::Success{payload}=>self.success(p.purpose,payload,out)?,
                    LocalExtensionResultV1::Rejected{error}=>{
                        if matches!(error.code.as_str(),"denied"|"thread_not_found") {
                            if let Purpose::Bind(scope)|Purpose::Renew{scope,..}|Purpose::Subscribe{scope,..}=&p.purpose {
                                if let Some(thread)=self.threads.get_mut(scope){thread.transcript.shared=Default::default();thread.transcript.cards.borrow_mut().invalidate(&thread.transcript.cache_namespace);}
                                for view in self.views.values_mut().filter(|view|&view.scope==scope){view.dirty=true;view.anchor=None;view.jump_pending=true;}
                            }
                        }
                        self.failed(p.purpose,&error.message,out)
                    },
                    _=>self.failed(p.purpose,"Polar access was refused or cancelled",out),
                }
            },
            LocalHostFrame::ExtensionEvent{event}=>{
                if event.extension!=POLAR_ACCESS_EXTENSION||event.version!=1{return Ok(());}
                if let Some((target,node))=self.catalogs.iter().find(|(_,f)|f.subscription==event.subscription_id).map(|(key,_)|key.clone()) {
                    if event.payload["target_id"].as_str().is_some_and(|t|t!=target)||event.payload["node_id"].as_str().is_some_and(|n|n!=node){return Err("catalog target scope mismatch".into());}
                    if event.payload["kind"]=="reset_required" {
                        self.catalogs.remove(&(target.clone(),node.clone()));
                        if event.payload["reason"]=="target_disconnected" {self.target_disconnected(&target,out)?;}
                        for view in self.catalog_views.values().filter(|v|v.target==target) {out.push(Effect::BrowseError{context:view.context.clone(),message:format!("Catalog disconnected: {}. Reopen to retry.",event.payload["reason"].as_str().unwrap_or("unknown"))});}
                        return Ok(());
                    }
                    let feed=self.catalogs.get_mut(&(target.clone(),node.clone())).ok_or("closed catalog")?;
                    let gap=Some(event.sequence)!=feed.sequence.checked_add(1)||feed.boot.as_deref().is_some_and(|b|event.payload["boot_epoch"].as_str()!=Some(b));
                    feed.sequence=event.sequence;feed.boot=event.payload["boot_epoch"].as_str().map(str::to_owned);
                    if gap||event.payload["kind"]=="catalog_changed" {self.invalidate_catalog(&target,&node,if gap{None}else{event.payload["channel_id"].as_str()},out)?;}
                    return Ok(());
                }
                if let Some(page)=self.pages.get(&event.subscription_id){
                    let scope=&page.scope;
                    if event.payload["target_id"].as_str().is_some_and(|target|target!=scope.target_id.as_str()){return Err("history target scope mismatch".into());}
                    if event.payload["kind"]=="reset_required" && event.payload["reason"]=="target_disconnected" {
                        let target=scope.target_id.clone();self.target_disconnected(&target,out)?;return Ok(());
                    }
                }
                if let Some(ReadPage{scope,mut assembly,sequence,mut keys})=self.pages.remove(&event.subscription_id){
                    if event.payload["kind"]=="reset_required"{self.status(&scope,"page_failed","Target connection ended; retry the history page",out);return Ok(());}
                    if Some(event.sequence)!=sequence.checked_add(1){return Err("history page event gap".into());}
                    let navigation=self.navigation.get(&event.subscription_id).cloned();
                    let done=event.payload["kind"]=="snapshot_end"||event.payload["kind"]=="navigation_end"||event.payload["kind"]=="reset_required";
                    if event.payload["kind"]=="reset_required" {self.status(&scope,"read_failed","Reference or history could not be loaded",out);}

                    if event.payload["kind"].as_str().is_some_and(|s|s.starts_with("record_")){
                        if let Some(value)=assembly.accept(&event.payload)?{if value["kind"]=="message"{
                            let other_bytes=self.threads.iter().filter(|(key,_)|*key!=&scope).map(|(_,thread)|thread.transcript.retained_bytes()+thread.staging.as_ref().map_or(0,|stage|stage.retained_bytes())).sum::<usize>();
                            let thread=self.threads.get_mut(&scope).ok_or("closed page")?;
                            thread.transcript.shared.set_admission_limit(self.cache_policy.message_bytes().saturating_sub(other_bytes+thread.staging.as_ref().map_or(0,|stage|stage.retained_bytes())));
                            thread.transcript.put_message(&scope.thread,value["value"].clone())?;keys.push(polar_lgp_reader::message_key(&value["value"])?);self.dirty(&scope);
                        }else if value["kind"]=="navigation" {if let Some(read)=navigation.as_ref(){self.navigation_result(read,&value["value"],out)?;}}}
                    }
                    if done && navigation.is_none() && event.payload["kind"]=="snapshot_end" {
                        if let Some(t)=self.threads.get_mut(&scope){t.transcript.shared.mark_keys(&keys);t.has_more=event.payload["has_more"].as_bool().unwrap_or(false);}
                        for view in self.views.values_mut().filter(|v|v.scope==scope){view.dirty=true;}
                    }
                    if done {
                        self.navigation.remove(&event.subscription_id);
                        if navigation.as_ref().is_some_and(|read|read.repair) {self.request_repair(&scope,out)?;}
                        self.request_visible_verification(&scope,out)?;
                    } else if navigation.is_none() || self.navigation.contains_key(&event.subscription_id) {
                        self.pages.insert(event.subscription_id,ReadPage{scope,assembly,sequence:event.sequence,keys});
                    }
                    return Ok(());
                }
                let scope=self.threads.iter().find(|(_,t)|t.subscription.as_deref()==Some(&event.subscription_id)).map(|(s,_)|s.clone());
                let Some(scope)=scope else{return Ok(());};
                if event.payload["target_id"].as_str().is_some_and(|target|target!=scope.target_id.as_str()){return Err("event target scope mismatch".into());}
                if event.payload["kind"]=="reset_required" && event.payload["reason"]=="target_disconnected" {
                    self.target_disconnected(&scope.target_id,out)?;
                    return Ok(());
                }
                let mut t=self.threads.remove(&scope).ok_or("closed Thread")?;
                let other_bytes=self.threads.values().map(|thread|thread.transcript.retained_bytes()+thread.staging.as_ref().map_or(0,|stage|stage.retained_bytes())).sum::<usize>();
                let allowance=self.cache_policy.message_bytes().saturating_sub(other_bytes);
                let staging_bytes=t.staging.as_ref().map_or(0,|stage|stage.retained_bytes());
                t.transcript.shared.set_admission_limit(if event.payload["kind"]=="snapshot_end" {allowance}else{allowance.saturating_sub(staging_bytes)});
                let committed_bytes=t.transcript.retained_bytes();
                if let Some(stage)=t.staging.as_mut(){stage.shared.set_admission_limit(allowance.saturating_sub(committed_bytes));}
                let snapshot_new=if event.payload["kind"]=="snapshot_end" {
                    t.staging.as_ref().map(|stage|stage.messages.keys().filter(|key|!t.transcript.messages.contains_key(*key)).cloned().collect::<Vec<_>>()).unwrap_or_default()
                }else{Vec::new()};
                let result=apply_event(&scope,&mut t,event.sequence,&event.payload);
                let budget_error=result.as_ref().is_err_and(|error|error=="message cache budget exhausted");
                let reset=matches!(result,Ok(true))||result.is_err();
                if budget_error {
                    self.card_cache.borrow_mut().clear();
                    let active=self.views.values().map(|view|view.scope.clone()).collect::<BTreeSet<_>>();
                    let needed=t.transcript.admission_shortfall.max(t.staging.as_ref().map_or(0,|stage|stage.admission_shortfall));
                    let protected=self.protected_ranges();
                    let total=other_bytes.saturating_add(t.transcript.retained_bytes());
                    let mut ranges=self.threads.iter_mut().map(|(key,thread)|(active.contains(key),protected.get(key).cloned().unwrap_or_default(),&mut thread.transcript.shared)).collect::<Vec<_>>();
                    ranges.push((active.contains(&scope),protected.get(&scope).cloned().unwrap_or_default(),&mut t.transcript.shared));
                    polar_lgp_reader::trim_ranges(ranges,total.saturating_sub(needed));
                }
                if event.payload["kind"]=="snapshot_end" && !reset {
                    for view in self.views.values_mut().filter(|v|v.scope==scope&&!v.at_bottom&&!v.rendered.is_empty()){
                        for key in &snapshot_new{view.reading.arrived(key.clone());}
                    }
                }
                if event.payload["kind"]=="reset_required"&&matches!(event.payload["reason"].as_str(),Some("denied"|"thread_not_found")) {
                    t.transcript.shared=Default::default();t.transcript.cards.borrow_mut().invalidate(&t.transcript.cache_namespace);
                    for view in self.views.values_mut().filter(|view|view.scope==scope){view.dirty=true;view.anchor=None;view.jump_pending=true;}
                }
                self.threads.insert(scope.clone(),t);
                if event.payload["kind"]=="reset_required"&&matches!(event.payload["reason"].as_str(),Some("denied"|"thread_not_found")){self.inquiry_revoke(&scope);}
                if !reset && matches!(event.payload["kind"].as_str(),Some("record_end"|"snapshot_end")) {self.inquiry_observe(&scope,event.payload["kind"]!="snapshot_end");}
                if reset {
                    self.status(&scope,"resync_required","Reload the canonical snapshot",out);
                    // Retain the last complete transcript; never treat cursor gaps as data.
                    let mut retry=false;
                    if let Some(t)=self.threads.get_mut(&scope){
                        t.staging=None;t.record.clear();t.resync_attempts=t.resync_attempts.saturating_add(1);
                        retry=t.resync_attempts<=2 && !matches!(event.payload["reason"].as_str(),Some("record_limit"|"lease_expired"|"denied"));
                        if !retry{t.status="unavailable".into();}
                    }
                    if retry{self.subscribe(&scope,out)?;}else{
                        self.status(&scope,"unavailable","The stream cannot be recovered automatically; inspect the error and explicitly retry",out);
                    }
                } else if matches!(event.payload["kind"].as_str(),Some("record_end"|"snapshot_end")){
                    self.dirty(&scope);
                    if event.payload["kind"]=="snapshot_end" {self.request_repair(&scope,out)?;self.request_visible_verification(&scope,out)?;}
                }
            },_=>{},
        }Ok(())
    }
    fn success(&mut self,p:Purpose,v:Value,out:&mut Vec<Effect>)->Result<(),String>{match p{
        Purpose::IdeaCodex(_)=>return Err("Codex response arrived on the wrong protocol".into()),
        Purpose::Inquiry{window,key,operation,view_token}=>{if self.inquiry_view_current(&key,view_token.as_deref()){self.inquiry_response_effects(&window,key,operation,v,out)}},
        Purpose::ListTargets=>{
            let rows=v["targets"].as_array().ok_or("invalid target list")?;
            if rows.len()>33{return Err("target list exceeds the configured bound".into());}
            let mut seen=std::collections::BTreeSet::new();
            for row in rows {
                let id=row["target_id"].as_str().ok_or("target list ID missing")?;validate_id(id)?;
                if !seen.insert(id){return Err("duplicate target list ID".into());}
            }
            // Configured expected_node_id is not a verified node identity. Only
            // Browse/Describe followed by Bind may establish conversation scope.
            for row in rows {if let Some(id)=row["target_id"].as_str(){self.target_titles.insert(id.into(),row["display_name"].as_str().unwrap_or(id).into());}}
            out.push(Effect::Targets{items:v["targets"].clone()});
        },
        Purpose::Describe{target,context}=>{
            if !self.current_catalog_context(&target,&context){return Ok(());}
            let node=v["node_id"].as_str().ok_or("target has no Node identity")?.to_owned();
            out.push(Effect::Targets{items:json!([{"target_id":target,"node_id":node,"owner":v["owner_node_id"],"capabilities":v["capabilities"]}])});
            if let Some(view)=self.catalog_views.get_mut(&context.window_id) {if view.context==context&&view.target==target{view.node=node.clone();}}
            else {self.catalog_views.insert(context.window_id.clone(),CatalogView{target:target.clone(),node:node.clone(),channel:None,context:context.clone()});}
            if v["capabilities"]["catalog_events"]==true{self.subscribe_catalog(&target,&node,out)?;}
            self.catalog_query(Purpose::Channels{target,node,context},out)?;
        },
        Purpose::Channels{target,node,context}=>{
            if !self.current_catalog_context(&target,&context){return Ok(());}
            if v["node_id"].as_str()!=Some(&node){return Err("channel target identity mismatch".into());}
            let items=catalog_projection(&v["channels"],"title","Untitled channel");
            if let Some(rows)=items.as_array(){for row in rows{if let Some(id)=row["channel_id"].as_str(){self.channel_titles.insert((target.clone(),node.clone(),id.into()),row["display_name"].as_str().unwrap_or(id).into());}}}
            out.push(Effect::Channels{target_id:target,node_id:node,context,items});
        },
        Purpose::Threads{target,node,channel,context}=>{
            if !self.current_catalog_context(&target,&context){return Ok(());}
            if v["node_id"].as_str()!=Some(&node){return Err("thread target identity mismatch".into());}
            let items=catalog_projection(&v["threads"],"tag","Untitled conversation");
            if let Some(rows)=items.as_array(){for row in rows{if let Some(id)=row["thread_id"].as_str(){self.thread_titles.insert((target.clone(),node.clone(),id.into()),(channel.clone(),row["display_name"].as_str().unwrap_or(id).into()));}}}
            out.push(Effect::Threads{target_id:target,node_id:node,channel_id:channel,context,items});
        },
        Purpose::Activity{scope,pane,sender,message,inline,after}=>{
            if inline {return self.inline_activity_response(scope,pane,(sender,message),after,v,out);}
            if self.views.get(&pane).is_none_or(|view|view.scope!=scope){return Ok(());}
            if v["node_id"].as_str()!=Some(&scope.node_id){return Err("activity target identity mismatch".into());}
            if self.activity_selection.as_ref()!=Some(&(pane.clone(),sender.clone(),message.clone())){return Ok(());}
            if let Some(rows)=v["page"]["observations"].as_array() {
                self.activity_rows=polar_lgp_reader::merge_activity(&self.activity_rows,rows,self.cache_policy.presentation_bytes())?;
            }
            let mut entries=self.activity_rows.iter().map(|row|json!({"title":format!("{} · {}",row["tool"]["tool_name"].as_str().unwrap_or("Tool"),row["state_label"].as_str().unwrap_or("Unknown"))})).collect::<Vec<_>>();
            if entries.is_empty() {
                if let Some(activity)=self.threads.get(&scope).and_then(|thread|thread.transcript.messages.get(&(sender.clone(),message.clone()))
                    .and_then(|row|thread.transcript.message_activity(row))) {
                    entries.extend(activity["tools"].as_array().into_iter().flatten().map(|tool|json!({"title":format!("{} · {}",
                        crate::text::safe(tool["tool_name"].as_str().unwrap_or("Tool")),
                        serde_json::from_value::<polar_inhouse_model::ReplyToolState>(tool["state"].clone()).map(polar_lgp_presentation::tool_state_label).unwrap_or("Unknown"))})));
                }
            }
            if v["page"]["available"]==false {entries.push(json!({"title":"Recorded tool history is unavailable; retained calls remain visible."}));}
            else if entries.is_empty(){entries.push(json!({"title":"No recorded tool activity is available."}));}
            if let Some(cursor)=v["page"]["next_cursor"].as_i64(){entries.push(json!({"title":"Load more","input":{"type":"activity","pane_id":pane,"sender_id":sender,"message_id":message,"after_sequence":cursor}}));}
            entries.push(json!({"title":"Choose another message","input":{"type":"activity","pane_id":pane}}));
            out.push(Effect::Activity{pane_id:pane,items:json!({"title":"Tool activity","entries":entries})});
        },
        Purpose::Bind(scope)=>{
            if !self.views.values().any(|view|view.scope==scope){
                if let Some(lease)=v["lease_id"].as_str(){self.request(&scope.target_id,json!({"op":"release","expected_node_id":scope.node_id,"lease_id":lease}),Purpose::ReleaseScope(scope.clone()),out)?;}
                return Ok(());
            }
            if v["node_id"].as_str()!=Some(&scope.node_id)||v["owner_node_id"].as_str()!=Some(&scope.owner)||v["thread_id"].as_str()!=Some(&scope.thread){return Err("Thread binding identity mismatch".into());}
            if let Some(t)=self.threads.get_mut(&scope){t.supports_resume=v["supports_message_resume"].as_bool().unwrap_or(false);if t.supports_resume {t.repair_after=None;}t.lease=Some(v["lease_id"].as_str().ok_or("missing lease")?.into());t.renew_at=self.now_ms.saturating_add(v["expires_in_ms"].as_u64().unwrap_or(300_000).min(300_000)*3/4);t.status="loading".into();}
            if let Some(channel)=v["thread"]["channel_id"].as_str(){
                let items=catalog_projection(&json!([v["thread"]]),"tag","Untitled conversation");
                self.thread_titles.insert((scope.target_id.clone(),scope.node_id.clone(),scope.thread.clone()),(channel.into(),items[0]["display_name"].as_str().unwrap_or("Untitled conversation").into()));
                self.catalog_query(Purpose::Channels{target:scope.target_id.clone(),node:scope.node_id.clone(),context:BrowseContext::default()},out)?;
                // Bind metadata signals the catalog-aware access profile.
                self.subscribe_catalog(&scope.target_id,&scope.node_id,out)?;
            }
            self.subscribe(&scope,out)?;
        },
        Purpose::Renew{scope,lease}=>{if let Some(t)=self.threads.get_mut(&scope){
            if t.lease.as_deref()!=Some(lease.as_str()){return Ok(());} // late predecessor
            if v["lease_id"].as_str()!=Some(lease.as_str()){return Err("lease renewal mismatch".into());}
            t.renew_at=self.now_ms.saturating_add(v["expires_in_ms"].as_u64().unwrap_or(300_000).min(300_000)*3/4);
        }},
        Purpose::Send{scope,window,revision,text}=>{
            if let Some(d)=self.drafts.get_mut(&(window,scope.clone().into())){d.pending=None;d.status="Sent".into();if d.revision==revision&&d.text==text{d.text.clear();d.revision=d.revision.checked_add(1).ok_or("draft version exhausted")?;}}
            // Final message content is received from canonical snapshot/events,
            // not synthesized from an ACK or from the text the user typed.
        },
        Purpose::ReleaseScope(scope)=>{if self.ready{self.bind(&scope,out)?;}},
        Purpose::Catalog{..}|Purpose::Subscribe{..}|Purpose::Release|Purpose::Page{..}=>{},
    }Ok(())}
    fn failed(&mut self,p:Purpose,message:&str,out:&mut Vec<Effect>){match p{
        Purpose::IdeaCodex(op)=>self.idea_codex_failed(op,message,out),
        Purpose::Inquiry{key,view_token,..}=>{if self.inquiry_view_current(&key,view_token.as_deref()){self.inquiry_failed(&key,message)}},
        Purpose::ListTargets=>{
            // Compatibility with the old embedded-only access handler. This
            // fallback is discovery only and never redirects a send or a Pane.
            if let Err(error)=self.request("embedded",json!({"op":"describe"}),Purpose::Describe{target:"embedded".into(),context:BrowseContext::default()},out){out.push(Effect::Error{message:error});}
        },
        Purpose::Send{scope,window,..}=>{if let Some(d)=self.drafts.get_mut(&(window,scope.clone().into())){d.pending=None;d.status=format!("Unconfirmed — {message}");}self.status(&scope,"send_unconfirmed",message,out);},
        Purpose::Bind(scope)=>{
            self.inquiry_revoke(&scope);
            if let Some(t)=self.threads.get_mut(&scope){t.lease=None;t.subscription=None;t.status="unavailable".into();}
            self.status(&scope,"unavailable",message,out);
        },
        Purpose::Subscribe{scope,subscription}=>{
            if self.threads.get(&scope).is_some_and(|t|t.subscription.as_deref()==Some(subscription.as_str())) {
                if let Some(t)=self.threads.get_mut(&scope){t.subscription=None;t.status="unavailable".into();}
                self.status(&scope,"unavailable",message,out);
            }
        },
        Purpose::Renew{scope,lease}=>{
            if self.threads.get(&scope).is_some_and(|t|t.lease.as_deref()==Some(lease.as_str())) {
                if let Some(t)=self.threads.get_mut(&scope){t.lease=None;t.subscription=None;t.status="unavailable".into();}
                self.status(&scope,"unavailable",message,out);
            }
        },
        Purpose::ReleaseScope(scope)=>{
            // An expired/restarted lease may already be gone. A new bind is
            // read-only setup, never an automatic retry of a business write.
            if self.ready{if let Err(error)=self.bind(&scope,out){out.push(Effect::Error{message:error});}}
        },
        Purpose::Page{scope,subscription}=>{self.pages.remove(&subscription);self.status(&scope,"page_failed",message,out);},
        Purpose::Describe{context,..}|Purpose::Channels{context,..}|Purpose::Threads{context,..}=>out.push(Effect::BrowseError{context,message:message.into()}),
        Purpose::Catalog{target,node,subscription}=>{
            if self.catalogs.get(&(target.clone(),node.clone())).is_some_and(|f|f.subscription==subscription){self.catalogs.remove(&(target.clone(),node));}
            for view in self.catalog_views.values().filter(|v|v.target==target){out.push(Effect::BrowseError{context:view.context.clone(),message:format!("Live catalog unavailable: {message}")});}
        },
        Purpose::Release|Purpose::Activity{..}=>out.push(Effect::Error{message:message.into()}),
    }}
    fn target_disconnected(&mut self,target:&str,out:&mut Vec<Effect>)->Result<(),String>{
        // Fence pending replies before any reconnect. The Native Host connection
        // itself remains live, so its generation alone cannot reject these.
        let requests=self.pending.iter().filter(|(_,p)|p.purpose.target()==Some(target)).map(|(id,_)|id.clone()).collect::<Vec<_>>();
        for id in requests{
            if let Some(p)=self.pending.remove(&id){
                if matches!(p.purpose,Purpose::Send{..}|Purpose::Inquiry{..}|Purpose::IdeaCodex(_)){self.failed(p.purpose,"Target disconnected; the operation may have committed",out);}
                if p.sent {
                    let request=LocalExtensionCancelV1{extension:POLAR_ACCESS_EXTENSION.into(),version:1,request_id:id};
                    out.push(Effect::Request{frame:serde_json::to_value(LocalClientFrame::ExtensionCancel{request}).map_err(|e|e.to_string())?});
                }
            }
        }
        self.queued.retain(|(id,_)|self.pending.contains_key(id));
        self.pages.retain(|_,page|page.scope.target_id!=target);
        self.catalogs.retain(|(t,_),_|t!=target);self.catalog_dirty.retain(|p|p.target()!=Some(target));
        let scopes=self.threads.keys().filter(|s|s.target_id==target).cloned().collect::<Vec<_>>();
        for scope in &scopes{
            if let Some(t)=self.threads.get_mut(scope){t.lease=None;t.subscription=None;t.sequence=0;t.boot=None;t.staging=None;t.record.clear();
                t.resync_attempts=t.resync_attempts.saturating_add(1);t.status="offline".into();}
            self.status(scope,"offline","This Polar connection ended; other targets remain active",out);
        }
        for scope in scopes{if self.ready&&self.threads.get(&scope).is_some_and(|t|t.resync_attempts<=2){self.bind(&scope,out)?;}}
        Ok(())
    }
    fn disconnect(&mut self,out:&mut Vec<Effect>){self.inquiry_disconnect_views();self.ready=false;self.idea_codex=Default::default();self.queued.clear();
        let pending=std::mem::take(&mut self.pending);for (_,p) in pending{if matches!(p.purpose,Purpose::Send{..}|Purpose::Inquiry{..}|Purpose::IdeaCodex(_)){self.failed(p.purpose,"Connection closed; the operation may have committed",out);}}
        self.catalogs.clear();self.catalog_dirty.clear();
        self.pages.clear();for t in self.threads.values_mut(){t.lease=None;t.subscription=None;t.sequence=0;t.boot=None;t.staging=None;t.record.clear();t.status="offline".into();}
        for scope in self.threads.keys().cloned().collect::<Vec<_>>(){self.status(&scope,"offline","Station Host is disconnected",out);}
    }
    fn dirty(&mut self,scope:&Scope){
        let new_message=self.threads.get_mut(scope).and_then(|t|t.new_message.take());
        for v in self.views.values_mut().filter(|v|&v.scope==scope){
            if v.at_bottom&&v.tail||v.jump_pending||v.rendered.is_empty() {v.dirty=true;}
            else if let Some(key)=&new_message {v.reading.arrived(key.clone());}
        }
    }
    fn reading_hints(&mut self,out:&mut Vec<Effect>){
        for (pane,v) in &mut self.views {
            let active=self.threads.get(&v.scope).is_some_and(|t|t.transcript.generations.values().any(|g|!matches!(g["state"].as_str(),Some("completed"|"failed"|"cancelled"))));
            let failed=self.threads.get(&v.scope).is_some_and(|t|t.transcript.generations.values().any(|g|g["state"]=="failed"));
            let hint=v.reading.hint(active,failed,v.render_failed);
            let visible=hint.visible;let label=hint.label;
            let state=format!("{visible}:{label}");
            if state!=v.hint {v.hint=state;out.push(Effect::ReadingHint{pane_id:pane.clone(),visible,label,retry:v.render_failed});}
        }
    }
    fn protected_ranges(&self)->BTreeMap<Scope,BTreeSet<(String,String)>> {
        let mut ranges:BTreeMap<Scope,BTreeSet<(String,String)>>=BTreeMap::new();
        for view in self.views.values() {
            let keys=ranges.entry(view.scope.clone()).or_default();
            keys.extend(view.display_window.anchors.iter().map(|anchor|anchor.key.clone()));
            if let Some(pending)=&view.pending_render {keys.extend(pending.window.anchors.iter().map(|anchor|anchor.key.clone()));}
            keys.extend(view.anchor.iter().chain(view.backstack.iter()).map(|anchor|anchor.key.clone()));
        }
        ranges
    }
    fn status(&mut self,scope:&Scope,state:&str,message:&str,out:&mut Vec<Effect>){if matches!(state,"offline"|"unavailable"){if let Some(t)=self.threads.get_mut(scope){t.connection_message=message.into();}}for (id,_) in self.views.iter().filter(|(_,v)|&v.scope==scope){out.push(Effect::Status{pane_id:id.clone(),state:state.into(),message:message.into()});}}
    fn render(&mut self,out:&mut Vec<Effect>)->Result<(),String>{
        for (id,v) in &mut self.views{if !v.dirty||!v.visible||v.blocked||v.render_failed||v.pending_render.is_some(){continue;}v.dirty=false;
            let Some(t)=self.threads.get(&v.scope)else{continue};
            crate::window::reconcile_tools(&t.transcript,&mut v.tools);
            let window=crate::window::render_with_tools(&t.transcript,v.columns,if v.jump_pending{None}else{v.anchor.as_ref()},v.tail,&v.tools);
            let mut window=window;
            if let Some(key)=&v.highlight {crate::window::highlight(&mut window,key);}
            let runs=&window.runs;
            let text=serde_json::to_string(&runs).map_err(|e|e.to_string())?;
            if text!=v.rendered{
                let previous=serde_json::from_str::<Vec<polar_edge_presentation::TextRun>>(&v.rendered).unwrap_or_default();
                let delta=crate::text::run_delta(&previous,runs);
                let operation=if delta.is_some(){4}else{3};
                let encoded=if let Some(delta)=delta{serde_json::to_string(&delta).map_err(|e|e.to_string())?}else{text.clone()};
                let next=v.revision.checked_add(1).ok_or("render revision exhausted")?;
                out.push(Effect::Render{pane_id:id.clone(),base_revision:v.revision,next_revision:next,operation,text:encoded});
                v.pending_render=Some(PendingRender{revision:next,text,window});
            } else {v.display_window=window;out.push(reference_controls(id,v));if v.jump_pending {out.push(Effect::ScrollLatest{pane_id:id.clone()});}}
            out.push(Effect::Attachments{pane_id:id.clone(),scope:v.scope.clone(),items:t.transcript.attachment_rows()});
        }Ok(())
    }
    fn composer_scope(&self,window:&str)->Result<(String,Scope),String>{let pane=self.selected.get(window).ok_or("select a conversation before composing")?;
        let view=self.views.get(pane).ok_or("selected Pane is closed")?;Ok((pane.clone(),view.scope.clone()))}
    fn composer_effects(&mut self,out:&mut Vec<Effect>){
        self.idea_composer_effects(out);
        for (window,pane) in &self.selected{
        let Some(v)=self.views.get(pane)else{continue};let draft_key=self.draft_key(window).unwrap_or_else(|_|(window.clone(),v.scope.clone().into()));let Some(d)=self.drafts.get(&draft_key)else{continue};
        let bound=self.threads.get(&v.scope).is_some_and(|t|t.lease.is_some());
        let thread=self.threads.get(&v.scope);
        let connection=thread.map(|t|t.status.as_str()).unwrap_or("offline");
        let (channel_id,thread_title)=self.thread_titles.get(&(v.scope.target_id.clone(),v.scope.node_id.clone(),v.scope.thread.clone())).cloned().unwrap_or_else(||(String::new(),v.scope.thread.clone()));
        let channel_title=self.channel_titles.get(&(v.scope.target_id.clone(),v.scope.node_id.clone(),channel_id.clone())).cloned().unwrap_or_else(||channel_id.clone());
        let effect=Effect::Composer{window_id:window.clone(),pane_id:pane.clone(),scope:v.scope.clone(),text:self.inquiries.saved_text(window,&v.scope).unwrap_or(&d.text).to_owned(),draft_revision:d.revision,
            can_send:self.ready&&bound&&thread.is_some_and(|t|t.status=="live"&&t.subscription.is_some())&&self.inquiries.can_send(window,&v.scope,&d.text).unwrap_or(d.pending.is_none()&&!d.text.trim().is_empty()),pending:d.pending.is_some(),status:thread.filter(|t|t.status!="live").map(|t|if !t.connection_message.is_empty(){t.connection_message.clone()}else{match t.status.as_str(){"binding"=>"Connecting…","loading"=>"Loading conversation…",_=>"Disconnected"}.into()}).unwrap_or_else(||d.status.clone()),connection:connection.into(),
            can_load_older:false,
            can_reconnect:!self.ready||matches!(connection,"offline"|"unavailable"),
            target_title:self.target_titles.get(&v.scope.target_id).cloned().unwrap_or_else(||v.scope.target_id.clone()),channel_id,channel_title,thread_title};
        let serialized=serde_json::to_string(&effect).unwrap_or_default();if self.composer_cache.get(window)!=Some(&serialized){self.composer_cache.insert(window.clone(),serialized);out.push(effect);}
    }}
}
/// Project display labels once; native adapters never guess a domain field.
fn catalog_projection(items:&Value,name_field:&str,empty_label:&str)->Value{
    let mut items=items.clone();
    if let Some(rows)=items.as_array_mut(){for row in rows{
        let name=row[name_field].as_str().map(str::trim).filter(|v|!v.is_empty()).unwrap_or(empty_label).to_owned();
        if let Some(object)=row.as_object_mut(){object.insert("display_name".into(),Value::String(name));}
    }}
    items
}
fn validate_id(id:&str)->Result<(),String>{polar_tmux_protocol::validate_stable_identifier(id,"native identifier").map_err(|e|e.to_string())}
fn apply_event(scope:&Scope,t:&mut ThreadState,sequence:u64,p:&Value)->Result<bool,String>{
    if Some(sequence)!=t.sequence.checked_add(1){return Err("subscription sequence gap".into());}t.sequence=sequence;
    let boot=p["boot_epoch"].as_str().ok_or("event has no boot epoch")?;
    if let Some(old)=&t.boot{if old!=boot{return Err("Polar process identity epoch changed".into());}}else{t.boot=Some(boot.into());}
    t.new_message=None;
    match p["kind"].as_str().ok_or("event has no kind")?{
        "snapshot_begin"=>{if p["node_id"].as_str()!=Some(&scope.node_id)||p["thread_id"].as_str()!=Some(&scope.thread){return Err("snapshot scope mismatch".into());}t.staging=Some(Transcript::default());t.status="loading".into();},
        "snapshot_end"=>{
            let snapshot=t.staging.take().ok_or("snapshot did not begin")?;
            let rows=snapshot.shared.sorted_messages();
            t.snapshot_boundary=rows.last().map(polar_lgp_reader::message_key).transpose()?;
            if p["resumed"]==true {
                t.transcript.shared.merge_snapshot(&scope.thread,snapshot.shared)?;
                // Verified pages carry their own coverage; disjoint ranges must not be joined.
                t.transcript.cards.borrow_mut().invalidate(&t.transcript.cache_namespace);
            }else{
                t.transcript.merge_snapshot(&scope.thread,snapshot)?;
                t.transcript.shared.mark_page(&rows,None,p["has_more"].as_bool(),Some(false));
            }
            if let Some(more)=p["has_more"].as_bool(){t.has_more=more;}
            // The completed server snapshot supplies the authoritative boundary,
            // including null when the previous anchor has disappeared.
            if p.get("resume_message").is_some() {t.transcript.shared.resume_message=None;}
            if let Some(key)=p.get("resume_message").filter(|key|!key.is_null()) {
                let sender=key["sender_id"].as_str().ok_or("missing resume sender")?;
                let message=key["message_id"].as_str().ok_or("missing resume message")?;
                t.transcript.shared.acknowledge_message((sender.into(),message.into()))?;
            } else if p.get("resume_message").is_none() {
                if let Some(last)=rows.last(){t.transcript.shared.acknowledge_message(polar_lgp_reader::message_key(last)?)?;}
            }
            t.status=if t.repair_after.is_some(){"loading"}else{"live"}.into();
            if let Some(boundary)=&t.repair_after {t.transcript.shared.resume_message=Some(boundary.clone());}
            t.resync_attempts=0;
        },
        "reset_required"=>return Ok(true),
        "record_begin"|"record_chunk"|"record_end"=>{if let Some(mut r)=t.record.accept(p)?{
            if r["kind"]=="message_ref" {
                let key=(r["key"]["sender_id"].as_str().ok_or("missing retained sender")?.to_owned(),
                    r["key"]["message_id"].as_str().ok_or("missing retained message")?.to_owned());
                r["value"]=t.staging.as_ref().and_then(|stage|stage.messages.get(&key))
                    .or_else(||t.transcript.messages.get(&key)).cloned().ok_or("retained message was evicted during recovery")?;
                r["kind"]=json!("message");
            }
            let live=t.staging.is_none();
            let data=t.staging.as_mut().unwrap_or(&mut t.transcript);
            match r["kind"].as_str(){Some("message")=>{
                let key=(r["value"]["sender_node_id_hex"].as_str().unwrap_or("").to_owned(),r["value"]["message_id"].as_str().unwrap_or("").to_owned());
                let fresh=!data.messages.contains_key(&key);
                let previous=if live {data.shared.cursor(false).and_then(|value|Some((value["sender_id"].as_str()?.to_owned(),value["message_id"].as_str()?.to_owned())))}else{None};
                data.put_message(&scope.thread,r["value"].clone())?;
                if live && t.repair_after.is_none() {data.shared.acknowledge_message(key.clone())?;}
                if live&&fresh {if let Some(previous)=previous {data.shared.mark_append(std::slice::from_ref(&r["value"]),Some(&previous));}t.new_message=Some(key);}
            },Some("generation")=>data.put_generation(&scope.owner,&scope.thread,r["value"].clone())?,
            Some("verified_page")=>{
                let rows=r["keys"].as_array().ok_or("invalid verified page")?.iter().map(|key| {
                    let key=(key["sender_id"].as_str().ok_or("missing verified sender")?.to_owned(),key["message_id"].as_str().ok_or("missing verified identity")?.to_owned());
                    data.messages.get(&key).cloned().ok_or("verified page contains unadmitted message")
                }).collect::<Result<Vec<_>,_>>()?;
                data.shared.mark_page(&rows,None,r["has_before"].as_bool(),r["has_after"].as_bool());
            },_=>return Err("unknown canonical record kind".into())}
        }},_=>return Err("unknown stream event".into()),
    }Ok(false)
}





