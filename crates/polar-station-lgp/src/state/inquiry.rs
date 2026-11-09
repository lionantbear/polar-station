//! Transient Inquiry presentation in the existing composer. The SDK remains
//! the owner of graph traversal, answer validation and persistent sessions.
use super::*;
use polar_inquiry_graph_model::{
    InquiryGraphNodeKind as Kind, InquiryGraphSessionProjection as Projection,
};
use polar_station_session::routing::{form_metadata, TEXT_NODE, TARGET_NODE};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct Key {
    pub scope: Scope,
    pub sender: String,
    pub message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryAction {
    Show,
    Inbox,
    History,
    Older,
    Inspect,
    Dismiss,
    DismissAll,
    Restore,
    Open,
    Choose,
    Next,
    Submit,
    Later,
    Recommend,
    RetryView,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Open,
    Answer,
    Submit,
    Recommend,
    PrepareView,
    VerifyView,
}
#[derive(Default)]
pub(super) struct InquiryState {
    items: BTreeMap<Key, Item>,
    pub(super) dismissed: BTreeSet<Key>,
    windows: BTreeMap<String, WindowState>,
}
struct Item {
    title: String,
    preview: String,
    created: u64,
    projection: Option<Projection>,
    revision: u64,
    selected: String,
    pending: bool,
    done: bool,
    status: String,
    routing: Option<RoutingForm>,
}
struct RoutingForm {
    choices: Vec<polar_inquiry_graph_model::InquiryGraphChoice>,
    initial_text: String,
    submit_after_answer: bool,
    dispatch: Option<ViewDispatch>,
    view_failed: bool,
    approved_origin: Option<String>,
}
struct ViewDispatch {
    token: String,
    pane: String,
    text: String,
    choice: String,
    prepared: Value,
    native_acknowledged: bool,
    native_deadline_ms: Option<u64>,
}
#[derive(Default)]
struct WindowState {
    listing: bool,
    history: bool,
    foreground: bool,
    composing: bool,
    muted: bool,
    notified: bool,
    prompt: Option<Key>,
    active: Option<Key>,
    focus_pending: bool,
    cache: String,
}
const MAX_INVITATIONS: usize = 256;

pub(crate) fn offer(value: &Value) -> Option<(&Value, String)> {
    let body = value["payload"].get("value")?;
    if body["message_kind"] != "inquiry_graph.package.offer" {
        return None;
    }
    let title = body["title"]
        .as_str()
        .or_else(|| body["package"]["manifest"]["title"].as_str())
        .unwrap_or("Inquiry");
    let title = if title == "Send task" {
        body["package"]["metadata"]["station_session_dispatch"]["draft_text"].as_str().unwrap_or(title)
    } else { title };
    Some((body, crate::text::safe(title).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect()))
}
impl Key {
    fn disposition(&self) -> Self {
        let mut key=self.clone();
        key.scope.target_id=key.scope.node_id.clone();
        key
    }
}
impl InquiryState {
    fn history(&self, scope: &Scope) -> Vec<Key> {
        let mut keys:Vec<_>=self.items.iter().filter(|(k,i)| &k.scope==scope && (i.done || self.dismissed.contains(&k.disposition()))).map(|(k,_)|k.clone()).collect();
        keys.sort_by_key(|k| (std::cmp::Reverse(self.items[k].created),k.clone()));
        keys
    }

    fn token(&self, key: &Key) -> String {
        let item = &self.items[key];
        serde_json::to_string(&(
            key,
            item.revision,
            item.projection
                .as_ref()
                .and_then(|p| p.current_node.as_ref())
                .map(|n| n.node_id.as_str()),
        ))
        .unwrap_or_default()
    }
    fn active(&self, window: &str, scope: &Scope) -> Option<Key> {
        self.windows
            .get(window)?
            .active
            .as_ref()
            .filter(|k| &k.scope == scope)
            .cloned()
    }
    pub(super) fn saved_text(&self, window:&str, scope:&Scope) -> Option<&str> {
        let key=self.active(window,scope)?;let item=self.items.get(&key)?;
        if !item.projection.as_ref()?.can_submit {return None;}
        Some(item.routing.as_ref()?.initial_text.as_str())
    }
    fn pending(&self, scope: &Scope) -> Vec<Key> {
        let mut keys = self
            .items
            .iter()
            .filter(|(k, i)| &k.scope == scope && !i.done && !self.dismissed.contains(&k.disposition()))
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        keys.sort_by_key(|key| {
            (
                std::cmp::Reverse(self.items[key].created),
                key.sender.clone(),
                key.message.clone(),
            )
        });
        keys
    }
    fn supported(item: &Item) -> bool {
        item.projection
            .as_ref()
            .and_then(|p| p.current_node.as_ref())
            .is_none_or(|n| {
                !matches!(
                    n.kind,
                    Kind::MaxDiff | Kind::CardSort | Kind::IdentityCapture
                )
            })
    }
    pub(super) fn draft_context(&self, window: &str, scope: &Scope) -> Option<DraftContext> {
        let key = self.active(window, scope)?;
        let node = self
            .items
            .get(&key)?
            .projection
            .as_ref()?
            .current_node
            .as_ref();
        let routing = self.items.get(&key)?.routing.is_some();
        Some(DraftContext::Inquiry {
            scope: scope.clone(),
            sender: key.sender,
            message: key.message,
            node: if routing { TEXT_NODE.into() } else { node
                .map(|n| n.node_id.clone())
                .unwrap_or_else(|| "$result".into()) },
        })
    }
    pub(super) fn can_send(&self, window: &str, scope: &Scope, text: &str) -> Option<bool> {
        let key = self.active(window, scope)?;
        let item = &self.items[&key];
        let Some(p) = &item.projection else {
            return Some(false);
        };
        if let Some(form) = &item.routing {
            return Some(!item.pending && item.status.is_empty() && (p.can_submit ||
                (!text.trim().is_empty() && form.choices.iter().any(|c|c.choice_id == item.selected))));
        }
        Some(
            !item.pending
                && item.status.is_empty()
                && Self::supported(item)
                && (p.can_submit
                    || p.current_node.as_ref().is_some_and(|n| match n.kind {
                        Kind::ShortText | Kind::LongText => !text.trim().is_empty(),
                        Kind::Section | Kind::ResultReview => {
                            n.choices.is_empty() || !item.selected.is_empty()
                        }
                        _ => !item.selected.is_empty(),
                    })),
        )
    }
}
impl Engine {
    fn dismiss_invitation(&mut self, key: &Key) {
        self.inquiries.dismissed.insert(key.disposition());
        // Detach only local continuations. Already submitted server work finishes normally.
        self.pending.retain(|_,p| !matches!(&p.purpose,Purpose::Inquiry{key:pending,..} if pending.disposition()==key.disposition()));
        self.queued.retain(|(id,_)| self.pending.contains_key(id));
        for (other,item) in &mut self.inquiries.items {
            if other.disposition()==key.disposition() {
                item.pending=false;
                if let Some(form)=&mut item.routing {form.dispatch=None;form.submit_after_answer=false;}
            }
        }
        for state in self.inquiries.windows.values_mut() {
            if state.active.as_ref().or(state.prompt.as_ref()).is_some_and(|other|other.disposition()==key.disposition()) {
                state.active=None;state.prompt=None;state.listing=false;state.focus_pending=false;state.muted=true;state.cache.clear();
            }
        }
    }

    pub(super) fn inquiry_open_reference(
        &mut self,
        window: String,
        key: Key,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        self.inquiry_observe(&key.scope, false);
        if !self.inquiries.items.contains_key(&key) {
            return Ok(());
        }
        let history=self.inquiries.items[&key].done || self.inquiries.dismissed.contains(&key.disposition());
        let state = self.inquiries.windows.entry(window.clone()).or_default();
        state.history=history;
        state.active = (!history).then(||key.clone());
        state.prompt = history.then(||key.clone());
        state.listing = false;
        state.muted = false;
        state.notified = true;
        state.focus_pending = true;
        if history {return Ok(());}
        self.inquiry_request(&window, key, Operation::Open, json!({"kind":"open"}), out)
    }
    pub(super) fn inquiry_revoke(&mut self, scope: &Scope) {
        self.inquiries.items.retain(|key, _| &key.scope != scope);
        for state in self.inquiries.windows.values_mut() {
            if state.active.as_ref().is_some_and(|k| &k.scope == scope) {
                state.active = None;
            }
            if state.prompt.as_ref().is_some_and(|k| &k.scope == scope) {
                state.prompt = None;
            }
            state.focus_pending = false;
        }
    }
    pub(super) fn inquiry_close_window(&mut self, window: &str) {
        self.inquiries.windows.remove(window);
    }
    pub(super) fn inquiry_selected(&mut self, pane: &str) {
        let window = self
            .views
            .get(pane)
            .map(|v| v.window.clone())
            .or_else(|| self.ideas.get(pane).map(|v| v.window.clone()));
        if let Some(window) = window {
            if let Some(state) = self.inquiries.windows.get_mut(&window) {
                state.cache.clear();
                state.focus_pending = false;
            }
        }
    }
    pub(super) fn inquiry_observe(&mut self, scope: &Scope, live: bool) {
        let rows = self
            .threads
            .get(scope)
            .map(|t| {
                if live {
                    t.new_message
                        .as_ref()
                        .and_then(|key| t.transcript.messages.get(key))
                        .cloned()
                        .into_iter()
                        .collect::<Vec<_>>()
                } else {
                    t.transcript
                        .messages
                        .values()
                        .cloned()
                        .collect()
                }
            })
            .unwrap_or_default();
        for row in &rows {
            let Some((_, title)) = offer(row) else {
                continue;
            };
            let Ok((sender, message)) = polar_lgp_reader::message_key(row) else {
                continue;
            };
            let key = Key {
                scope: scope.clone(),
                sender,
                message,
            };
            if self.inquiries.items.contains_key(&key)
                || self.inquiries.items.len() >= MAX_INVITATIONS
            {
                continue;
            }
            self.inquiries.items.insert(
                key.clone(),
                Item {
                    title,
                    preview: row["payload"]["value"]["package"]["metadata"]["station_session_dispatch"]["draft_text"].as_str().unwrap_or("").to_owned(),
                    created: row["created_at"].as_u64().unwrap_or(0),
                    projection: None,
                    revision: 0,
                    selected: String::new(),
                    pending: false,
                    done: false,
                    status: String::new(),
                    routing: None,
                },
            );
            if live && !self.inquiries.dismissed.contains(&key.disposition()) {
                for (window, pane) in &self.selected {
                    if self.views.get(pane).is_none_or(|v| &v.scope != scope) {
                        continue;
                    }
                    let state = self.inquiries.windows.entry(window.clone()).or_default();
                    if state.foreground
                        && !state.muted
                        && !state.notified
                        && state.active.is_none()
                        && state.prompt.is_none()
                    {
                        state.prompt = Some(key.clone());
                        state.focus_pending = true;
                        state.notified = true;
                    }
                }
            }
        }
        // Resolve from canonical messages, including replayed snapshots. A package
        // ID is reusable; only the original sender/message identifies this offer.
        for row in &rows {
            if row["payload"]["value"]["message_kind"] != "inquiry_graph.session.result"
                || row["payload"]["value"]["result"]["session_id"].as_str().is_none() { continue; }
            let Some(sender) = row["causal_sender_id"].as_str() else { continue; };
            let Some(message) = row["causal_message_id"].as_str() else { continue; };
            let key = Key { scope:scope.clone(), sender:sender.into(), message:message.into() };
            if let Some(item) = self.inquiries.items.get_mut(&key) {
                item.done = true;
                item.pending = false;
                item.status = "Response submitted".into();
            }
            for state in self.inquiries.windows.values_mut() {
                if state.active.as_ref() == Some(&key) { state.active = None; state.cache.clear(); }
                if state.prompt.as_ref() == Some(&key) { state.prompt = None; state.focus_pending = false; state.cache.clear(); }
            }
        }
    }
    pub(super) fn inquiry_view_current(&self, key:&Key, token:Option<&str>) -> bool {
        !self.inquiries.dismissed.contains(&key.disposition()) && token.is_none_or(|token| self.inquiries.items.get(key).and_then(|i|i.routing.as_ref()).and_then(|r|r.dispatch.as_ref()).is_some_and(|d|d.token==token))
    }
    pub(super) fn inquiry_expire_native(&mut self, now:u64) {
        let expired=self.inquiries.items.iter().filter_map(|(key,item)|item.routing.as_ref()?.dispatch.as_ref()?.native_deadline_ms.filter(|deadline|*deadline<=now).map(|_|key.clone())).collect::<Vec<_>>();
        for key in expired {self.inquiry_failed(&key,"The target terminal did not become ready. Task not sent.");}
    }
    pub(super) fn inquiry_cancel_source(&mut self, pane:&str) {
        for item in self.inquiries.items.values_mut() {
            if let Some(form)=item.routing.as_mut() {
                if form.approved_origin.as_deref()==Some(pane) || form.dispatch.as_ref().is_some_and(|d|d.pane==pane) {
                    form.dispatch=None;form.approved_origin=None;form.submit_after_answer=false;
                    item.pending=false;item.status="The originating pane closed. Task not sent.".into();
                }
            }
        }
    }
    pub(super) fn inquiry_disconnect_views(&mut self) {
        for item in self.inquiries.items.values_mut() {
            if let Some(form)=item.routing.as_mut() {
                if form.dispatch.take().is_some() {
                    form.view_failed=true;item.pending=false;item.status="Connection lost. Task not sent.".into();
                }
            }
        }
    }
    pub(super) fn inquiry_environment(
        &mut self,
        window: String,
        foreground: bool,
        composing: bool,
    ) {
        let state = self.inquiries.windows.entry(window).or_default();
        state.foreground = foreground;
        state.composing = composing;
        if !foreground {
            state.focus_pending = false;
        }
    }
    pub(super) fn inquiry_request(
        &mut self,
        window: &str,
        key: Key,
        operation: Operation,
        action: Value,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let lease = self
            .threads
            .get(&key.scope)
            .and_then(|t| t.lease.clone())
            .filter(|_| self.ready)
            .ok_or("Inquiry is disconnected")?;
        if self.inquiries.items.get(&key).is_some_and(|i| i.pending) {
            return Ok(());
        }
        self.request(
            &key.scope.target_id,
            json!({"op":"inquiry","expected_node_id":key.scope.node_id,"lease_id":lease,
            "offer_key":{"sender_id":key.sender,"message_id":key.message},"action":action}),
            Purpose::Inquiry {
                window: window.into(),
                key: key.clone(),
                operation,
                view_token: if matches!(operation,Operation::PrepareView|Operation::VerifyView) { self.inquiries.items.get(&key).and_then(|i|i.routing.as_ref()).and_then(|r|r.dispatch.as_ref()).map(|d|d.token.clone()) } else {None},
            },
            out,
        )?;
        if let Some(item) = self.inquiries.items.get_mut(&key) {
            item.pending = true;
            item.status = "Loading…".into();
        }
        Ok(())
    }
    pub(super) fn inquiry_action(
        &mut self,
        window: String,
        action: InquiryAction,
        token: Option<String>,
        value: Option<String>,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let (_, scope) = self.composer_scope(&window)?;
        let state = self.inquiries.windows.entry(window.clone()).or_default();
        let current = state
            .active
            .as_ref()
            .or(state.prompt.as_ref())
            .filter(|k| k.scope == scope)
            .cloned();
        if action == InquiryAction::Older {
            let pane=self.selected.get(&window).cloned().ok_or("No selected conversation")?;
            return self.load_older(&pane,out);
        }
        if action == InquiryAction::DismissAll {
            for key in self.inquiries.pending(&scope) { self.dismiss_invitation(&key); }
            for state in self.inquiries.windows.values_mut() {
                if state.active.as_ref().or(state.prompt.as_ref()).is_some_and(|k| k.scope==scope) {
                    state.active=None;state.prompt=None;state.listing=false;state.focus_pending=false;state.cache.clear();
                }
            }
            self.composer_cache.clear();
            out.push(Effect::Drafts{value:self.export_drafts()});
            return Ok(());
        }
        if matches!(action, InquiryAction::Show | InquiryAction::Inbox | InquiryAction::History) {
            let history=action==InquiryAction::History || (self.inquiries.pending(&scope).is_empty() && !self.inquiries.history(&scope).is_empty());
            let keys = if history {self.inquiries.history(&scope)}else{self.inquiries.pending(&scope)};
            let state = self.inquiries.windows.get_mut(&window).unwrap();
            state.muted = false;
            state.focus_pending = true;
            state.notified = true;
            state.history=history;
            state.listing = action==InquiryAction::Inbox || history || keys.len() > 1;
            state.active = None;
            state.prompt = keys.first().cloned();
            return Ok(());
        }
        if action==InquiryAction::Inspect {
            let history=self.inquiries.windows[&window].history;
            let keys=if history {self.inquiries.history(&scope)}else{self.inquiries.pending(&scope)};
            if let Some(key)=keys.into_iter().find(|k|Some(self.inquiries.token(k))==value) {
                let state=self.inquiries.windows.get_mut(&window).unwrap();
                state.listing=false;state.prompt=Some(key.clone());state.active=None;state.focus_pending=false;
                if !history {
                    state.active=Some(key.clone());state.prompt=None;
                    return self.inquiry_request(&window,key,Operation::Open,json!({"kind":"open"}),out);
                }
            }
            return Ok(());
        }
        if action == InquiryAction::Choose && self.inquiries.windows[&window].listing {
            let history=self.inquiries.windows[&window].history;
            let keys=if history {self.inquiries.history(&scope)} else {self.inquiries.pending(&scope)};
            if let Some(key) = keys.into_iter()
                .find(|k| Some(self.inquiries.token(k)) == value)
            {
                let state = self.inquiries.windows.get_mut(&window).unwrap();
                state.listing = false;
                if history {
                    state.prompt=Some(key);state.active=None;state.focus_pending=false;
                    return Ok(());
                }
                state.prompt = None;
                state.active = Some(key.clone());
                state.focus_pending = true;
                return self.inquiry_request(
                    &window,
                    key,
                    Operation::Open,
                    json!({"kind":"open"}),
                    out,
                );
            }
            return Ok(());
        }
        let Some(key) = current else { return Ok(()) };
        if token.as_deref() != Some(self.inquiries.token(&key).as_str()) {
            return Ok(());
        }
        if matches!(action, InquiryAction::Dismiss | InquiryAction::Restore) {
            if action==InquiryAction::Dismiss { self.dismiss_invitation(&key); }
            else { self.inquiries.dismissed.remove(&key.disposition()); }
            for state in self.inquiries.windows.values_mut() {
                if state.active.as_ref().or(state.prompt.as_ref()).is_some_and(|other|other.disposition()==key.disposition()) {
                    state.active=None;state.prompt=None;state.listing=false;state.focus_pending=false;state.muted=true;state.cache.clear();
                }
            }
            self.composer_cache.clear();
            out.push(Effect::Drafts{value:self.export_drafts()});
            return Ok(());
        }
        if action == InquiryAction::Recommend {
            let item=&self.inquiries.items[&key];
            if item.routing.is_none() || item.pending || item.done {return Ok(());}
            let text=self.inquiries.draft_context(&window,&scope).and_then(|c|self.drafts.get(&(window.clone(),c))).map(|d|d.text.clone()).unwrap_or_default();
            if text.trim().is_empty() {return Ok(());}
            return self.inquiry_request(&window,key,Operation::Recommend,json!({"kind":"recommend","text":text,
                "client_intent_id":polar_identity::generation::new_uuid_v7().to_string(),"created_at_secs":time::OffsetDateTime::now_utc().unix_timestamp().max(1)}),out);
        }
        if action == InquiryAction::Later {
            let state = self.inquiries.windows.get_mut(&window).unwrap();
            state.active = None;
            state.listing = false;
            state.prompt = None;
            state.muted = true;
            state.focus_pending = false;
            self.composer_cache.remove(&window);
            out.push(Effect::FocusComposer {
                window_id: window.clone(),
                pane_id: self.selected[&window].clone(),
            });
            return Ok(());
        }
        if action == InquiryAction::Open {
            let state = self.inquiries.windows.get_mut(&window).unwrap();
            state.active = Some(key.clone());
            state.prompt = None;
            state.focus_pending = true;
            return self.inquiry_request(
                &window,
                key,
                Operation::Open,
                json!({"kind":"open"}),
                out,
            );
        }
        if action == InquiryAction::RetryView {
            if let Some(item)=self.inquiries.items.get_mut(&key) {
                if !item.pending && item.routing.as_ref().is_some_and(|r|r.view_failed) {
                    item.status.clear();item.routing.as_mut().unwrap().view_failed=false;
                    return self.inquiry_send(&window,out);
                }
            }
            return Ok(());
        }
        if action == InquiryAction::Choose {
            let item = self.inquiries.items.get_mut(&key).unwrap();
            if !item.pending
                && (item.routing.as_ref().is_some_and(|r| r.choices.iter().any(|c|Some(&c.choice_id) == value.as_ref())) || item
                    .projection
                    .as_ref()
                    .and_then(|p| p.current_node.as_ref())
                    .is_some_and(|n| {
                        n.choices
                            .iter()
                            .any(|c| Some(&c.choice_id) == value.as_ref())
                    }))
            {
                item.selected = value.unwrap_or_default();
            }
            return Ok(());
        }
        self.inquiry_send(&window, out)
    }
    pub(super) fn inquiry_send(
        &mut self,
        window: &str,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let (_, scope) = self.composer_scope(window)?;
        let key = self
            .inquiries
            .active(window, &scope)
            .ok_or("Open an Inquiry first")?;
        let item = &self.inquiries.items[&key];
        if item.pending || !item.status.is_empty() || !InquiryState::supported(item) {
            return Ok(());
        }
        let Some(p) = &item.projection else {
            return Ok(());
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp().max(1);
        if p.can_submit && item.routing.is_none() {
            return self.inquiry_request(window,key.clone(),Operation::Submit,json!({"kind":"submit","expected_revision":item.revision,
                "client_intent_id":polar_identity::generation::new_uuid_v7().to_string(),"created_at_secs":now}),out);
        }
        let node = p.current_node.as_ref();
        if node.is_none() && item.routing.is_none() { return Err("Inquiry has no current question".into()); }
        let text = self
            .inquiries
            .draft_context(window, &scope)
            .and_then(|c| self.drafts.get(&(window.into(), c)))
            .map(|d| d.text.clone())
            .unwrap_or_default();
        if self.inquiries.can_send(window, &scope, &text) != Some(true) {
            return Ok(());
        }
        let text_node = node.is_some_and(|node|matches!(node.kind, Kind::ShortText | Kind::LongText));
        let kind = if text_node {
            "text"
        } else if node.is_some_and(|node|node.kind == Kind::Consent) {
            "consent"
        } else if node.is_some_and(|node|node.kind == Kind::ResultReview) {
            "review_decision"
        } else {
            "choice"
        };
        let choices = if item.selected.is_empty() {
            vec![]
        } else {
            vec![item.selected.clone()]
        };
        if let Some(form) = &item.routing {
            let pane = self.composer_scope(window)?.0;
            let choice = item.selected.clone();
            let text=if p.can_submit {form.initial_text.clone()} else {text};
            self.inquiries.items.get_mut(&key).unwrap().routing.as_mut().unwrap().dispatch = Some(ViewDispatch {
                token:polar_identity::generation::new_uuid_v7().to_string(), pane, text, choice:choice.clone(), prepared:Value::Null, native_acknowledged:false, native_deadline_ms:None,
            });
            return self.inquiry_request(window,key,Operation::PrepareView,json!({"kind":"prepare_view","choice_id":choice}),out);
        }
        let node=node.ok_or("Inquiry has no current question")?;
        let answer = json!({"node_id":node.node_id,"answer_kind":kind,"choice_ids":choices,"text":if text_node{Some(text)}else{None},"answered_at_secs":now});
        self.inquiry_request(window,key.clone(),Operation::Answer,json!({"kind":"answer","expected_revision":item.revision,
            "client_operation_id":polar_identity::generation::new_uuid_v7().to_string(),"answers":[answer]}),out)
    }
    pub(super) fn inquiry_terminal_result(&mut self, window:&str, token:&str, ready:bool, error:Option<String>, out:&mut Vec<Effect>) {
        let key = self.inquiries.items.iter().find_map(|(key,item)| item.routing.as_ref()?.dispatch.as_ref()
            .filter(|d|d.token==token && self.views.get(&d.pane).is_some_and(|v|v.window==window && v.scope==key.scope))
            .map(|_|key.clone()));
        let Some(key)=key else { return; };
        let item=self.inquiries.items.get_mut(&key).unwrap();
        if !item.pending || item.done { return; }
        if item.routing.as_ref().unwrap().dispatch.as_ref().is_none_or(|d|d.prepared.is_null() || d.native_acknowledged) {return;}
        if !ready {
            let _ = error;
            self.inquiry_failed(&key,"The target terminal could not be opened. Task not sent.");
            return;
        }
        let dispatch=item.routing.as_mut().unwrap().dispatch.as_mut().unwrap();
        dispatch.native_acknowledged=true;dispatch.native_deadline_ms=None;
        let choice=dispatch.choice.clone();
        item.pending=false;item.status.clear();
        if let Err(error)=self.inquiry_request(window,key.clone(),Operation::VerifyView,json!({"kind":"prepare_view","choice_id":choice,"activate":true}),out) {self.inquiry_failed(&key,&error);}
    }
    
    pub(super) fn inquiry_response_effects(
        &mut self,
        window: &str,
        key: Key,
        operation: Operation,
        value: Value,
        out: &mut Vec<Effect>,
    ) {
        if matches!(operation,Operation::PrepareView|Operation::VerifyView) {
            let Some(item)=self.inquiries.items.get_mut(&key).filter(|i|!i.done) else {return;};
            let Some(form)=item.routing.as_mut() else {return;};
            let Some(dispatch)=form.dispatch.as_mut() else {return;};
            if !value["client_view"].is_object() {
                self.inquiry_failed(&key,"This client cannot open the target terminal. Task not sent.");return;
            }
            if operation==Operation::PrepareView {
                if !form.choices.iter().any(|choice|choice.choice_id==dispatch.choice && choice.value==value["reference"]) {
                    self.inquiry_failed(&key,"The opened target differs from the chosen session. Task not sent.");return;
                }
                dispatch.prepared=value.clone();dispatch.native_deadline_ms=Some(self.now_ms.saturating_add(30_000));
                item.pending=true;item.status="Opening target terminal…".into();
                out.push(Effect::EnsureTerminalVisible{window_id:window.into(),pane_id:dispatch.pane.clone(),token:dispatch.token.clone(),target:value});
                return;
            }
            if dispatch.prepared["client_view"]!=value["client_view"] || dispatch.prepared["reference"]!=value["reference"] {
                self.inquiry_failed(&key,"The target changed while opening. Task not sent.");return;
            }
            let dispatch=form.dispatch.take().unwrap();
            form.approved_origin=Some(dispatch.pane.clone());
            if item.projection.as_ref().is_some_and(|p|p.can_submit) {
                item.pending=false;item.status.clear();let revision=item.revision;
                if let Err(error)=self.inquiry_request(window,key.clone(),Operation::Submit,json!({"kind":"submit","expected_revision":revision,
                    "client_intent_id":polar_identity::generation::new_uuid_v7().to_string(),"created_at_secs":time::OffsetDateTime::now_utc().unix_timestamp().max(1)}),out) {self.inquiry_failed(&key,&error);}
                return;
            }
            form.submit_after_answer=true;item.pending=false;item.status.clear();
            let revision=item.revision;
            let now=time::OffsetDateTime::now_utc().unix_timestamp().max(1);
            let answers=json!([
                {"node_id":TEXT_NODE,"answer_kind":"text","text":dispatch.text,"choice_ids":[],"answered_at_secs":now},
                {"node_id":TARGET_NODE,"answer_kind":"choice","choice_ids":[dispatch.choice],"answered_at_secs":now}]);
            if let Err(error)=self.inquiry_request(window,key.clone(),Operation::Answer,json!({"kind":"answer","expected_revision":revision,
                "client_operation_id":polar_identity::generation::new_uuid_v7().to_string(),"answers":answers}),out) {self.inquiry_failed(&key,&error);}
            return;
        }
        let mut continue_submit = None;
        let mut focus_origin = None;
        if let Some(item) = self.inquiries.items.get_mut(&key) {
            item.pending = false;
            item.status = String::new();
            if matches!(operation,Operation::Submit|Operation::Recommend) {
                if operation==Operation::Submit {focus_origin=item.routing.as_mut().and_then(|form|form.approved_origin.take());}
                item.done = true;
                item.status = "Response submitted".into();
            } else {
                item.revision = value["revision"].as_u64().unwrap_or(item.revision);
                item.projection = serde_json::from_value(value["projection"].clone()).ok();
                item.selected = String::new();
                if item.projection.is_none() {
                    item.status = "Invalid Inquiry projection".into();
                }
                if !InquiryState::supported(item) {
                    item.status = "This Inquiry question type is not supported yet".into();
                }
                if operation == Operation::Open {
                    if let Some(metadata) = form_metadata(&value["package_metadata"]) {
                        let batch: Vec<polar_inquiry_graph_model::InquiryGraphNode> = serde_json::from_value(value["batch"].clone()).unwrap_or_default();
                        let nodes = if batch.len()==2 {batch} else {
                            self.threads.get(&key.scope).and_then(|t|t.transcript.messages.get(&(key.sender.clone(),key.message.clone())))
                                .and_then(|row|serde_json::from_value::<Vec<polar_inquiry_graph_model::InquiryGraphNode>>(row["payload"]["value"]["package"]["nodes"].clone()).ok()).unwrap_or_default()
                        };
                        if metadata["context"]["owner"] == key.scope.node_id && metadata["context"]["thread"] == key.scope.thread
                            && nodes.len() == 2 && nodes[0].node_id == TEXT_NODE && nodes[1].node_id == TARGET_NODE {
                            item.routing = Some(RoutingForm { choices:nodes[1].choices.clone(), initial_text:metadata["draft_text"].as_str().unwrap_or("").into(), submit_after_answer:false, dispatch:None, view_failed:false, approved_origin:None });
                        }
                    }
                    if let Some(form)=item.routing.as_mut() {
                        if let Some(answers)=value["session"]["answers"].as_array() {
                            if let Some(text)=answers.iter().rev().find(|a|a["node_id"]==TEXT_NODE).and_then(|a|a["text"].as_str()) {form.initial_text=text.into();}
                            if let Some(choice)=answers.iter().rev().find(|a|a["node_id"]==TARGET_NODE).and_then(|a|a["choice_ids"][0].as_str()) {item.selected=choice.into();}
                        }
                    }
                }
                if operation == Operation::Answer && item.projection.as_ref().is_some_and(|p|p.can_submit)
                    && item.routing.as_mut().is_some_and(|r|std::mem::take(&mut r.submit_after_answer)) {
                    continue_submit = Some(item.revision);
                }
                if value["submitted"] == true {
                    item.done = true;
                }
            }
        }
        if let Some(pane)=focus_origin.filter(|pane|self.selected.get(window)==Some(pane) && self.views.get(pane).is_some_and(|view|view.window==window && view.scope==key.scope)) {
            out.push(Effect::FocusComposer{window_id:window.into(),pane_id:pane});
        }
        if matches!(operation,Operation::Submit|Operation::Recommend) || self.inquiries.items.get(&key).is_some_and(|i| i.done)
        {
            let state = self.inquiries.windows.entry(window.into()).or_default();
            if state.active.as_ref() == Some(&key) {
                state.active = None;
                state.prompt = None;
            }
            if self.inquiries.pending(&key.scope).is_empty() {
                let state = self.inquiries.windows.get_mut(window).unwrap();
                state.notified = false;
            }
            self.composer_cache.remove(window);
        }
        if self
            .selected
            .get(window)
            .and_then(|p| self.views.get(p))
            .is_some_and(|v| v.scope == key.scope)
        {
            let state = self.inquiries.windows.entry(window.into()).or_default();
            if state.active.as_ref() == Some(&key) {
                state.focus_pending = true;
            }
        }
        if operation == Operation::Recommend {
            let state=self.inquiries.windows.entry(window.into()).or_default();
            state.notified=false;state.muted=false;
        }
        if let Some(context) = self.inquiries.draft_context(window, &key.scope) {
            let initial = self.inquiries.items[&key].routing.as_ref().map(|r|r.initial_text.clone());
            if let std::collections::btree_map::Entry::Vacant(entry) = self.drafts.entry((window.into(), context)) {
                let mut draft = Draft::default();
                if let Some(text) = initial { draft.text = text; }
                entry.insert(draft);
            }
        }
        if let Some(revision) = continue_submit {
            let mut out = Vec::new();
            if let Err(error) = self.inquiry_request(window,key.clone(),Operation::Submit,json!({"kind":"submit","expected_revision":revision,
                "client_intent_id":polar_identity::generation::new_uuid_v7().to_string(),"created_at_secs":time::OffsetDateTime::now_utc().unix_timestamp().max(1)}),&mut out) {
                self.inquiry_failed(&key,&error);
            }
        }
    }
    pub(super) fn inquiry_failed(&mut self, key: &Key, message: &str) {
        if let Some(item) = self.inquiries.items.get_mut(key) {
            item.pending = false;
            if let Some(form)=item.routing.as_mut() { form.view_failed=form.dispatch.take().is_some(); }
            item.status = format!("{message} — reopen to refresh; no automatic retry");
            if let Some(routing) = &mut item.routing { routing.submit_after_answer = false; }
        }
    }
    pub(super) fn inquiry_effects(&mut self, out: &mut Vec<Effect>) {
        for (window, pane) in self.selected.clone() {
            let Some(scope) = self.views.get(&pane).map(|v| v.scope.clone()) else {
                continue;
            };
            let pending = self.inquiries.pending(&scope);
            let history_keys = self.inquiries.history(&scope);
            let state = self.inquiries.windows.entry(window.clone()).or_default();
            let key = state
                .active
                .as_ref()
                .or(state.prompt.as_ref())
                .filter(|k| k.scope == scope)
                .cloned();
            let active = state.active.as_ref().is_some_and(|k| k.scope == scope);
            let stage = if state.listing && state.foreground {
                "list"
            } else if active {
                "question"
            } else if key.is_some() && state.foreground {
                "prompt"
            } else {
                "badge"
            };
            let focus =
                state.focus_pending && state.foreground && !state.composing && key.is_some();
            if focus {
                state.focus_pending = false;
            }
            let mut view = json!({"stage":stage,"active":active,"count":pending.len(),"title":"Inquiry","token":"","status":"","choices":[],"primary_label":"Next","can_next":false,"can_submit":false,"editable":false,"open_label":"Open","later_label":"Later","entry_label":"Inquiry","prompt":"Open this Inquiry?"});
            view["entry_title"] = json!("Inquiries");
            view["has_history"]=json!(!history_keys.is_empty());
            let history=state.history;
            view["history"]=json!(history);
            let entries=if history {&history_keys}else{&pending};
            view["invitations"]=json!(entries.iter().map(|k| {
                let item=&self.inquiries.items[k];
                let time=chrono::DateTime::from_timestamp(item.created as i64,0).map(|t|t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
                json!({"token":self.inquiries.token(k),"title":item.title,"preview":item.preview,"time":time,"source":k.scope.thread,
                    "status":if item.done {"Submitted"}else if history {"Dismissed"}else{"Pending"}})
            }).collect::<Vec<_>>());
            view["others_label"] = json!("");
            view["inbox_label"] = json!("All Inquiries");
            if let Some(key) = key {
                let item = &self.inquiries.items[&key];
                view["title"] = json!(item.title);
                view["task_preview"] = json!(item.preview);
                view["prompt"] = json!(item.preview);
                view["dismissed"]=json!(self.inquiries.dismissed.contains(&key.disposition()));
                view["can_restore"]=json!(!item.done && self.inquiries.dismissed.contains(&key.disposition()));
                let source=self.thread_titles.get(&(scope.target_id.clone(),scope.node_id.clone(),scope.thread.clone())).map(|(_,title)|title.as_str()).unwrap_or("Conversation");
                view["source"]=json!(source);
                view["details"]=json!(format!("{} · {} · {}",source,
                    chrono::DateTime::from_timestamp(item.created as i64,0).map(|t|t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default(),
                    if item.done {"Submitted"}else if history {"Dismissed"}else{"Pending"}));
                view["created_at"]=json!(item.created);
                view["token"] = json!(self.inquiries.token(&key));
                view["status"] = json!(item.status);
                if let Some(p) = item.projection.as_ref().filter(|_| !history) {
                    view["can_submit"] = json!(p.can_submit && !item.pending);
                    view["primary_label"] = json!(if p.can_submit {
                        "Submit response"
                    } else {
                        "Next"
                    });
                    if let Some(node) = &p.current_node {
                        view["question"] = json!(node.prompt.as_ref().unwrap_or(&node.title));
                        view["editable"] = json!(
                            matches!(node.kind, Kind::ShortText | Kind::LongText) && !item.pending
                        );
                        view["choices"]=json!(node.choices.iter().take(64).enumerate().map(|(i,c)|json!({"value":c.choice_id,"label":c.label,"number":i+1,"selected":item.selected==c.choice_id,"enabled":!item.pending&&InquiryState::supported(item)})).collect::<Vec<_>>());
                    }
                    if let Some(form) = &item.routing {
                        view["routing"] = json!(true);
                        view["recommend_label"] = json!("Other Sessions…");
                        view["can_recommend"] = json!(!item.pending && !item.done);
                        view["retry_view_label"] = json!("Retry opening");
                        view["can_retry_view"] = json!(form.view_failed && !item.pending);
                        view["question"] = json!("Choose a Session");
                        view["task_label"] = json!("Task to send");
                        view["editable"] = json!(!item.pending && !p.can_submit);
                        view["primary_label"] = json!("Send task");
                        view["delivery_hint"] = json!("Opens the selected pane before sending. Nothing is sent until you confirm.");
                        view["destination"] = json!(form.choices.iter().find(|c|c.choice_id==item.selected)
                            .map(|c|format!("Send to: {} · {}",c.label,c.metadata["host"].as_str().unwrap_or("Host")))
                            .unwrap_or_else(||"Select a Session to send this task.".into()));
                        view["choices"] = json!(form.choices.iter().enumerate().map(|(i,c)|candidate_view(c,i,item)).collect::<Vec<_>>());
                    }
                    let text = self
                        .inquiries
                        .draft_context(&window, &scope)
                        .and_then(|c| self.drafts.get(&(window.clone(), c)))
                        .map(|d| d.text.as_str())
                        .unwrap_or("");
                    if let Some(form)=&item.routing { view["task_preview"] = json!(if item.projection.as_ref().is_some_and(|p|p.can_submit) {form.initial_text.as_str()}else{text}); }
                    view["can_next"] =
                        json!(self.inquiries.can_send(&window, &scope, text) == Some(true));
                }
            }
            if stage == "list" {
                view["routing"]=json!(false);view["editable"]=json!(false);view["task_preview"]=json!("");
                view["title"] = json!(if history {"Inquiry history"}else{"Pending Inquiries"});
                view["details"] = json!("Respond only when useful. Dismiss keeps an Inquiry in history.");
                view["question"] = json!("");
                let rows=if self.inquiries.windows[&window].history {self.inquiries.history(&scope)}else{pending.clone()};
                view["choices"]=json!(rows.iter().enumerate().map(|(i,key)|json!({"value":self.inquiries.token(key),"label":self.inquiries.items[key].title,"context":self.inquiries.items[key].preview,"detail":format!("{} · {}",if self.inquiries.items[key].done {"Submitted"}else if history {"Dismissed"}else{"Pending"},chrono::DateTime::from_timestamp(self.inquiries.items[key].created as i64,0).map(|t|t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()),"number":i+1,"selected":false,"enabled":true})).collect::<Vec<_>>());
            }
            let encoded = serde_json::to_string(&view).unwrap_or_default();
            let state = self.inquiries.windows.get_mut(&window).unwrap();
            if state.cache != encoded || focus {
                state.cache = encoded;
                out.push(Effect::Inquiry {
                    window_id: window,
                    pane_id: pane,
                    view,
                    focus,
                });
            }
        }
    }
}

fn candidate_view(c:&polar_inquiry_graph_model::InquiryGraphChoice,index:usize,item:&Item)->Value {
    let metadata=&c.metadata;
    let kind=match metadata["agent_type"].as_str().unwrap_or("terminal") {
        "codex"|"codex_plane"=>"Codex", "claude"|"claude_code"=>"Claude", "shell"=>"Shell",
        "adam"=>"Adam", "pi"=>"Pi", "terminal"=>"Terminal", _=>"Agent",
    };
    let host=metadata["host"].as_str().unwrap_or("Host");
    let state=metadata["status"].as_str().unwrap_or("").trim();
    let subtitle=[kind,host,state].into_iter().filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" · ");
    let preview=metadata["preview"].as_str().unwrap_or("").trim();
    let mut context=Vec::new();
    for text in std::iter::once(preview).chain(metadata["summary"].as_array().into_iter().flatten().filter_map(Value::as_str)) {
        let text=text.trim();
        if !text.is_empty() && text!=c.label && !context.contains(&text) {context.push(text);}
    }
    let reason=metadata["reason"].as_str().unwrap_or("").trim();
    json!({"value":c.choice_id,"label":c.label,"number":index+1,"selected":item.selected==c.choice_id,
        "subtitle":subtitle,"directory":metadata["directory"].as_str().unwrap_or(""),"context":context.join("\n"),
        "reason":reason,"reason_text":if reason.is_empty(){"Recommendation reason not provided.".into()}else{format!("Why suggested: {reason}")},
        "enabled":!item.pending})
}


