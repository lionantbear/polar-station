//! Generic drafts are independent of transient window selection and delivery credentials.
use polar_station_session::{SendRequest, SessionReference};
use polar_tmux_protocol::{CodexBindingV1, CodexCommandV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    #[serde(default)]
    pending: Option<Pending>,
    text: String,
    revision: u64,
}

#[derive(Clone, Debug)]
struct Context {
    pane: String,
    key: String,
    revision: u64,
    destination: Destination,
    label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct GenericView {
    pub target_label: String,
    pub lease: Lease,
    pub text: String,
    pub unavailable: Option<String>,
    pub can_send: bool,
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
    #[serde(default)]
    dispatched: bool,
    #[serde(default)]
    error: Option<String>,
}

pub struct GenericDrafts {
    instance_id: String,
    contexts: BTreeMap<String, Context>,
    drafts: BTreeMap<String, Draft>,
    revision: u64,
}

impl Default for GenericDrafts {
    fn default() -> Self {
        Self {
            instance_id: polar_identity::generation::new_uuid_v4().to_string(),
            contexts: BTreeMap::new(),
            drafts: BTreeMap::new(),
            revision: 0,
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
        self.contexts.insert(
            window.clone(),
            Context {
                pane,
                key,
                revision: self.revision,
                destination,
                label,
            },
        );
        self.view(&window)
            .ok_or_else(|| "Input context missing".into())
    }

    pub fn view(&self, window: &str) -> Option<GenericView> {
        let context = self.contexts.get(window)?;
        let draft = self.drafts.get(&context.key)?;
        Some(GenericView {
            target_label: context.label.clone(),
            lease: Lease {
                instance_id: self.instance_id.clone(),
                window: window.into(),
                pane: context.pane.clone(),
                context_revision: context.revision,
                draft_key: context.key.clone(),
                draft_revision: draft.revision,
            },
            text: draft.text.clone(),
            can_send: draft.pending.is_none()
                && !draft.text.trim().is_empty()
                && match &context.destination {
                    Destination::Terminal(_) => {
                        polar_station_session::validate_terminal_text(&draft.text).is_ok()
                    }
                    Destination::Codex(_) => {
                        draft.text.len() <= polar_tmux_protocol::CODEX_INPUT_MAX_BYTES
                    }
                    Destination::Unavailable(_) => false,
                },
            unavailable: if draft.pending.is_some() {
                Some(draft.pending.as_ref().and_then(|p| p.error.clone()).unwrap_or_else(|| "Submission pending or unresolved; check receipt before retrying".into()))
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
        draft.pending = Some(Pending {
            intent,
            revision: lease.draft_revision,
            request,
            dispatched: false,
            error: None,
        });
        Ok(())
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
                pending.error = Some(format!("Delivery unconfirmed: {}. Check receipt before retrying.", error.chars().take(256).collect::<String>()));
            }
        }
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
            {
                return Err("Invalid saved input draft".into());
            }
        }
        // A process may have exited either side of the I/O boundary. Never
        // replay restored submissions, including ones persisted before dispatch.
        for draft in drafts.values_mut() {
            if let Some(pending) = &mut draft.pending {
                pending.dispatched = true;
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

    pub(crate) fn invalidate_codex(&mut self) -> Result<Vec<GenericView>, String> {
        let contexts = self
            .contexts
            .iter()
            .filter(|(_, c)| matches!(c.destination, Destination::Codex(_)))
            .map(|(w, c)| (w.clone(), c.pane.clone(), c.key.clone()))
            .collect::<Vec<_>>();
        contexts
            .into_iter()
            .map(|(w, p, k)| {
                self.observe(
                    w,
                    p,
                    k,
                    Destination::Unavailable(
                        "Codex connection changed; refresh original thread".into(),
                    ),
                )
            })
            .collect()
    }

    pub(crate) fn active_views(&self) -> Vec<GenericView> {
        self.contexts
            .keys()
            .filter_map(|window| self.view(window))
            .collect()
    }

    pub fn close_window(&mut self, window: &str) {
        self.contexts.remove(window);
    }
}
