//! Idea context in the existing Station composer; no Thread or network authority.
use super::*;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DraftContext {
    Thread(Scope),
    Idea(String),
    Inquiry {scope:Scope,sender:String,message:String,node:String},
}
impl From<Scope> for DraftContext {
    fn from(scope: Scope) -> Self {
        Self::Thread(scope)
    }
}
pub(super) struct IdeaView {
    pub window: String,
    pub identity: String,
    pub state: ControlsState,
}
impl Engine {
    pub(super) fn open_idea(
        &mut self,
        pane: String,
        window: String,
        identity: String,
    ) -> Result<(), String> {
        for id in [&pane, &window, &identity] {
            validate_id(id)?;
        }
        if self.views.contains_key(&pane)
            || self
                .ideas
                .get(&pane)
                .is_some_and(|v| v.identity != identity)
        {
            return Err("changing content identity requires a new Pane binding".into());
        }
        if self.ideas.len() >= 64 && !self.ideas.contains_key(&pane) {
            return Err("too many Idea Panes".into());
        }
        if self
            .ideas
            .iter()
            .any(|(id, v)| id != &pane && v.identity == identity)
        {
            return Err("Idea view is already mounted".into());
        }
        self.drafts
            .entry((String::new(), DraftContext::Idea(identity.clone())))
            .or_default();
        self.ideas
            .entry(pane)
            .and_modify(|v| v.window = window.clone())
            .or_insert(IdeaView {
                window,
                identity,
                state: ControlsState::default(),
            });
        Ok(())
    }
    pub(super) fn draft_key(&self, window: &str) -> Result<(String, DraftContext), String> {
        let pane = self
            .selected
            .get(window)
            .ok_or("select a Pane before composing")?;
        if let Some(view) = self.ideas.get(pane) {
            return Ok((String::new(), DraftContext::Idea(view.identity.clone())));
        }
        let (_, scope) = self.composer_scope(window)?;
        if let Some(context)=self.inquiries.draft_context(window,&scope){return Ok((window.into(),context));}
        Ok((window.into(), scope.into()))
    }
    pub(super) fn idea_action(
        &self,
        window: &str,
        action: IdeaAction,
        query: String,
        out: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let pane = self.selected.get(window).ok_or("select an Idea Tree")?;
        let view = self
            .ideas
            .get(pane)
            .ok_or("selected Pane is not an Idea Tree")?;
        if query.len() > 4096 {
            return Err("search exceeds 4 KiB".into());
        }
        if action == IdeaAction::LoadRelated && !view.state.related_sources.iter().any(|source| source.path == query) {
            return Err("related source is not in the current view".into());
        }
        if matches!(action, IdeaAction::SelectBrowser | IdeaAction::SelectTheme | IdeaAction::ExportViewPackage | IdeaAction::UninstallViewPackage)
            && !view.state.controls().iter().any(|control| control.action == action && control.query == query && control.enabled) {
            return Err("view package action is not in the current catalog".into());
        }
        if !view.state.permits(action) {
            return Err("Idea action is not available in the current view".into());
        }
        out.push(Effect::IdeaCommand {
            pane_id: pane.clone(),
            view_id: view.identity.clone(),
            action,
            query,
        });
        Ok(())
    }
    pub(super) fn idea_composer_effects(&mut self, out: &mut Vec<Effect>) {
        for (window, pane) in &self.selected {
            let Some(view) = self.ideas.get(pane) else {
                continue;
            };
            let Some(draft) = self
                .drafts
                .get(&(String::new(), DraftContext::Idea(view.identity.clone())))
            else {
                continue;
            };
            let effect = Effect::IdeaComposer {
                window_id: window.clone(),
                pane_id: pane.clone(),
                view_id: view.identity.clone(),
                text: draft.text.clone(),
                node_blocks: draft.node_blocks.clone(),
                selected_node: view.state.selected_reference.as_ref().and_then(super::IdeaSelectedNode::from_reference),
                document_text: draft.idea_document_text(),
                draft_revision: draft.revision,
                mode_label: "Idea",
                placeholder: "Describe the change…",
                can_send: polar_tmux_protocol::validate_codex_input(&draft.idea_document_text()).is_ok() && draft.idea_submission.is_none() && draft.idea_target.is_some() && self.idea_codex.host.is_some(),
                binding_id: draft.idea_target.as_ref().map(|b| b.thread_id.clone()),
                target_label: draft.idea_target.as_ref().map(|b| if b.title.is_empty() { b.thread_id.clone() } else { b.title.clone() }).unwrap_or_else(|| "Bind Codex…".into()),
                can_open_session: draft.idea_target.is_some() && self.idea_codex.host.is_some(),
                can_check_receipt: draft.idea_submission.is_some() && draft.pending.is_none() && self.idea_codex.host.is_some(),
                references: json!([]),
                status: if !draft.status.is_empty() { draft.status.clone() } else if draft.idea_submission.is_some() { "Unresolved submission — Check receipt".into() } else if self.idea_codex.host.is_none() { "Codex protocol unavailable".into() } else if draft.idea_target.is_some() { "Ready to send instructions".into() } else { "Bind a Codex session to send instructions".into() },
                selection: view.state.selection.clone(),
                controls: view.state.controls(),
                can_search: view.state.ready,
            };
            let encoded = serde_json::to_string(&effect).unwrap_or_default();
            if self.composer_cache.get(window) != Some(&encoded) {
                self.composer_cache.insert(window.clone(), encoded);
                out.push(effect);
            }
        }
    }
    pub(super) fn export_drafts(&self) -> Value {
        let rows = self
            .drafts
            .iter()
            .filter(|(_, d)| !d.text.is_empty() || !d.node_blocks.is_empty() || d.idea_target.is_some() || d.idea_submission.is_some() || d.idea_context.is_some())
            .map(|((window, context), d)| match context {
                DraftContext::Thread(scope) => {
                    json!({"window_id":window,"scope":scope,"text":d.text,"revision":d.revision})
                }
                DraftContext::Inquiry{scope,sender,message,node} => json!({"window_id":window,"scope":scope,"inquiry_sender":sender,"inquiry_message":message,"inquiry_node":node,"text":d.text,"revision":d.revision}),
                DraftContext::Idea(id) => {
                    json!({"idea_view_id":id,"text":d.text,"revision":d.revision,"idea_target":d.idea_target,"idea_submission":d.idea_submission,"node_blocks":d.node_blocks})
                }
            })
            .collect::<Vec<_>>();
        json!({"schema":"polar.station.drafts.v8","drafts":rows,"dismissed_inquiries":self.inquiries.dismissed})
    }
    pub(super) fn restore_drafts(&mut self, value: Value) -> Result<(), String> {
        let schema = value["schema"].as_str().unwrap_or_default();
        if ![
            "polar.station.drafts.v1",
            "polar.station.drafts.v2",
            "polar.station.drafts.v3",
            "polar.station.drafts.v4",
            "polar.station.drafts.v5",
            "polar.station.drafts.v6",
            "polar.station.drafts.v7",
            "polar.station.drafts.v8",
        ]
        .contains(&schema)
        {
            return Err("unsupported drafts schema".into());
        }
        let dismissed: BTreeSet<super::inquiry::Key> = serde_json::from_value(value.get("dismissed_inquiries").cloned().unwrap_or_else(||json!([]))).map_err(|_|"invalid dismissed inquiries")?;
        for key in &dismissed {key.scope.validate()?; validate_id(&key.sender)?;validate_id(&key.message)?;}
        let rows = value["drafts"].as_array().ok_or("invalid drafts")?;
        if rows.len() > 256 {
            return Err("too many saved drafts".into());
        }
        let mut restored = Vec::new();
        for row in rows {
            let key = if matches!(schema,"polar.station.drafts.v3"|"polar.station.drafts.v4"|"polar.station.drafts.v5"|"polar.station.drafts.v6"|"polar.station.drafts.v7"|"polar.station.drafts.v8") && row.get("idea_view_id").is_some() {
                if row.get("scope").is_some() {
                    return Err("ambiguous draft context".into());
                }
                let id = row["idea_view_id"].as_str().ok_or("invalid Idea view")?;
                validate_id(id)?;
                (String::new(), DraftContext::Idea(id.into()))
            } else {
                let scope: Scope = serde_json::from_value(row["scope"].clone())
                    .map_err(|_| "invalid draft scope")?;
                scope.validate()?;
                let window = if schema == "polar.station.drafts.v1" {
                    String::new()
                } else {
                    row["window_id"]
                        .as_str()
                        .ok_or("missing draft window")?
                        .to_owned()
                };
                if !window.is_empty() {
                    validate_id(&window)?;
                }
                if row.get("inquiry_sender").is_some() {
                    let sender=row["inquiry_sender"].as_str().ok_or("missing Inquiry sender")?.to_owned();
                    let message=row["inquiry_message"].as_str().ok_or("missing Inquiry message")?.to_owned();
                    let node=row["inquiry_node"].as_str().ok_or("missing Inquiry node")?.to_owned();
                    for id in [&sender,&message,&node]{validate_id(id)?;}
                    (window,DraftContext::Inquiry{scope,sender,message,node})
                }else{(window, scope.into())}
            };
            let text = row["text"].as_str().ok_or("invalid draft text")?;
            if text.len() > 32 * 1024 {
                return Err("draft too large".into());
            }
            let node_blocks: Vec<super::IdeaNodeBlock> = serde_json::from_value(row.get("node_blocks").cloned().unwrap_or_else(||json!([]))).map_err(|_|"invalid Idea node blocks")?;
            super::idea_document::validate_document(text,&node_blocks)?;
            if !matches!(key.1,DraftContext::Idea(_)) && !node_blocks.is_empty() {return Err("node blocks require an Idea draft".into());}
            let mut idea_target: Option<super::idea_codex::Target> = serde_json::from_value(row.get("idea_target").cloned().unwrap_or(Value::Null)).map_err(|_| "invalid Idea target")?;
            if idea_target.is_none() {
                let legacy: Option<polar_tmux_protocol::CodexBindingV1> = serde_json::from_value(row.get("idea_binding").cloned().unwrap_or(Value::Null)).map_err(|_| "invalid Idea binding")?;
                idea_target = legacy.map(|b| super::idea_codex::Target { owner_host_id: b.owner_host_id, endpoint_id: b.endpoint_id, title: b.thread_id.clone(), thread_id: b.thread_id });
            }
            if let Some(target) = &idea_target { target.validate()?; }
            let idea_submission: Option<super::idea_codex::Submission> = serde_json::from_value(row.get("idea_submission").cloned().unwrap_or(Value::Null)).map_err(|_| "invalid Idea submission")?;
            if let Some(submission) = &idea_submission { submission.target.validate()?; validate_id(&submission.intent)?; }
            restored.push((
                key,
                Draft {
                    idea_target, idea_submission, node_blocks,
                    text: text.into(),
                    revision: row["revision"].as_u64().unwrap_or(0),
                    ..Draft::default()
                },
            ));
        }
        self.inquiries.dismissed.extend(dismissed);
        for (key, draft) in restored {
            self.drafts.entry(key).or_insert(draft);
        }
        Ok(())
    }
}
