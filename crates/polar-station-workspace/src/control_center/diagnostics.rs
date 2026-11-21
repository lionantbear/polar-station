//! Pure readiness composition. Inputs are observations from existing owners;
//! this module does not perform checks, setup, authentication or I/O.
use polar_station_protocol::{
    ConnectionsProjectionV1, SessionTargetToolsV1,
    SetupAuthenticationV1 as Auth,
};
use polar_station_resources::ResourcesControlProjection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticHostConnection {
    #[default]
    Unknown,
    Connected,
    Offline,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticsFacts {
    pub host: DiagnosticHostConnection,
    pub connections: Option<ConnectionsProjectionV1>,
    #[serde(default)]
    pub tools: Vec<SessionTargetToolsV1>,
    pub resources: Option<ResourcesControlProjection>,
    #[serde(default)]
    pub requested: Vec<RequestedCapability>,
    #[serde(default)]
    pub codex_settings: Option<polar_station_settings::CodexControlSettingsV1>,
    #[serde(default)]
    pub inspection_supported: Option<bool>,
    #[serde(default)]
    pub voice: Option<VoiceDiagnosticsFacts>,
}

/// Observed platform permission only; reading this value never requests access.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPermission {
    #[default]
    Unknown,
    NotRequested,
    Granted,
    Denied,
    Restricted,
    NotRequired,
}

/// Existing capture/account/storage owners supply facts, not UI decisions.
/// An absent observation is unknown, never an absent dependency or permission denial.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceDiagnosticsFacts {
    pub enabled: bool,
    pub microphone: DiagnosticPermission,
    pub input_device: Option<polar_station_protocol::SessionToolStateV1>,
    pub account_signed_in: Option<bool>,
    pub service_available: Option<bool>,
    pub external_input_requested: bool,
    pub external_input_permission: DiagnosticPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSetupSection {
    Account,
    Microphone,
    Service,
    ExternalInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedCapability {
    pub target_id: String,
    pub capability: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticState {
    Ready,
    Missing,
    Unknown,
    Offline,
    NotEnabled,
    Busy,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticAction {
    ReconnectHost,
    AddMachineConnection,
    ConnectSshAlias { destination: String },
    OpenSshLogin { destination: String },
    AddVerifiedSshAlias { destination: String },
    EditVoiceSetup {
        section: VoiceSetupSection,
    },
    RequestMicrophoneAccess,
    TryDictate,
    OpenMicrophoneSettings,
    RequestExternalInputAccess,
    OpenExternalInputSettings,
    FocusSetup { setup_id:String },
    PrepareTools {
        target_id: String,
        tmux: bool,
        codex: bool,
        codex_service: bool,
        claude: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        claude_login: bool,
    },
    EditConnection {
        target_id: String,
    },
    EditPolarConnection {
        target_id: String,
        connection_id: String,
    },
    EditCodexEndpoint {
        target_id: String,
        endpoint_id: String,
    },
    EditResource {
        resource_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticRow {
    pub id: String,
    pub title: String,
    pub state: DiagnosticState,
    pub detail: String,
    pub action: Option<DiagnosticAction>,
    pub action_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDiagnostics {
    pub id: String,
    pub name: String,
    pub rows: Vec<DiagnosticRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsProjection {
    pub local_title: String,
    pub remote_title: String,
    pub refresh_label: String,
    pub empty_remote_label: String,
    pub local: Vec<DiagnosticRow>,
    pub remote_setup: Vec<DiagnosticRow>,
    pub remote: Vec<RemoteDiagnostics>,
    #[serde(default)]
    pub capability_sections: BTreeMap<String, String>,
}

pub fn project_diagnostics(facts: &DiagnosticsFacts) -> DiagnosticsProjection {
    use polar_station_protocol::ControlConnectionStatusV1 as Connection;
    let online = facts.host == DiagnosticHostConnection::Connected;
    let (host_state, host_detail, host_action) = match facts.host {
        DiagnosticHostConnection::Connected => (DiagnosticState::Ready, "Connected", None),
        DiagnosticHostConnection::Offline => (
            DiagnosticState::Offline,
            "Station Host is unreachable on this computer.",
            Some(DiagnosticAction::ReconnectHost),
        ),
        DiagnosticHostConnection::Unknown => (
            DiagnosticState::Unknown,
            "Station Host connection has not been checked.",
            Some(DiagnosticAction::ReconnectHost),
        ),
    };
    let mut local = vec![DiagnosticRow {
        id: "station_host".into(),
        title: "Station Host".into(),
        state: host_state,
        detail: host_detail.into(),
        action: host_action,
        action_label: None,
    }];
    local.extend(tool_rows(facts, "local", "This computer", online));
    local.extend(voice_rows(facts, online));
    let mut remote_setup = vec![DiagnosticRow {
        id: "add_machine".into(),
        title: "Remote connections".into(),
        state: DiagnosticState::NotEnabled,
        detail: "Add a machine using the existing SSH connection settings.".into(),
        action: online.then_some(DiagnosticAction::AddMachineConnection),
        action_label: None,
    }];
    let mut remote = Vec::new();
    if let Some(connections) = &facts.connections {
        for machine in &connections.machines {
            let status = connections
                .machine_statuses
                .iter()
                .find(|s| s.id == machine.id)
                .map(|s| s.connection.status);
            let reachable = online && status == Some(Connection::Connected);
            let state = if !online {
                DiagnosticState::Unknown
            } else {
                match status {
                    Some(Connection::Connected) => DiagnosticState::Ready,
                    Some(Connection::Connecting | Connection::Recovering) => DiagnosticState::Busy,
                    Some(Connection::Offline) => DiagnosticState::Offline,
                    Some(Connection::Partial | Connection::Inactive) | None => {
                        DiagnosticState::Unknown
                    }
                }
            };
            let detail = if !online {
                "Station Host is unreachable; remote status is unknown.".to_owned()
            } else {
                connections
                    .machine_statuses
                    .iter()
                    .find(|s| s.id == machine.id)
                    .map(|s| s.connection.accessibility_label.clone())
                    .unwrap_or_else(|| "Connection has not been checked.".into())
            };
            let mut rows = vec![DiagnosticRow {
                id: "connection".into(),
                title: "SSH connection".into(),
                state,
                detail,
                action: Some(DiagnosticAction::EditConnection {
                    target_id: machine.id.clone(),
                }),
                action_label: None,
            }];
            rows.extend(tool_rows(facts, &machine.id, &machine.display_name, reachable));
            if let Some(resources) = &facts.resources {
                for entry in resources
                    .entries
                    .iter()
                    .filter(|e| e.enabled && e.execution_target_id == machine.id)
                {
                    use polar_station_resources::ResourceRuntimeState as State;
                    let state = if !reachable {
                        DiagnosticState::Unknown
                    } else {
                        match entry.status.state {
                            State::Running => DiagnosticState::Ready,
                            State::Disabled => DiagnosticState::NotEnabled,
                            State::Starting | State::Recovering | State::Stopping => {
                                DiagnosticState::Busy
                            }
                            State::WaitingForSsh => DiagnosticState::Offline,
                            State::NeedsAttention => DiagnosticState::NeedsAttention,
                            State::Stopped => DiagnosticState::Unknown,
                        }
                    };
                    let detail = if reachable {
                        entry.status.message.clone()
                    } else {
                        "Connection unavailable; current mapping state is unknown.".into()
                    };
                    rows.push(DiagnosticRow {
                        id: format!("resource:{}", entry.id),
                        title: entry.name.clone(),
                        state,
                        detail,
                        action: Some(DiagnosticAction::EditResource {
                            resource_id: entry.id.to_string(),
                        }),
                        action_label: None,
                    });
                }
            }
            remote.push(RemoteDiagnostics {
                id: machine.id.clone(),
                name: machine.display_name.clone(),
                rows,
            });
        }
        for polar in &connections.polar_instances {
            let row = DiagnosticRow {
                id: format!("polar:{}", polar.id),
                title: format!("Polar connection: {}", polar.display_name),
                state: DiagnosticState::Unknown,
                detail: "Connection has not been verified.".into(),
                action: Some(DiagnosticAction::EditPolarConnection {
                    target_id: polar.execution_target_id.clone(),
                    connection_id: polar.id.clone(),
                }),
                action_label: None,
            };
            if polar.execution_target_id == "local" {
                local.push(row)
            } else {
                let host = remote
                    .iter_mut()
                    .find(|r| r.id == polar.execution_target_id);
                if let Some(host) = host {
                    host.rows.push(row)
                } else {
                    remote.push(RemoteDiagnostics {
                        id: polar.execution_target_id.clone(),
                        name: polar.execution_target_id.clone(),
                        rows: vec![row],
                    });
                }
            }
        }
    }
    if let Some(settings) = &facts.codex_settings {
        use polar_station_settings::CodexControlModeV1 as Mode;
        for endpoint in &settings.endpoints {
            let (id, name, detail) = match endpoint.control_mode {
                Mode::Local => (
                    "local".to_owned(),
                    "This computer".to_owned(),
                    "Local Codex App Server status is not verified.",
                ),
                Mode::SshDirect => (
                    endpoint.execution_target_id.clone(),
                    endpoint.execution_target_id.clone(),
                    "SSH Codex App Server status is not verified.",
                ),
                Mode::RemoteHost => (
                    format!("station:{}", endpoint.owner_host_id),
                    endpoint.owner_host_id.clone(),
                    "Remote Station Host Codex endpoint status is not verified.",
                ),
            };
            let row = DiagnosticRow {
                id: format!("codex_endpoint:{}", endpoint.id),
                title: format!("Codex endpoint: {}", endpoint.id),
                state: if settings.enabled {
                    DiagnosticState::Unknown
                } else {
                    DiagnosticState::NotEnabled
                },
                detail: detail.into(),
                action: Some(DiagnosticAction::EditCodexEndpoint {
                    target_id: endpoint.execution_target_id.clone(),
                    endpoint_id: endpoint.id.clone(),
                }),
                action_label: None,
            };
            if id == "local" {
                local.push(row)
            } else if let Some(host) = remote.iter_mut().find(|host| host.id == id) {
                host.rows.push(row)
            } else {
                remote.push(RemoteDiagnostics {
                    id,
                    name,
                    rows: vec![row],
                });
            }
        }
    }
    for row in local
        .iter_mut()
        .chain(remote_setup.iter_mut())
        .chain(remote.iter_mut().flat_map(|host| host.rows.iter_mut()))
    {
        if row.action_label.is_some() { continue; }
        row.action_label = row.action.as_ref().map(|action| {
            match action {
                DiagnosticAction::ReconnectHost => "Retry connection",
                DiagnosticAction::AddMachineConnection => "Add remote host",
                DiagnosticAction::ConnectSshAlias { .. } => "Connect",
                DiagnosticAction::OpenSshLogin { .. } => "Open SSH terminal",
                DiagnosticAction::AddVerifiedSshAlias { .. } => "Add",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Account,
                } => "Account settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Microphone,
                } => "Input device settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Service,
                } => "Voice service settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::ExternalInput,
                } => "External input settings",
                DiagnosticAction::TryDictate => "Try Dictate",
                DiagnosticAction::RequestMicrophoneAccess => "Request microphone access",
                DiagnosticAction::OpenMicrophoneSettings => "Open microphone settings",
                DiagnosticAction::RequestExternalInputAccess => "Allow external input",
                DiagnosticAction::OpenExternalInputSettings => "Open external input permissions",
                DiagnosticAction::FocusSetup { .. } => "Show setup Pane",
                DiagnosticAction::PrepareTools { tmux: true, .. } => "Install tmux",
                DiagnosticAction::PrepareTools { codex: true, .. } => "Install Codex",
                DiagnosticAction::PrepareTools { claude: true, .. } => "Install Claude",
                DiagnosticAction::PrepareTools { .. } => "Set up tools",
                DiagnosticAction::EditConnection { .. }
                | DiagnosticAction::EditPolarConnection { .. } => "Connection settings",
                DiagnosticAction::EditCodexEndpoint { .. } => "Codex settings",
                DiagnosticAction::EditResource { .. } => "Resource settings",
            }
            .into()
        });
    }
    let mut capability_sections = BTreeMap::new();
    for rows in std::iter::once(&mut local).chain(remote.iter_mut().map(|host| &mut host.rows)) {
        project_capability_sections(rows, &mut capability_sections);
    }
    DiagnosticsProjection {
        local_title: "Local Diagnostics".into(),
        remote_title: "Remote Hosts".into(),
        refresh_label: "Refresh".into(),
        empty_remote_label: "No remote hosts configured.".into(),
        local,
        remote_setup,
        remote,
        capability_sections,
    }
}

fn project_capability_sections(
    rows: &mut [DiagnosticRow],
    sections: &mut BTreeMap<String, String>,
) {
    const CODEX: u8 = 4;
    const CLAUDE: u8 = 5;
    const DICTATE: u8 = 6;
    let order = |id: &str| match id {
        "station_host" => 0,
        "connection" => 1,
        "start_setup" => 2,
        "tmux" => 3,
        "codex" | "codex_auth" | "codex_server" => CODEX,
        "claude" | "claude_auth" => CLAUDE,
        id if id.starts_with("voice_") => DICTATE,
        _ => 7,
    };
    rows.sort_by_key(|row| order(&row.id));
    for row in rows {
        let section = match order(&row.id) {
            CODEX => "Codex",
            CLAUDE => "Claude Code",
            DICTATE => "Dictate",
            _ => continue,
        };
        sections.entry(row.id.clone()).or_insert_with(|| section.into());
        match row.id.as_str() {
            "codex" | "claude" => row.title = "CLI".into(),
            "codex_auth" | "claude_auth" => row.title = "Account".into(),
            "codex_server" => row.title = "App Server".into(),
            "voice_trial" => row.title = "Recording test".into(),
            _ => {}
        }
    }
}

fn voice_rows(facts: &DiagnosticsFacts, host_online: bool) -> Vec<DiagnosticRow> {
    use polar_station_protocol::SessionToolStateV1 as Tool;
    let empty = VoiceDiagnosticsFacts::default();
    let voice = facts.voice.as_ref().unwrap_or(&empty);
    let required = voice.enabled
        || facts
            .requested
            .iter()
            .any(|r| r.target_id == "local" && r.capability == "dictate");
    let mut rows = vec![permission_row(
        "voice_microphone",
        "Microphone permission",
        voice.microphone,
        required,
        false,
    )];
    let (state, detail) = match voice.input_device {
        Some(Tool::Available) => (DiagnosticState::Ready, "The selected audio input is available; capture has not been tested."),
        Some(Tool::Missing) => (missing_state(required), "The selected audio input is unavailable. Select a connected microphone or the system default."),
        Some(Tool::Unavailable) => (DiagnosticState::Unknown, "Audio input devices could not be inspected."),
        None => (DiagnosticState::Unknown, "Audio input devices have not been checked."),
    };
    rows.push(DiagnosticRow {
        id: "voice_input_device".into(),
        title: "Audio input".into(),
        state,
        detail: detail.into(),
        action: Some(DiagnosticAction::EditVoiceSetup {
            section: VoiceSetupSection::Microphone,
        }),
        action_label: None,
    });
    for (id,title,observation,section,ready,missing) in [
        ("voice_account","Dictate account",voice.account_signed_in,VoiceSetupSection::Account,
         "An account is connected. Transcription has not been tested.",
         "Connect an account to transcribe recordings. No Codex command-line installation is required."),
        ("voice_service","Voice service",voice.service_available,VoiceSetupSection::Service,
         "The local Voice service is available. This does not verify the remote transcription provider.",
         "The local Voice service is unavailable. Check its setup before recording."),
    ] {
        let (state,detail) = match observation.filter(|_|host_online) {
            Some(true)=>(DiagnosticState::Ready,ready),
            Some(false)=>(missing_state(required),missing),
            None=>(DiagnosticState::Unknown,if host_online {"Not checked on this computer."}else{"Station Host is unreachable; current service status is unknown."}),
        };
        rows.push(DiagnosticRow {id:id.into(),title:title.into(),state,detail:detail.into(),action:Some(DiagnosticAction::EditVoiceSetup {section}),action_label:None});
    }
    if host_online {
        rows.push(DiagnosticRow {
            id: "voice_trial".into(), title: "Dictate".into(), state: DiagnosticState::Unknown,
            detail: "Not tested. Say a short sentence to check recording and transcription.".into(),
            action: Some(DiagnosticAction::TryDictate), action_label: None,
        });
    }
    if voice.external_input_requested {
        rows.push(permission_row(
            "voice_external_input",
            "External input permission",
            voice.external_input_permission,
            true,
            true,
        ));
    }
    rows
}

fn missing_state(required: bool) -> DiagnosticState {
    if required {
        DiagnosticState::NeedsAttention
    } else {
        DiagnosticState::NotEnabled
    }
}

fn permission_row(
    id: &str,
    title: &str,
    permission: DiagnosticPermission,
    required: bool,
    external: bool,
) -> DiagnosticRow {
    let request = if external {
        DiagnosticAction::RequestExternalInputAccess
    } else {
        DiagnosticAction::RequestMicrophoneAccess
    };
    let settings = if external {
        DiagnosticAction::OpenExternalInputSettings
    } else {
        DiagnosticAction::OpenMicrophoneSettings
    };
    let (state, detail, action) = match permission {
        DiagnosticPermission::Granted => (DiagnosticState::Ready, "Permission is granted.", None),
        DiagnosticPermission::NotRequired => (
            DiagnosticState::Ready,
            "This platform does not require this permission.",
            None,
        ),
        DiagnosticPermission::NotRequested => (
            missing_state(required),
            if external {
                "Allow inserting into other applications. Polar Station internal input remains available."
            } else {
                "Microphone access has not been requested. Grant access before recording."
            },
            Some(request),
        ),
        DiagnosticPermission::Denied => (
            missing_state(required),
            if external {
                "External input permission is off. Enable it in system settings; Polar Station internal input remains available."
            } else {
                "Microphone permission is off. Allow Polar Station in the system microphone settings."
            },
            Some(settings),
        ),
        DiagnosticPermission::Restricted => (
            missing_state(required),
            if external {
                "System restrictions prevent external input; Polar Station internal input remains available."
            } else {
                "System restrictions prevent microphone access. The application cannot change this restriction."
            },
            None,
        ),
        DiagnosticPermission::Unknown => (
            DiagnosticState::Unknown,
            "Permission has not been checked.",
            None,
        ),
    };
    DiagnosticRow {
        id: id.into(),
        title: title.into(),
        state,
        detail: detail.into(),
        action,
        action_label: None,
    }
}

fn tool_rows(facts: &DiagnosticsFacts, target: &str, name: &str, reachable: bool) -> Vec<DiagnosticRow> {
    use polar_station_protocol::SessionToolStateV1 as Tool;
    let tools = if reachable {
        facts.tools.iter().find(|t| t.execution_target_id == target)
    } else {
        None
    };
    let mut rows = Vec::new();
    for (id, title, state, optional) in [
        ("tmux", "Terminal · tmux", tools.map(|t| t.tmux), false),
        ("codex", "Codex", tools.map(|t| t.codex), true),
        ("claude", "Claude Code", tools.and_then(|t| t.claude), true),
    ] {
        let required = facts
            .requested
            .iter()
            .any(|r| r.target_id == target && r.capability == id)
            || (id == "codex"
                && facts.codex_settings.as_ref().is_some_and(|settings| {
                    settings.enabled
                        && settings.endpoints.iter().any(|e| {
                            e.execution_target_id == target
                                && e.control_mode
                                    != polar_station_settings::CodexControlModeV1::RemoteHost
                        })
                }));
        let (state, detail, action) = match state {
            Some(Tool::Available) => (
                DiagnosticState::Ready,
                if id == "tmux" {
                    "Executable available."
                } else {
                    "Executable available. Authentication is checked separately."
                },
                None,
            ),
            Some(Tool::Missing) => (
                if optional && !required {
                    DiagnosticState::NotEnabled
                } else {
                    DiagnosticState::Missing
                },
                if tools.is_some_and(|tool|!tool.native_setup) {
                    "Not installed. Update this Station Host to install in an interactive Shell Pane."
                } else if optional && !required {
                    "Not installed; available to set up."
                } else {
                    "Required on this machine for the requested capability."
                },
                tools.filter(|tool|tool.native_setup).map(|_|DiagnosticAction::PrepareTools {
                    target_id: target.into(),
                    tmux: id == "tmux",
                    codex: id == "codex",
                    codex_service: false,
                    claude: id == "claude",
                    claude_login: false,
                }),
            ),
            Some(Tool::Unavailable) => (
                DiagnosticState::Unknown,
                "Could not check this executable on the selected machine.",
                None,
            ),
            None if reachable
                && (facts.inspection_supported == Some(false)
                    || (id == "claude" && tools.is_some())) =>
            {
                (
                    DiagnosticState::Unknown,
                    "This Host does not advertise this check; update the Host.",
                    None,
                )
            }
            None => (
                DiagnosticState::Unknown,
                "Not checked on this machine.",
                None,
            ),
        };
        rows.push(DiagnosticRow {
            id: id.into(),
            title: title.into(),
            state,
            detail: detail.into(),
            action,
            action_label: None,
        });
    }
    if let Some(tools) = tools.filter(|_| reachable) {
        let checks = tools.setup_checks.as_ref();
        let codex_auth = checks.and_then(|checks| checks.codex_auth);
        let codex_server = checks.and_then(|checks| checks.codex_server_ready);
        let claude_auth = checks.and_then(|checks| checks.claude_auth);
        let tmux = tools.tmux == Tool::Missing;
        let codex_service = tools.codex_control_enabled && (tools.codex == Tool::Missing
            || (tools.codex == Tool::Available && (codex_auth == Some(Auth::SignInRequired)
                || codex_server == Some(false))));
        let codex = tools.codex_control_enabled && (tools.codex == Tool::Missing || codex_service);
        let mut steps = Vec::new();
        if tmux {
            let size = checks.and_then(|checks| checks.tmux_archive_bytes)
                .map(|bytes| format!(" ({:.1} MiB)", bytes as f64 / 1_048_576.0))
                .unwrap_or_default();
            steps.push(format!("Install tmux from the bundled archive{size}"));
        }
        if codex && tools.codex == Tool::Missing {
            steps.push("Install Codex (download size unknown)".into());
        }
        if codex_service {
            steps.push("Connect Codex App Server and sign in if prompted".into());
        }
        if tools.native_setup && !steps.is_empty() {
            rows.insert(0, DiagnosticRow {
                id: "start_setup".into(),
                title: if target == "local" { "Required setup".into() } else { format!("Set up {name}") },
                state: DiagnosticState::NeedsAttention,
                detail: steps.join(". ") + ". Follow the prompts in the Shell Pane.",
                action: Some(DiagnosticAction::PrepareTools {
                    target_id: target.into(), tmux, codex, codex_service, claude: false, claude_login: false,
                }),
                action_label: Some("Start setup".into()),
            });
        }
        let sign_in = (tools.native_setup && codex_auth == Some(Auth::SignInRequired)
            && tools.codex_control_enabled).then(|| DiagnosticAction::PrepareTools {
                target_id: target.into(), tmux: false, codex: true, codex_service: true, claude: false, claude_login: false,
            });
        rows.push(authentication_row("codex_auth", "Codex account", codex_auth, sign_in));
        let (state, detail) = match codex_server {
            Some(true) => (DiagnosticState::Ready, "Connected to the default App Server."),
            Some(false) => (DiagnosticState::NeedsAttention, "Could not connect to the default App Server."),
            None => (DiagnosticState::Unknown, "The default App Server has not been verified."),
        };
        rows.push(DiagnosticRow {
            id: "codex_server".into(), title: "Codex App Server".into(), state, detail: detail.into(),
            action: None, action_label: None,
        });
        let claude_login = (tools.native_setup && claude_auth == Some(Auth::SignInRequired))
            .then(|| DiagnosticAction::PrepareTools {
                target_id: target.into(), tmux: false, codex: false, codex_service: false,
                claude: true, claude_login: true,
            });
        rows.push(authentication_row("claude_auth", "Claude authentication", claude_auth, claude_login));
    }
    rows
}

fn authentication_row(
    id: &str,
    title: &str,
    observation: Option<Auth>,
    action: Option<DiagnosticAction>,
) -> DiagnosticRow {
    let (state, detail) = match observation {
        Some(Auth::Ready) => (DiagnosticState::Ready, "Account is ready."),
        Some(Auth::SignInRequired) => (DiagnosticState::NeedsAttention, "Sign in on this machine."),
        Some(Auth::Unsupported) => (DiagnosticState::Unknown, "This CLI does not support the account check."),
        Some(Auth::CheckFailed) => (DiagnosticState::Unknown, "Could not check account status on this machine."),
        None => (DiagnosticState::Unknown, "Account status has not been checked."),
    };
    let action_label = action.as_ref().map(|_| {
        if id == "claude_auth" { "Sign in to Claude" } else { "Sign in to Codex" }.into()
    });
    DiagnosticRow {
        id: id.into(), title: title.into(), state, detail: detail.into(), action, action_label,
    }
}




