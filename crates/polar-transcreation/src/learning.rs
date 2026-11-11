//! Bounded personal terminology learned only from confirmed draft submissions.
//! Transport/model execution and persistence are supplied by the Host.
use serde::{Deserialize,Serialize};
use std::collections::{BTreeMap,BTreeSet};

pub const MAX_RULES:usize=32;
pub const MAX_PROMPT_BYTES:usize=8192;
pub const SAMPLE_TTL_MS:u64=7*24*60*60*1000;
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Refinement {
    pub original:String,
    pub refined:String,
    pub english:bool,
    pub project:Option<String>,
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct LearningSample {
    pub intent:String,
    pub refinement:Refinement,
    pub final_text:String,
    pub created_at_ms:u64,
}
impl LearningSample {
    pub fn validate(&self)->Result<(),String>{
        if self.intent.is_empty() || self.intent.len()>128 || self.intent.chars().any(char::is_control)
            || [&self.refinement.original,&self.refinement.refined,&self.final_text].iter().any(|s|s.is_empty()||s.len()>65536||s.contains('\0'))
            || self.refinement.original.len()+self.refinement.refined.len()+self.final_text.len()>crate::policy::MAX_TEXT_BYTES
            || self.refinement.refined==self.final_text
            || self.refinement.project.as_ref().is_none_or(|p|!p.starts_with('/')||p.len()>4096||p.chars().any(char::is_control)) {
            return Err("No valid project-scoped learning sample".into());
        }
        Ok(())
    }
}
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct Replacement {pub from:String,pub to:String}
impl Replacement {
    pub fn valid(&self)->bool{
        self.from!=self.to && [&self.from,&self.to].iter().all(|s|!s.is_empty() && s.len()<=128 && !s.chars().any(char::is_control) && s.split_whitespace().count()<=3)
    }
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Rule {
    pub replacement:Replacement,
    pub english:bool,
    pub projects:BTreeSet<String>,
    pub retired:bool,
    pub reason:String,
}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct Preferences {
    pub disabled:bool,
    pub revision:u64,
    pub rules:Vec<Rule>,
    /// Highest processed timestamp per intent; bounded store receipts own dedup.
    #[serde(default)]
    pub processed:BTreeMap<String,u64>,
}
impl Preferences {
    pub fn reference(&self,project:Option<&str>,english:bool)->Vec<Replacement>{
        if self.disabled{return vec![]}
        let mut selected=Vec::new();let mut bytes=0;
        // Project evidence wins over global evidence for the same source token.
        for local in [true,false] {
            for rule in &self.rules {
                let exact=project.is_some_and(|p|rule.projects.contains(p));
                if rule.retired||rule.english!=english||exact!=local||(!exact&&rule.projects.len()<2)
                    ||selected.iter().any(|r:&Replacement|r.from==rule.replacement.from){continue}
                let size=serde_json::to_vec(&rule.replacement).map_or(MAX_PROMPT_BYTES+1,|v|v.len());
                if bytes+size>MAX_PROMPT_BYTES{continue}bytes+=size;selected.push(rule.replacement.clone());
            }
        }
        selected
    }
    pub fn learn(&mut self,sample:&LearningSample,candidates:Vec<Replacement>,now_ms:u64)->Result<bool,String>{
        sample.validate()?;
        if self.disabled || now_ms.saturating_sub(sample.created_at_ms)>SAMPLE_TTL_MS || self.processed.contains_key(&sample.intent){return Ok(false)}
        let project=sample.refinement.project.as_deref().ok_or("Project unavailable")?;
        for rule in &mut self.rules {
            if !rule.retired && rule.english==sample.refinement.english && (rule.projects.contains(project)||rule.projects.len()>=2)
                && sample.refinement.original.contains(&rule.replacement.from)
                && sample.refinement.refined.contains(&rule.replacement.to)
                && !sample.refinement.original.contains(&rule.replacement.to)
                && sample.final_text.contains(&rule.replacement.from)
                && !sample.final_text.contains(&rule.replacement.to) {
                rule.retired=true;rule.reason="User reverted this applied correction before sending".into();
            }
        }
        for replacement in candidates.into_iter().take(8) {
            if !replacement.valid() || !sample.refinement.refined.contains(&replacement.from) || !sample.final_text.contains(&replacement.to){continue}
            if let Some(existing)=self.rules.iter_mut().find(|r|r.english==sample.refinement.english&&r.replacement==replacement){
                if !existing.retired && existing.projects.len()<2{existing.projects.insert(project.into());}
                continue;
            }
            if self.rules.len()>=1024 || self.rules.iter().filter(|r|!r.retired&&r.projects.contains(project)).count()>=MAX_RULES{continue}
            self.rules.push(Rule{replacement,english:sample.refinement.english,projects:BTreeSet::from([project.into()]),retired:false,reason:"Confirmed correction in successfully sent draft".into()});
        }
        // Promotion cannot grow the global prompt without bound.
        let mut globals=0;
        for rule in &mut self.rules {
            if !rule.retired&&rule.projects.len()>=2 {globals+=1;if globals>MAX_RULES{rule.retired=true;rule.reason="Global preference limit reached".into();}}
        }
        self.processed.insert(sample.intent.clone(),sample.created_at_ms);
        self.processed.retain(|_,t|now_ms.saturating_sub(*t)<=SAMPLE_TTL_MS);
        while self.processed.len()>4096 {let key=self.processed.iter().min_by_key(|(_,t)|*t).map(|(k,_)|k.clone()).ok_or("Invalid learning receipts")?;self.processed.remove(&key);}
        self.revision=self.revision.checked_add(1).ok_or("Preference revision exhausted")?;
        Ok(true)
    }
}


#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct PendingSample {pub sample:LearningSample,pub failed:bool}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct LearningState {
    pub preferences:Preferences,
    pub pending:BTreeMap<String,PendingSample>,
    pub cleared_at_ms:u64,
}
#[derive(Clone,Debug)]
pub enum LearningChange {
    Admit(LearningSample),
    Complete{intent:String,candidates:Option<Vec<Replacement>>,now_ms:u64},
    Configure{expected_revision:u64,enabled:bool,retire:Option<usize>,clear:bool,now_ms:u64},
}
impl LearningState {
    pub fn apply(&mut self,change:LearningChange)->Result<(),String>{
        match change {
            LearningChange::Admit(sample)=>{
                sample.validate()?;
                if self.preferences.disabled || sample.created_at_ms<=self.cleared_at_ms || self.preferences.processed.contains_key(&sample.intent)||self.pending.contains_key(&sample.intent){return Ok(())}
                if self.pending.len()>=64{return Err("Automatic learning pending sample limit reached".into())}
                self.pending.insert(sample.intent.clone(),PendingSample{sample,failed:false});
            }
            LearningChange::Complete{intent,candidates,now_ms}=>{
                if let Some(pending)=self.pending.remove(&intent){
                    if now_ms.saturating_sub(pending.sample.created_at_ms)>SAMPLE_TTL_MS || pending.sample.created_at_ms<=self.cleared_at_ms{return Ok(())}
                    if let Some(candidates)=candidates {self.preferences.learn(&pending.sample,candidates,now_ms)?;}
                    else {self.pending.insert(intent,PendingSample{failed:true,..pending});}
                }
            }
            LearningChange::Configure{expected_revision,enabled,retire,clear,now_ms}=>{
                if self.preferences.revision!=expected_revision{return Err("Learning preferences changed; refresh and retry".into())}
                if let Some(index)=retire {let rule=self.preferences.rules.get_mut(index).ok_or("Learning rule no longer exists")?;rule.retired=true;rule.reason="Disabled by user".into();}
                if clear {
                    for rule in &mut self.preferences.rules{rule.retired=true;rule.reason="Cleared by user".into();}
                    self.pending.clear();self.cleared_at_ms=now_ms;
                }
                self.preferences.disabled = !enabled;
                self.preferences.revision=self.preferences.revision.checked_add(1).ok_or("Learning preference revision exhausted")?;
            }
        }
        Ok(())
    }
}
pub trait LearningStore:Send+Sync {
    fn load(&self)->Result<LearningState,String>;
    fn change(&self,change:LearningChange)->Result<LearningState,String>;
}
#[async_trait::async_trait]
pub trait LearningModel:Send+Sync {
    async fn infer(&self,sample:&LearningSample)->Result<Vec<Replacement>,String>;
}
pub struct LearningRuntime {
    store:std::sync::Arc<dyn LearningStore>,
    model:std::sync::Arc<dyn LearningModel>,
    wake:tokio::sync::Notify,
}
impl LearningRuntime {
    pub fn new(store:std::sync::Arc<dyn LearningStore>,model:std::sync::Arc<dyn LearningModel>)->Self {Self{store,model,wake:tokio::sync::Notify::new()}}
    pub fn admit(&self,sample:LearningSample)->Result<(),String>{self.store.change(LearningChange::Admit(sample))?;self.wake.notify_one();Ok(())}
    pub fn state(&self)->Result<LearningState,String>{self.store.load()}
    pub fn configure(&self,change:LearningChange)->Result<LearningState,String>{self.store.change(change)}
    /// One event-driven Host worker, with an executor independent of Refine.
    pub async fn run(&self)->Result<(),String>{
        self.wake.notify_one();
        loop {
            self.wake.notified().await;
            loop {
                let state=self.store.load()?;
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|v|v.as_millis().min(u128::from(u64::MAX)) as u64);
                let Some(pending)=state.pending.values().find(|p|!p.failed || now.saturating_sub(p.sample.created_at_ms)>SAMPLE_TTL_MS) else {break};
                let result=if now.saturating_sub(pending.sample.created_at_ms)>SAMPLE_TTL_MS {Some(vec![])}else{self.model.infer(&pending.sample).await.ok()};
                self.store.change(LearningChange::Complete{intent:pending.sample.intent.clone(),candidates:result,now_ms:now})?;
            }
        }
    }
}
