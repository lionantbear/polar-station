//! Compose existing observations and tool preparation for both native pages.
//! The desktop client still owns transport correlation, cancellation and deadlines.
use super::diagnostics::*;
use super::tool_preparation::{PreparationOutcome, ToolPreparation};
use crate::dictate::Phase;
use polar_station_protocol::{MachineConnectionV1, SessionEditorRequestV1, SessionEditorResultV1};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::{Duration, Instant}};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsInput {
    Facts {
        generation: u64,
        facts: Box<DiagnosticsFacts>,
    },
    Refresh,
    DictateTrialSucceeded,
    DictateTrialProgress { phase: Phase, error: Option<String> },
    ConnectDestination { destination: String },
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
    LaunchSetup {token:u64,setup:polar_station_protocol::ToolSetupLaunchV1},
    FocusSetup {token:u64,setup_id:String},
    Read {
        token: u64,
        request: SessionEditorRequestV1,
    },
    Present {
        action: DiagnosticAction,
    },
    SaveMachine {
        expected_revision: String,
        machine: MachineConnectionV1,
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
    manual_destination: Option<String>,
    alias_probe: Option<(u64, String)>,
    failed_probe: Option<String>,
    ssh_login: Option<String>,
    verified_alias: Option<(MachineConnectionV1, Instant)>,
    preparation: ToolPreparation,
    preparation_target: Option<String>,
    message: Option<String>,
    dictate_trial: Option<(Phase, Option<String>)>,
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
        Self::target_available_in(&self.facts, target)
    }
    fn target_available_in(facts: &DiagnosticsFacts, target: &str) -> bool {
        facts.host == DiagnosticHostConnection::Connected
            && (target == "local"
                || facts.connections.as_ref().is_some_and(|connections| {
                    connections
                        .machines
                        .iter()
                        .any(|machine| machine.id == target)
                        && connections.machine_statuses.iter().any(|status| {
                            status.id == target
                                && status.connection.status
                                    == polar_station_protocol::ControlConnectionStatusV1::Connected
                        })
                }))
    }
    fn probe_destination(&mut self, destination: String) -> Vec<DiagnosticsEffect> {
        if self.facts.host != DiagnosticHostConnection::Connected || self.alias_probe.is_some() {
            return vec![];
        }
        if !polar_station_protocol::is_canonical_ssh_destination(&destination) {
            self.message = Some("Enter an SSH host, alias, or user@host.".into());
            return vec![];
        }
        let token = self.token();
        self.verified_alias = None;
        self.failed_probe = None;
        self.message = None;
        self.alias_probe = Some((token, destination.clone()));
        vec![DiagnosticsEffect::Read {
            token,
            request: SessionEditorRequestV1::ProbeSshDestination { destination },
        }]
    }
    fn begin_preparation(
        &mut self,
        target: String,
        request: SessionEditorRequestV1,
    ) -> Vec<DiagnosticsEffect> {
        if self.preparation.is_pending() {
            return vec![];
        }
        let token = self.token();
        self.preparation.start(token, target.clone(), target.clone());
        self.preparation_target = Some(target);
        self.message = Some("Opening setup Pane…".into());
        vec![DiagnosticsEffect::Read { token, request }]
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
        if self.alias_probe.as_ref().is_some_and(|(pending, _)| *pending == token) {
            let (_, destination) = self.alias_probe.take().expect("active SSH check");
            match (result, error) {
                (Some(SessionEditorResultV1::SshDestinationVerified { machine }), None)
                    if machine.destination == destination =>
                {
                    self.verified_alias = Some((machine, Instant::now()));
                    self.message = None;
                }
                (_, failure) => {
                    self.failed_probe = Some(destination);
                    self.message = Some(failure.unwrap_or_else(|| "SSH connection could not be verified.".into()));
                }
            }
            return vec![];
        }
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
            DiagnosticsInput::DictateTrialSucceeded => {
                self.dictate_trial = Some((Phase::Delivered, None));
                vec![]
            }
            DiagnosticsInput::DictateTrialProgress { phase, error } => {
                if phase != Phase::Delivered {
                    self.dictate_trial = Some((phase, error));
                }
                vec![]
            }
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
                if generation != self.generation || facts.host != self.facts.host {
                    self.alias_probe = None;
                    self.failed_probe = None;
                    self.ssh_login = None;
                    self.verified_alias = None;
                }
                if changed {
                    self.reads.clear();
                    if let Some(target) = self.preparation_target.as_deref() {
                        let original = self.facts.connections.as_ref()
                            .and_then(|connections| connections.machines.iter().find(|machine| machine.id == target));
                        let updated = facts.connections.as_ref()
                            .and_then(|connections| connections.machines.iter().find(|machine| machine.id == target));
                        if generation != self.generation || !Self::target_available_in(&facts, target)
                            || original != updated
                        {
                            self.preparation = ToolPreparation::default();
                            self.preparation_target = None;
                            self.message = Some("The setup connection changed. Recheck the original machine before retrying.".into());
                        }
                    }
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
            DiagnosticsInput::ConnectDestination { destination } => {
                let destination = destination.trim().to_owned();
                let effects = self.probe_destination(destination.clone());
                if !effects.is_empty() { self.manual_destination = Some(destination); }
                effects
            }
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
                else if let DiagnosticAction::ConnectSshAlias { destination } = action {
                    self.probe_destination(destination)
                } else if let DiagnosticAction::OpenSshLogin { destination } = action {
                    let effects = self.begin_preparation(
                        "local".into(), SessionEditorRequestV1::PrepareSshLogin { destination: destination.clone() },
                    );
                    if !effects.is_empty() {
                        self.ssh_login = Some(destination);
                    }
                    effects
                } else if let DiagnosticAction::AddVerifiedSshAlias { destination } = action {
                    match (self.verified_alias.as_ref(), self.facts.connections.as_ref()) {
                        (Some((machine, checked)), Some(connections))
                            if machine.destination == destination && checked.elapsed() < Duration::from_secs(120) =>
                        {
                            let effect = DiagnosticsEffect::SaveMachine {
                                expected_revision: connections.revision.clone(),
                                machine: machine.clone(),
                            };
                            self.verified_alias = None;
                            vec![effect]
                        }
                        _ => vec![],
                    }
                }
                else if let DiagnosticAction::FocusSetup{setup_id}=action {
                    self.preparation.focus().filter(|(_,id)|*id==setup_id).map(|(token,_)|vec![DiagnosticsEffect::FocusSetup{token,setup_id}]).unwrap_or_default()
                } else if let DiagnosticAction::PrepareTools {
                    target_id,
                    tmux,
                    codex,
                    codex_service,
                    claude,
                    claude_login,
                } = action
                {
                    self.begin_preparation(target_id.clone(), SessionEditorRequestV1::PrepareTools {
                        execution_target_id: target_id,
                        tmux,
                        codex,
                        codex_service,
                        claude,
                        claude_login,
                        delivery: polar_station_protocol::ToolSetupDeliveryV1::NativePane,
                    })
                } else {
                    if action == DiagnosticAction::TryDictate {
                        self.dictate_trial = Some((Phase::Starting, None));
                    }
                    vec![DiagnosticsEffect::Present { action }]
                }
            }
        };
        self.output(effects)
    }
    fn output(&self,effects:Vec<DiagnosticsEffect>)->DiagnosticsOutput {
        DiagnosticsOutput {projection:self.projection(),busy:self.alias_probe.is_some() || (self.preparation.is_pending() && self.preparation.focus().is_none()) || !self.reads.is_empty(),message:self.message.clone(),effects}
    }
    fn projection(&self)->DiagnosticsProjection {
        let mut projection=project_diagnostics(&self.facts);
        if let (Some((phase, error)), Some(trial)) = (
            &self.dictate_trial,
            projection.local.iter_mut().find(|row| row.id == "voice_trial"),
        ) {
            let (state, detail, action): (_, &str, _) = match phase {
                Phase::Starting => (DiagnosticState::Busy, "Opening microphone…", None),
                Phase::Recording => (DiagnosticState::Busy, "Recording… Stop in Dictate Preview.", None),
                Phase::Transcribing | Phase::Rewriting | Phase::Delivering =>
                    (DiagnosticState::Busy, "Transcribing… Your result will appear here.", None),
                Phase::Delivered => (DiagnosticState::Ready, "Dictate verified", Some("Try again")),
                Phase::Failed => (DiagnosticState::NeedsAttention,
                    error.as_deref().unwrap_or("Dictate failed. Check audio input or try again."), Some("Try again")),
                Phase::Cancelled => (DiagnosticState::Unknown, "Test cancelled.", Some("Try again")),
                _ => (DiagnosticState::NeedsAttention, "Text did not reach the test field. Try again.", Some("Try again")),
            };
            trial.state = state;
            trial.detail = detail.into();
            trial.action = action.map(|_| DiagnosticAction::TryDictate);
            trial.action_label = action.map(str::to_owned);
        }
        let connections = self.facts.connections.as_ref()
            .filter(|_| self.facts.host == DiagnosticHostConnection::Connected);
        let local_checks = self.facts.tools.iter().find(|tool| tool.execution_target_id == "local")
            .and_then(|tool| tool.setup_checks.as_ref());
        if let (Some(connections), Some(checks)) = (connections, local_checks) {
            let aliases = checks.ssh_aliases.iter().chain(self.manual_destination.iter()
                .filter(|destination| !checks.ssh_aliases.contains(destination)));
            for alias in aliases {
                let saved = connections.machines.iter().any(|machine| machine.destination.eq_ignore_ascii_case(alias));
                let verified = self.verified_alias.as_ref().is_some_and(|(machine, checked)| {
                    machine.destination == *alias && checked.elapsed() < Duration::from_secs(120)
                });
                let (state, detail, action, label) = if saved {
                    (DiagnosticState::Ready, "Already added.", None, None)
                } else if verified {
                    (DiagnosticState::Ready, "SSH connected. Add this host to Station.",
                     Some(DiagnosticAction::AddVerifiedSshAlias { destination: alias.clone() }), Some("Add".into()))
                } else if self.failed_probe.as_ref() == Some(alias) {
                    (DiagnosticState::NeedsAttention, "Connect from the Shell Pane to handle SSH login or the host-key prompt.",
                     Some(DiagnosticAction::OpenSshLogin { destination: alias.clone() }), Some("Open SSH terminal".into()))
                } else {
                    (DiagnosticState::Unknown, "OpenSSH configuration.",
                     Some(DiagnosticAction::ConnectSshAlias { destination: alias.clone() }), Some("Connect".into()))
                };
                projection.remote_setup.push(DiagnosticRow {
                    id: format!("ssh:{alias}"), title: alias.clone(), state, detail: detail.into(),
                    action, action_label: label,
                });
            }
        }
        if let Some((_,id))=self.preparation.focus() {
            for row in projection.local.iter_mut().chain(projection.remote.iter_mut().flat_map(|host|host.rows.iter_mut())) {
                if matches!(row.action,Some(DiagnosticAction::PrepareTools{..})) {
                    row.action=None;row.action_label=None;
                }
            }
            let target=self.preparation_target.as_deref();
            let rows=if target==Some("local") {Some(&mut projection.local)}
                else {projection.remote.iter_mut().find(|host|Some(host.id.as_str())==target).map(|host|&mut host.rows)};
            if let Some(rows)=rows {rows.retain(|row| row.id != "start_setup");rows.insert(0,DiagnosticRow{id:format!("setup:{id}"),title:"Setup in progress".into(),state:DiagnosticState::Busy,
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
                let target=self.preparation_target.take().expect("setup target");
                if let Some(destination) = self.ssh_login.take() {
                    if self.message.is_none() {
                        return self.probe_destination(destination);
                    }
                    return vec![];
                }
                self.reads.retain(|_,pending|*pending!=target);
                self.inspect(target)
            }
            PreparationOutcome::Stale=>{self.preparation_target=None;self.ssh_login=None;self.message=Some("The selected machine changed. Reconnect and check its tools.".into());vec![]}
            PreparationOutcome::Failed(message)=>{self.preparation_target=None;self.ssh_login=None;self.message=Some(message);vec![]}
            PreparationOutcome::Ignored=>vec![],
        }
    }
}


