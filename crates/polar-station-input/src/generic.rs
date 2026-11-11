//! Generic drafts are independent of transient window selection and delivery credentials.
use polar_station_session::{SendRequest, SessionReference};
use polar_tmux_protocol::{CodexBindingV1, CodexCommandV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Existing snapshots stored an optional single value at these keys.
fn one_or_many<'de,D,T>(deserializer:D)->Result<Vec<T>,D::Error>
where D:serde::Deserializer<'de>,T:Deserialize<'de> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved<T> { Many(Vec<T>), One(T) }
    Ok(match Option::<Saved<T>>::deserialize(deserializer)? {
        None=>Vec::new(), Some(Saved::Many(v))=>v, Some(Saved::One(v))=>vec![v],
    })
}


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Destination {
    Terminal(SessionReference),
    Codex(CodexBindingV1),
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub instance_id: String,
    pub window: String,
    pub pane: String,
    pub context_revision: u64,
    pub draft_key: String,
    pub draft_revision: u64,
}

impl Lease {
    pub(crate) fn same_context(&self, other: &Self) -> bool {
        self.instance_id == other.instance_id
            && self.window == other.window
            && self.pane == other.pane
            && self.context_revision == other.context_revision
            && self.draft_key == other.draft_key
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Draft {
    #[serde(default,deserialize_with="one_or_many")]
    refinement:Vec<crate::learning::Refinement>,
    #[serde(default)]
    refinement_revision:Option<u64>,
    #[serde(default,deserialize_with="one_or_many")]
    learn_pending:Vec<crate::learning::LearningSample>,
    #[serde(default)]
    pending: Option<Pending>,
    text: String,
    revision: u64,
    #[serde(default,skip_serializing_if="Option::is_none")]
    last_send_error:Option<String>,
}

#[derive(Clone, Debug)]
struct Context {
    pane: String,
    key: String,
    revision: u64,
    destination: Destination,
    label: String,
    project:Option<String>,
    conversation_context:Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GenericView {
    pub project:Option<String>,
    pub conversation_context:Option<String>,
    pub target_label: String,
    pub target_status: Option<&'static str>,
    pub lease: Lease,
    pub text: String,
    pub unavailable: Option<String>,
    pub can_send: bool,
    pub send_label: &'static str,
    pub can_refresh: bool,
}

/// A frozen submission candidate, not permission to perform I/O before saving it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Submission {
    Terminal(SendRequest),
    Codex {
        owner_host_id: String,
        command: CodexCommandV1,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    intent: String,
    revision: u64,
    request: Submission,
    #[serde(default,deserialize_with="one_or_many")]
    learning:Vec<crate::learning::LearningSample>,
    #[serde(default)]
    source_count:Option<usize>,
    #[serde(default)]
    dispatched: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(skip)]
    deadline_ms: Option<u64>,
}

pub struct GenericDrafts {
    instance_id: String,
    contexts: BTreeMap<String, Context>,
    drafts: BTreeMap<String, Draft>,
    revision: u64,
    pub(crate) now_ms:u64,
}

impl Default for GenericDrafts {
    fn default() -> Self {
        Self {
            instance_id: polar_identity::generation::new_uuid_v4().to_string(),
            contexts: BTreeMap::new(),
            drafts: BTreeMap::new(),
            revision: 0,
            now_ms:0,
        }
    }
}

impl GenericDrafts {
    /// Called with stable native identities and freshly observed Host credentials.
    /// Selection observation alone never requests keyboard focus.
    pub fn observe(
        &mut self,
        window: String,
        pane: String,
        key: String,
        destination: Destination,
    ) -> Result<GenericView, String> {
        for id in [&window, &pane, &key] {
            if id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control) {
                return Err("Invalid input context identity".into());
            }
        }
        if !self.drafts.contains_key(&key) && self.drafts.len() >= 256 {
            return Err("Saved input draft capacity reached".into());
        }
        if let Some(current) = self.contexts.get(&window) {
            if current.pane == pane && current.key == key && current.destination == destination {
                return self
                    .view(&window)
                    .ok_or_else(|| "Input context missing".into());
            }
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("Input context revision exhausted")?;
        self.drafts.entry(key.clone()).or_default();
        let label = self
            .contexts
            .get(&window)
            .filter(|c| c.pane == pane && c.key == key)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| "Terminal input".into());
        let project=self.contexts.get(&window).filter(|c|c.pane==pane&&c.key==key).and_then(|c|c.project.clone());
        let conversation_context=self.contexts.get(&window).filter(|c|c.pane==pane&&c.key==key&&matches!(&destination,Destination::Codex(_))).and_then(|c|c.conversation_context.clone());
        self.contexts.insert(
            window.clone(),
            Context {
                pane,
                key,
                revision: self.revision,
                destination,
                label,
                project,
                conversation_context,
            },
        );
        self.view(&window)
            .ok_or_else(|| "Input context missing".into())
    }

    pub fn view(&self, window: &str) -> Option<GenericView> {
        let context = self.contexts.get(window)?;
        let draft = self.drafts.get(&context.key)?;
        Some(GenericView {
            project:context.project.clone(),
            conversation_context:if matches!(&context.destination,Destination::Codex(_)){context.conversation_context.clone()}else{None},
            target_label: context.label.clone(),
            target_status: if let Some(pending) = &draft.pending {
                Some(if pending.error.is_some() { "Sending result unknown" } else { "Sending…" })
            } else if draft.last_send_error.is_some() { Some("Send failed")
            } else if matches!(context.destination, Destination::Unavailable(_)) { Some("Target unavailable") } else { None },
            lease: Lease {
                instance_id: self.instance_id.clone(),
                window: window.into(),
                pane: context.pane.clone(),
                context_revision: context.revision,
                draft_key: context.key.clone(),
                draft_revision: draft.revision,
            },
            text: draft.text.clone(),
            can_refresh: draft.pending.is_some() || matches!(context.destination, Destination::Unavailable(_)),
            can_send: !draft.text.trim().is_empty()
                && draft.pending.as_ref().is_none_or(|pending| pending.error.is_some()),
            send_label: match &draft.pending {
                Some(pending) if pending.error.is_some() => "Retry",
                Some(_) => "Sending…",
                None => "Send",
            },
            unavailable: if draft.pending.is_some() {
                Some(draft.pending.as_ref().and_then(|p| p.error.clone()).unwrap_or_else(|| "Sending is pending or unconfirmed; draft retained".into()))
            } else if let Some(error)=&draft.last_send_error {
                Some(error.clone())
            } else {
                match &context.destination {
                    Destination::Unavailable(reason) => Some(reason.clone()),
                    Destination::Terminal(_) if !draft.text.trim().is_empty() => {
                        polar_station_session::validate_terminal_text(&draft.text)
                            .err()
                            .map(|e| e.to_string())
                    }
                    Destination::Codex(_)
                        if draft.text.len() > polar_tmux_protocol::CODEX_INPUT_MAX_BYTES =>
                    {
                        Some("Input exceeds the Codex 16 KiB limit; draft retained".into())
                    }
                    _ => None,
                }
            },
        })
    }

    fn validate(&self, lease: &Lease) -> Result<&Context, String> {
        let context = self
            .contexts
            .get(&lease.window)
            .ok_or("Input window closed")?;
        let draft = self.drafts.get(&context.key).ok_or("Input draft missing")?;
        if lease.instance_id != self.instance_id
            || context.pane != lease.pane
            || context.key != lease.draft_key
            || context.revision != lease.context_revision
            || draft.revision != lease.draft_revision
        {
            return Err("Input target or draft changed".into());
        }
        Ok(context)
    }

    pub fn edit(&mut self, lease: &Lease, text: String) -> Result<GenericView, String> {
        self.validate(lease)?;
        if text.len() > 1024 * 1024 {
            return Err("Input draft exceeds storage budget".into());
        }
        let draft = self
            .drafts
            .get_mut(&lease.draft_key)
            .ok_or("Input draft missing")?;
        if draft.text != text {
            draft.revision = draft
                .revision
                .checked_add(1)
                .ok_or("Draft revision exhausted")?;
            draft.text = text;
            draft.last_send_error=None;
        }
        self.view(&lease.window)
            .ok_or_else(|| "Input window closed".into())
    }

    /// Returns a frozen request for the caller's durable-save barrier.
    pub fn prepare(
        &self,
        lease: &Lease,
        intent: String,
        press_enter: bool,
    ) -> Result<Submission, String> {
        let context = self.validate(lease)?;
        polar_tmux_protocol::validate_stable_identifier(&intent, "input intent")
            .map_err(|e| e.to_string())?;
        let text = self
            .drafts
            .get(&lease.draft_key)
            .ok_or("Input draft missing")?
            .text
            .clone();
        match &context.destination {
            Destination::Unavailable(reason) => Err(reason.clone()),
            Destination::Terminal(reference) => {
                reference.validate().map_err(|e| e.to_string())?;
                polar_station_session::validate_terminal_text(&text).map_err(|e| e.to_string())?;
                Ok(Submission::Terminal(SendRequest {
                    intent_id: intent,
                    reference: reference.clone(),
                    text,
                    press_enter,
                }))
            }
            Destination::Codex(binding) => {
                if text.len()>polar_tmux_protocol::CODEX_INPUT_MAX_BYTES {
                    return Err("Input exceeds the Codex 16 KiB limit; draft retained".into());
                }
                polar_tmux_protocol::validate_codex_input(&text).map_err(|e| e.to_string())?;
                Ok(Submission::Codex {
                    owner_host_id: binding.owner_host_id.clone(),
                    command: CodexCommandV1::SubmitThreadText {
                        endpoint_id: binding.endpoint_id.clone(),
                        thread_id: binding.thread_id.clone(),
                        intent_id: intent,
                        text,
                    },
                })
            }
        }
    }

    /// Stage one immutable submission. The returned snapshot must be durably saved
    /// before `saved_submission` is called. No I/O is performed here.
    pub fn stage(
        &mut self,
        lease: &Lease,
        intent: String,
        press_enter: bool,
    ) -> Result<(), String> {
        let request = self.prepare(lease, intent.clone(), press_enter)?;
        if self.drafts.values().any(|draft| {
            draft
                .pending
                .as_ref()
                .is_some_and(|pending| pending.intent == intent)
        }) {
            return Err("Submission intent already pending".into());
        }
        let draft = self
            .drafts
            .get_mut(&lease.draft_key)
            .ok_or("Input draft missing")?;
        if draft.pending.is_some() {
            return Err("Resolve the previous submission before sending again".into());
        }
        draft.last_send_error=None;
        // No alignment guesses: retain all source text and let the existing
        // conservative learner compare it with the single successfully sent draft.
        let mut groups:BTreeMap<_,crate::learning::Refinement>=BTreeMap::new();
        for source in &draft.refinement {
            let group=groups.entry((source.project.clone(),source.english)).or_insert_with(||
                crate::learning::Refinement{original:String::new(),refined:String::new(),english:source.english,project:source.project.clone()});
            if !group.original.is_empty(){group.original.push_str("\n\n");group.refined.push_str("\n\n");}
            group.original.push_str(&source.original); group.refined.push_str(&source.refined);
        }
        let grouped=groups.len()>1;
        let learning=groups.into_values().enumerate().filter_map(|(index,refinement)|{
            let sample=crate::learning::LearningSample{
                intent:if grouped {format!("{intent}:{index}")} else {intent.clone()},
                refinement,final_text:draft.text.clone(),created_at_ms:self.now_ms};
            sample.validate().ok().map(|()|sample)
        }).collect();
        draft.pending = Some(Pending {
            source_count:Some(draft.refinement.len()),
            learning,
            intent,
            revision: lease.draft_revision,
            request,
            dispatched: false,
            error: None,
            deadline_ms: None,
        });
        Ok(())
    }

    pub(crate) fn has_pending(&self,intent:&str)->bool {
        self.drafts.values().any(|d|d.pending.as_ref().is_some_and(|p|p.intent==intent))
    }
    pub(crate) fn watch_pending(&mut self, intent:&str, now_ms:u64) {
        for pending in self.drafts.values_mut().filter_map(|d|d.pending.as_mut()) {
            if pending.intent==intent {pending.deadline_ms=Some(now_ms.saturating_add(30_000));}
        }
    }
    pub(crate) fn next_deadline_ms(&self)->Option<u64> {
        self.drafts.values().filter_map(|d|d.pending.as_ref()).filter(|p|p.error.is_none()).filter_map(|p|p.deadline_ms).min()
    }
    pub(crate) fn expire_pending(&mut self,now_ms:u64)->bool {
        let mut changed=false;
        for pending in self.drafts.values_mut().filter_map(|d|d.pending.as_mut()) {
            if pending.error.is_none() && pending.deadline_ms.is_some_and(|due|due<=now_ms) {
                pending.error=Some("Sending result is unconfirmed; Retry checks the original receipt without resending text".into());
                changed=true;
            }
        }
        changed
    }

    /// Called only for a successful save of the snapshot containing this intent.
    pub fn saved_submission(&mut self, intent: &str) -> Option<Submission> {
        let pending = self
            .drafts
            .values_mut()
            .filter_map(|draft| draft.pending.as_mut())
            .find(|pending| pending.intent == intent)?;
        if pending.dispatched {
            return None;
        }
        pending.dispatched = true;
        Some(pending.request.clone())
    }

    pub(crate) fn submission_error(&mut self, intent: &str, error: String) {
        for draft in self.drafts.values_mut() {
            if let Some(pending) = draft.pending.as_mut().filter(|p| p.intent == intent) {
                pending.error = Some(format!("Sending result is unconfirmed: {}. Draft retained; refresh sending status before retrying.", error.chars().take(256).collect::<String>()));
            }
        }
    }

    pub(crate) fn reject(&mut self,intent:&str,message:String)->Result<(),String> {
        let draft=self.drafts.values_mut().find(|d|d.pending.as_ref().is_some_and(|p|p.intent==intent)).ok_or("Unknown submission intent")?;
        draft.last_send_error=Some(message.chars().take(512).collect());
        self.resolve(intent,false)
    }

    /// Accept only a verified backend receipt for the immutable original intent.
    /// Unknown delivery deliberately leaves the pending record intact.
    pub fn resolve(&mut self, intent: &str, accepted: bool) -> Result<(), String> {
        let draft = self
            .drafts
            .values_mut()
            .find(|draft| {
                draft
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.intent == intent)
            })
            .ok_or("Unknown submission intent")?;
        let pending = draft.pending.as_ref().ok_or("Unknown submission intent")?;
        if accepted {
            for sample in &pending.learning {
                if !draft.learn_pending.iter().any(|s|s.intent==sample.intent) && draft.learn_pending.len()<64 {
                    draft.learn_pending.push(sample.clone());
                }
            }
            let consumed=pending.source_count.unwrap_or(usize::from(!pending.learning.is_empty()));
            draft.refinement.drain(..consumed.min(draft.refinement.len()));
        }
        if accepted && draft.revision == pending.revision {
            draft.revision = draft
                .revision
                .checked_add(1)
                .ok_or("Draft revision exhausted")?;
            draft.text.clear();
        }
        draft.pending = None;
        Ok(())
    }

    /// Only drafts are persistent. Execution credentials and selection are not restored.
    pub fn snapshot(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(&self.drafts).map_err(|e| e.to_string())
    }

    pub fn restore_drafts(&mut self, value: serde_json::Value) -> Result<(), String> {
        let mut drafts: BTreeMap<String, Draft> =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        if drafts.len() > 256 {
            return Err("Too many saved input drafts".into());
        }
        for (key, draft) in &drafts {
            if key.is_empty()
                || key.len() > 1024
                || key.chars().any(char::is_control)
                || draft.text.len() > 1024 * 1024
                || draft.refinement.len()>32 || draft.learn_pending.len()>64
                || draft.refinement.iter().map(|r|r.original.len()+r.refined.len()).sum::<usize>()>65536
                || draft.last_send_error.as_ref().is_some_and(|e|e.chars().count()>512)
            {
                return Err("Invalid saved input draft".into());
            }
        }
        // A process may have exited either side of the I/O boundary. Never
        // replay restored submissions, including ones persisted before dispatch.
        for draft in drafts.values_mut() {
            if let Some(pending) = &mut draft.pending {
                pending.dispatched = true;
                pending.error.get_or_insert_with(|| "Previous send is unconfirmed; Retry checks its receipt without resending text".into());
            }
        }
        self.drafts = drafts;
        self.contexts.clear();
        Ok(())
    }

    pub(crate) fn label(&mut self, window: &str, label: String) {
        if let Some(context) = self.contexts.get_mut(window) {
            context.label = label;
        }
    }

    pub(crate) fn pending_submission(
        &self,
        lease: &Lease,
    ) -> Result<Option<(String, Submission)>, String> {
        let view = self.view(&lease.window).ok_or("Input context missing")?;
        if view.lease != *lease {
            return Err("Input context changed".into());
        }
        Ok(self
            .drafts
            .get(&lease.draft_key)
            .and_then(|d| d.pending.as_ref())
            .map(|p| (p.intent.clone(), p.request.clone())))
    }

    /// Observation disconnect does not revoke the original thread identity.
    /// SubmitThreadText connects/resumes that exact thread on explicit Send.
    pub(crate) fn invalidate_codex(&mut self) -> Result<Vec<GenericView>, String> {
        Ok(self.contexts.iter().filter(|(_,c)|matches!(c.destination,Destination::Codex(_)))
            .filter_map(|(w,_)|self.view(w)).collect())
    }

    pub(crate) fn needs_resolution(&self, lease:&Lease) -> Result<bool,String> {
        Ok(matches!(self.validate(lease)?.destination,Destination::Unavailable(_)))
    }

    pub(crate) fn active_views(&self) -> Vec<GenericView> {
        self.contexts
            .keys()
            .filter_map(|window| self.view(window))
            .collect()
    }

    pub(crate) fn clear_conversation_contexts(&mut self) {
        for context in self.contexts.values_mut(){context.conversation_context=None;}
    }
    pub(crate) fn conversation_context(&mut self,window:&str,context:Option<String>){
        if let Some(current)=self.contexts.get_mut(window){current.conversation_context=context;}
    }
    pub(crate) fn project(&mut self,window:&str,project:Option<String>){
        if let Some(context)=self.contexts.get_mut(window){context.project=project.filter(|p|p.starts_with('/')&&p.len()<=4096&&!p.chars().any(char::is_control));}
    }
    pub(crate) fn refined(&mut self,lease:&Lease,original:String,text:String,english:bool)->Result<GenericView,String>{
        self.record_refinement(lease,crate::learning::Refinement{original,refined:text.clone(),english,project:None},text)
    }
    pub(crate) fn record_refinement(&mut self,lease:&Lease,mut source:crate::learning::Refinement,text:String)->Result<GenericView,String>{
        let current=self.view(&lease.window).ok_or("Input window closed")?;
        if !current.lease.same_context(lease) || !(current.lease.draft_revision==lease.draft_revision || current.lease.draft_revision==lease.draft_revision.saturating_add(1)){return Err("Refine input context changed".into())}
        let context=self.contexts.get(&lease.window).ok_or("Input window closed")?;
        if [&source.original,&source.refined].iter().any(|s|s.is_empty()||s.len()>65536||s.contains('\0')) {return Err("Invalid Refine lineage".into())}
        source.project=context.project.clone();
        let draft=self.drafts.get_mut(&lease.draft_key).ok_or("Draft unavailable")?;
        if draft.text!=text{return Err("Refined draft changed; learning skipped".into())}
        if draft.refinement_revision!=Some(draft.revision) {
            if draft.refinement.len()>=32 || draft.refinement.iter().map(|r|r.original.len()+r.refined.len()).sum::<usize>()+source.original.len()+source.refined.len()>65536 {
                return Err("Learning source limit reached; inserted draft retained".into());
            }
            draft.refinement.push(source);draft.refinement_revision=Some(draft.revision);
        }
        self.view(&lease.window).ok_or("Input window closed".into())
    }
    pub(crate) fn learning_samples(&self)->Vec<crate::learning::LearningSample>{self.drafts.values().flat_map(|d|d.learn_pending.clone()).collect()}
    pub(crate) fn learned(&mut self,intent:&str){for draft in self.drafts.values_mut(){draft.learn_pending.retain(|s|s.intent!=intent);}}

    pub fn close_window(&mut self, window: &str) {
        self.contexts.remove(window);
    }
}



