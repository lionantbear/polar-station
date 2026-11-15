//! Compose existing observations and tool preparation for both native pages.
//! The desktop client still owns transport correlation, cancellation and deadlines.
use super::diagnostics::*;
use super::tool_preparation::{PreparationOutcome, ToolPreparation};
use polar_tmux_protocol::{SessionEditorRequestV1, SessionEditorResultV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsInput {
    Facts {
        generation: u64,
        facts: Box<DiagnosticsFacts>,
    },
    Refresh,
    Action {
        action: DiagnosticAction,
    },
    Returned,
    SetupStarted {token:u64,error:Option<String>},
    SetupExited {token:u64,exit_code:Option<u32>},
    Complete {
        token: u64,
        result: Option<Box<SessionEditorResultV1>>,
        error: Option<String>,
    },
}
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsEffect {
    LaunchSetup {token:u64,setup:polar_tmux_protocol::ToolSetupLaunchV1},
    FocusSetup {token:u64,setup_id:String},
    Read {
        token: u64,
        request: SessionEditorRequestV1,
    },
    Present {
        action: DiagnosticAction,
    },
}
#[derive(Debug, Serialize)]
pub struct DiagnosticsOutput {
    pub projection: DiagnosticsProjection,
    pub busy: bool,
    pub message: Option<String>,
    pub effects: Vec<DiagnosticsEffect>,
}
#[derive(Default)]
pub struct DiagnosticsEngine {
    facts: DiagnosticsFacts,
    generation: u64,
    next_token: u64,
    reads: BTreeMap<u64, String>,
    preparation: ToolPreparation,
    preparation_target: Option<String>,
    message: Option<String>,
}
impl DiagnosticsEngine {
    fn token(&mut self) -> u64 {
        self.next_token = self
            .next_token
            .checked_add(1)
            .expect("diagnostics token exhausted");
        self.next_token
    }
    fn target_available(&self, target: &str) -> bool {
        self.facts.host == DiagnosticHostConnection::Connected
            && (target == "local"
                || self.facts.connections.as_ref().is_some_and(|connections| {
                    connections
                        .machines
                        .iter()
                        .any(|machine| machine.id == target)
                        && connections.machine_statuses.iter().any(|status| {
                            status.id == target
                                && status.connection.status
                                    == polar_tmux_protocol::ControlConnectionStatusV1::Connected
                        })
                }))
    }
    fn inspect(&mut self, target: String) -> Vec<DiagnosticsEffect> {
        if self.facts.inspection_supported != Some(true)
            || !self.target_available(&target)
            || self.reads.values().any(|current| *current == target)
        {
            return vec![];
        }
        let token = self.token();
        self.reads.insert(token, target.clone());
        vec![DiagnosticsEffect::Read {
            token,
            request: SessionEditorRequestV1::InspectTools {
                execution_target_id: target,
            },
        }]
    }
    fn refresh(&mut self) -> Vec<DiagnosticsEffect> {
        let targets = self
            .facts
            .connections
            .as_ref()
            .map(|connections| {
                connections
                    .machines
                    .iter()
                    .map(|machine| machine.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut effects = self.inspect("local".into());
        for target in targets {
            effects.extend(self.inspect(target));
        }
        effects
    }
    fn complete(
        &mut self,
        token: u64,
        result: Option<SessionEditorResultV1>,
        error: Option<String>,
    ) -> Vec<DiagnosticsEffect> {
        if self.preparation.matches(token) {
            let target = self.preparation_target.clone().expect("preparation target");
            let active = self.target_available(&target);
            let outcome=self.preparation.complete(active.then_some(target.as_str()),result,error);
            return self.preparation_outcome(outcome);
        } else if let Some(target) = self.reads.remove(&token) {
            if !self.target_available(&target) {
                return vec![];
            }
            match (result, error) {
                (Some(SessionEditorResultV1::Tools { tools }), None) => {
                    // InspectTools can additionally report local prerequisites. Only
                    // accept facts for the requested machine, never another request's target.
                    if let Some(tool) = tools
                        .into_iter()
                        .find(|tool| tool.execution_target_id == target)
                    {
                        self.facts
                            .tools
                            .retain(|old| old.execution_target_id != target);
                        self.facts.tools.push(tool);
                        self.message = None;
                    } else {
                        self.message =
                            Some("The Host did not return tools for the requested machine.".into());
                    }
                }
                (_, error) => {
                    self.message =
                        Some(error.unwrap_or_else(|| "Tool inspection was not confirmed.".into()))
                }
            }
        }
        vec![]
    }
    pub fn dispatch(&mut self, input: DiagnosticsInput) -> DiagnosticsOutput {
        let effects = match input {
            DiagnosticsInput::Facts {
                generation,
                mut facts,
            } => {
                let connection_changed = match (&facts.connections, &self.facts.connections) {
                    (Some(next), Some(previous)) => {
                        next.revision != previous.revision
                            || next.machine_statuses != previous.machine_statuses
                    }
                    (None, None) => false,
                    _ => true,
                };
                let changed = generation != self.generation
                    || facts.host != self.facts.host
                    || connection_changed
                    || facts.inspection_supported != self.facts.inspection_supported;
                if changed {
                    self.reads.clear();
                    self.preparation = ToolPreparation::default();
                    self.preparation_target = None;
                    self.message = None;
                } else {
                    facts.tools = std::mem::take(&mut self.facts.tools);
                }
                self.generation = generation;
                self.facts = *facts;
                let active_targets: Vec<_> = self
                    .facts
                    .tools
                    .iter()
                    .filter(|tool| self.target_available(&tool.execution_target_id))
                    .map(|tool| tool.execution_target_id.clone())
                    .collect();
                self.facts
                    .tools
                    .retain(|tool| active_targets.contains(&tool.execution_target_id));
                if changed {
                    self.refresh()
                } else {
                    vec![]
                }
            }
            DiagnosticsInput::Refresh => self.refresh(),
            DiagnosticsInput::Returned=>vec![],
            DiagnosticsInput::SetupStarted{token,error}=>{
                let target=self.preparation_target.clone();
                let active=target.as_ref().is_some_and(|target|self.target_available(target));
                let outcome=self.preparation.started(target.as_deref().filter(|_|active),token,error);
                self.preparation_outcome(outcome)
            }
            DiagnosticsInput::SetupExited{token,exit_code}=>{
                let target=self.preparation_target.clone();
                let active=target.as_ref().is_some_and(|target|self.target_available(target));
                let outcome=self.preparation.exited(target.as_deref().filter(|_|active),token,exit_code);
                self.preparation_outcome(outcome)
            }
            DiagnosticsInput::Complete {
                token,
                result,
                error,
            } => self.complete(token, result.map(|result| *result), error),
            DiagnosticsInput::Action { action } => {
                let projection = self.projection();
                let allowed = projection
                    .local
                    .iter()
                    .chain(projection.remote_setup.iter())
                    .chain(projection.remote.iter().flat_map(|host| host.rows.iter()))
                    .any(|row| row.action.as_ref() == Some(&action));
                if !allowed { vec![] }
                else if let DiagnosticAction::FocusSetup{setup_id}=action {
                    self.preparation.focus().filter(|(_,id)|*id==setup_id).map(|(token,_)|vec![DiagnosticsEffect::FocusSetup{token,setup_id}]).unwrap_or_default()
                } else if let DiagnosticAction::PrepareTools {
                    target_id,
                    tmux,
                    codex,
                    codex_service,
                    claude,
                } = action
                {
                    if self.preparation.is_pending() {return self.output(vec![]);}
                    let token = self.token();
                    self.preparation
                        .start(token, target_id.clone(), target_id.clone());
                    self.preparation_target = Some(target_id.clone());
                    self.message = Some("Opening setup Pane…".into());
                    vec![DiagnosticsEffect::Read {
                        token,
                        request: SessionEditorRequestV1::PrepareTools {
                            execution_target_id: target_id,
                            tmux,
                            codex,
                            codex_service,
                            claude,
                            delivery:polar_tmux_protocol::ToolSetupDeliveryV1::NativePane,
                        },
                    }]
                } else {
                    vec![DiagnosticsEffect::Present { action }]
                }
            }
        };
        self.output(effects)
    }
    fn output(&self,effects:Vec<DiagnosticsEffect>)->DiagnosticsOutput {
        DiagnosticsOutput {projection:self.projection(),busy:(self.preparation.is_pending() && self.preparation.focus().is_none()) || !self.reads.is_empty(),message:self.message.clone(),effects}
    }
    fn projection(&self)->DiagnosticsProjection {
        let mut projection=project_diagnostics(&self.facts);
        if let Some((_,id))=self.preparation.focus() {
            for row in projection.local.iter_mut().chain(projection.remote.iter_mut().flat_map(|host|host.rows.iter_mut())) {
                if matches!(row.action,Some(DiagnosticAction::PrepareTools{..})) {
                    row.action=None;row.action_label=None;
                    row.detail="Setup is already running. Complete it in its Shell Pane first.".into();
                }
            }
            let target=self.preparation_target.as_deref();
            let rows=if target==Some("local") {Some(&mut projection.local)}
                else {projection.remote.iter_mut().find(|host|Some(host.id.as_str())==target).map(|host|&mut host.rows)};
            if let Some(rows)=rows {rows.insert(0,DiagnosticRow{id:format!("setup:{id}"),title:"Setup in progress".into(),state:DiagnosticState::Busy,
                detail:"Setup is running in its Shell Pane; enter any required password there.".into(),
                action:Some(DiagnosticAction::FocusSetup{setup_id:id.into()}),action_label:Some("Show setup Pane".into())});}
        }
        projection
    }
    fn preparation_outcome(&mut self,outcome:PreparationOutcome)->Vec<DiagnosticsEffect> {
        match outcome {
            PreparationOutcome::Launch{token,setup}=>vec![DiagnosticsEffect::LaunchSetup{token,setup}],
            PreparationOutcome::Running=>{self.message=None;vec![]}
            PreparationOutcome::Recheck{message,..}=>{
                self.message=message;
                let target=self.preparation_target.clone().expect("setup target");
                self.reads.retain(|_,pending|*pending!=target);
                self.inspect(target)
            }
            PreparationOutcome::Stale=>{self.message=Some("The selected machine changed. Reconnect and check its tools.".into());vec![]}
            PreparationOutcome::Failed(message)=>{self.message=Some(message);vec![]}
            PreparationOutcome::Ignored=>vec![],
        }
    }
}


