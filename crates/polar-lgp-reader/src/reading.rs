use serde::{Serialize, Deserialize};
use std::collections::BTreeSet;

/// Source node offsets and grapheme position are stable across display widths.
/// Physical rows/pixels are deliberately absent from the stored read anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAnchor {
    pub key: (String,String),
    pub start: usize,
    pub end: usize,
    pub grapheme: usize,
}

#[derive(Clone)]
pub struct ReadingState {
    pub anchor: Option<SourceAnchor>,
    pub backstack: Vec<SourceAnchor>,
    pub highlight: Option<(String,String)>,
    pub at_bottom: bool,
    pub tail: bool,
    pub jump_pending: bool,
    pub new_messages: BTreeSet<(String,String)>,
    pub unknown_new: bool,
}
impl Default for ReadingState {
    fn default()->Self { Self{anchor:None,backstack:Vec::new(),highlight:None,at_bottom:true,tail:true,jump_pending:false,new_messages:BTreeSet::new(),unknown_new:false} }
}
#[derive(Serialize)]
pub struct ReadingHint { pub visible:bool, pub label:String, pub retry:bool }
impl ReadingState {
    pub fn locate(&mut self,key:(String,String),previous:Option<SourceAnchor>) {
        if let Some(anchor)=previous {if self.backstack.len()==32{self.backstack.remove(0);}self.backstack.push(anchor);}
        self.anchor=Some(SourceAnchor{key:key.clone(),start:0,end:0,grapheme:0});
        self.highlight=Some(key);self.tail=false;self.at_bottom=false;self.jump_pending=false;
    }
    pub fn return_to_reading(&mut self)->bool {
        let Some(anchor)=self.backstack.pop()else{return false;};
        self.anchor=Some(anchor);self.highlight=None;self.tail=false;self.at_bottom=false;self.jump_pending=false;true
    }
    pub fn following(&self)->bool { self.at_bottom && self.tail }
    pub fn arrived(&mut self,key:(String,String)) {
        if self.following() {return;}
        if self.new_messages.len()<8192 {self.new_messages.insert(key);} else {self.unknown_new=true;}
    }
    pub fn applied_latest(&mut self) {
        self.new_messages.clear();self.unknown_new=false;self.jump_pending=false;
    }
    pub fn hint(&self,active:bool,failed:bool,display_failed:bool)->ReadingHint {
        let visible=display_failed||!self.at_bottom||!self.tail||self.jump_pending||!self.new_messages.is_empty()||self.unknown_new;
        let mut label=if display_failed {"Display failed — retry".to_owned()}
            else if self.unknown_new {"↓ New messages".to_owned()}
            else if !self.new_messages.is_empty(){format!("↓ {} new messages",self.new_messages.len())}
            else {"↓ Back to latest".to_owned()};
        if !display_failed {if active {label.push_str(" · Replying");}else if failed {label.push_str(" · Reply failed");}}
        ReadingHint{visible,label,retry:display_failed}
    }
}

