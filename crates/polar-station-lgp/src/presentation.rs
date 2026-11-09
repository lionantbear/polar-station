//! Declarative attachment document validation. Parsing never opens a Pane.
//! Authority is supplied by the native user gesture or an independently
//! authorized agent tool, not by `activation` or other message content.
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};

pub const MIME: &str = "application/vnd.polar.station-view+json";
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema: String, pub activation: String,
    pub views: Vec<View>, pub actions: Vec<Action>,
    pub layout_hint: Option<Layout>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {pub id:String,pub renderer:Renderer,pub attachment_id:String}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Renderer {Image,Pdf,File,Diff}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id:String,pub kind:ActionKind,pub label:String,
    pub view_ids:Option<Vec<String>>,pub choice:Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum ActionKind {OpenViews,ReplyChoice,CopyReference}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {pub relation:Relation,pub arrangement:Arrangement,pub preserve_focus:bool}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Relation {RightOfSource,LeftOfSource,NewTab,CurrentTab}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Arrangement {Single,SideBySide,Stack}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub node_id:String,pub owner:String,pub thread:String,
    pub sender_id:String,pub message_id:String,
    pub attachment_id:String,pub digest:String,
}
impl Origin {
    pub fn validate(&self)->Result<(),String> {
        for s in [&self.node_id,&self.owner,&self.thread,&self.sender_id,&self.message_id,&self.attachment_id] {
            polar_tmux_protocol::validate_stable_identifier(s,"presentation origin").map_err(|e|e.to_string())?;
        }
        // The digest must come from the authenticated attachment resolver.
        if self.digest.len()!=64 || !self.digest.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("expected a verified SHA-256 digest".into());}
        Ok(())
    }
}
#[derive(Debug,Clone,Serialize)]
#[serde(tag="kind",rename_all="snake_case")]
pub enum Plan {
    OpenViews{origin:Origin,action_id:String,views:Vec<View>,layout:Option<Layout>},
    ReplyChoice{origin:Origin,action_id:String,choice:String},
    CopyReference{origin:Origin,action_id:String},
}
impl Document {
    pub fn parse(bytes:&[u8], authorized_attachments:&BTreeSet<String>)->Result<Self,String>{
        if bytes.len()>MAX_MANIFEST_BYTES{return Err("presentation document exceeds 64 KiB".into());}
        let doc:Self=serde_json::from_slice(bytes).map_err(|e|format!("invalid presentation: {e}"))?;
        if doc.schema!="polar.station.view.v1"||doc.activation!="suggest"{return Err("unsupported presentation contract".into());}
        if doc.views.is_empty()||doc.views.len()>16||doc.actions.len()>32{return Err("presentation item limit".into());}
        let mut views=BTreeSet::new();
        for view in &doc.views {
            id(&view.id)?;id(&view.attachment_id)?;
            if !views.insert(view.id.clone()){return Err("duplicate view ID".into());}
            if !authorized_attachments.contains(&view.attachment_id){return Err("view references an unavailable message attachment".into());}
        }
        let mut actions=BTreeSet::new();
        for action in &doc.actions {
            id(&action.id)?;
            if !actions.insert(action.id.clone()){return Err("duplicate action ID".into());}
            if action.label.is_empty()||action.label.chars().count()>120||action.label.chars().any(char::is_control){return Err("invalid action label".into());}
            match action.kind {
                ActionKind::OpenViews=>{
                    if action.choice.is_some(){return Err("open_views cannot supply a reply choice".into());}
                    let ids=action.view_ids.as_ref().ok_or("missing view_ids")?;
                    if ids.is_empty()||ids.len()>16{return Err("invalid view count".into());}
                    let mut unique=BTreeSet::new();
                    for name in ids{if !views.contains(name)||!unique.insert(name){return Err("unknown or duplicate view reference".into());}}
                },
                ActionKind::ReplyChoice=>{
                    if action.view_ids.is_some(){return Err("reply_choice cannot open views".into());}
                    let choice=action.choice.as_ref().ok_or("missing choice")?;
                    if choice.is_empty()||choice.chars().count()>128||choice.chars().any(char::is_control){return Err("invalid choice".into());}
                },
                ActionKind::CopyReference=>if action.view_ids.is_some()||action.choice.is_some(){return Err("copy_reference has unexpected arguments".into());},
            }
        }
        Ok(doc)
    }
    /// Called only as a result of an explicit native interaction. Receiving or
    /// restoring a message never calls this method by itself.
    pub fn plan_user_action(&self,origin:Origin,action_id:&str)->Result<Plan,String>{
        origin.validate()?;
        let a=self.actions.iter().find(|a|a.id==action_id).ok_or("unknown action")?;
        Ok(match a.kind {
            ActionKind::OpenViews=>Plan::OpenViews{origin,action_id:a.id.clone(),views:a.view_ids.as_ref().ok_or("missing views")?.iter().map(|id|self.views.iter().find(|v|&v.id==id).cloned().ok_or("unknown view".to_owned())).collect::<Result<Vec<_>,_>>()?,layout:self.layout_hint.clone()},
            ActionKind::ReplyChoice=>Plan::ReplyChoice{origin,action_id:a.id.clone(),choice:a.choice.clone().ok_or("missing choice")?},
            ActionKind::CopyReference=>Plan::CopyReference{origin,action_id:a.id.clone()},
        })
    }
}
fn id(value:&str)->Result<(),String>{
    if value.is_empty()||value.len()>128||!value.bytes().next().is_some_and(|b|b.is_ascii_alphanumeric())||!value.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-'){
        return Err("invalid presentation identifier".into());
    }Ok(())
}
/// Local, bounded action receipts. An agent invocation has its own authenticated
/// request ID; UI history replay does not generate one. Unknown/interrupted
/// actions remain claimed until explicit reconciliation, never silently retried.
#[derive(Debug,Default,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionJournal {claims:BTreeMap<String,Claim>}
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim{origin:Origin,action_id:String,completed:bool}
impl ActionJournal {
    pub fn claim(&mut self,request_id:&str,origin:Origin,action_id:&str)->Result<bool,String>{
        id(request_id)?;id(action_id)?;origin.validate()?;
        if let Some(old)=self.claims.get(request_id){
            if old.origin!=origin||old.action_id!=action_id{return Err("action request identity collision".into());}
            return Ok(false);
        }
        if self.claims.len()>=4096{return Err("action journal requires explicit retention cleanup".into());}
        self.claims.insert(request_id.into(),Claim{origin,action_id:action_id.into(),completed:false});Ok(true)
    }
    pub fn complete(&mut self,request_id:&str)->Result<(),String>{self.claims.get_mut(request_id).ok_or("unknown action request")?.completed=true;Ok(())}
    pub fn restore(bytes:&[u8])->Result<Self,String>{
        if bytes.len()>4*1024*1024{return Err("action journal too large".into());}
        let journal:Self=serde_json::from_slice(bytes).map_err(|e|e.to_string())?;
        if journal.claims.len()>4096{return Err("too many action receipts".into());}
        for (request,c) in &journal.claims{id(request)?;id(&c.action_id)?;c.origin.validate()?;}
        Ok(journal)
    }
}

