//! Pure readiness composition. Inputs are observations from existing owners;
//! this module does not perform checks, setup, authentication or I/O.
use polar_station_protocol::{ConnectionsProjectionV1, SessionTargetToolsV1};
use polar_station_resources::ResourcesControlProjection;
use serde::{Deserialize, Serialize};

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
    pub storage_writable: Option<bool>,
    pub external_input_requested: bool,
    pub external_input_permission: DiagnosticPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSetupSection {
    Account,
    Microphone,
    Storage,
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
    EditVoiceSetup {
        section: VoiceSetupSection,
    },
    RequestMicrophoneAccess,
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
    local.extend(tool_rows(facts, "local", online));
    local.extend(voice_rows(facts, online));
    local.push(DiagnosticRow {
        id: "resource_settings".into(),
        title: "Resources".into(),
        state: if online {
            DiagnosticState::Ready
        } else {
            DiagnosticState::Unknown
        },
        detail: "Manage the existing resource catalog and its local prerequisites.".into(),
        action: online.then_some(DiagnosticAction::EditResource {
            resource_id: String::new(),
        }),
        action_label: None,
    });
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
            rows.extend(tool_rows(facts, &machine.id, reachable));
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
        row.action_label = row.action.as_ref().map(|action| {
            match action {
                DiagnosticAction::ReconnectHost => "Retry connection",
                DiagnosticAction::AddMachineConnection => "Add remote host",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Account,
                } => "Account settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Microphone,
                } => "Input device settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Storage,
                } => "Recording storage",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::Service,
                } => "Voice service settings",
                DiagnosticAction::EditVoiceSetup {
                    section: VoiceSetupSection::ExternalInput,
                } => "External input settings",
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
    DiagnosticsProjection {
        local_title: "Local Diagnostics".into(),
        remote_title: "Remote Hosts".into(),
        refresh_label: "Refresh".into(),
        empty_remote_label: "No remote hosts configured.".into(),
        local,
        remote_setup,
        remote,
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
        ("voice_storage","Recording storage",voice.storage_writable,VoiceSetupSection::Storage,
         "The recording store is available for saving audio.",
         "Recordings cannot be saved to the configured store. Check its access and available space."),
    ] {
        let (state,detail) = match observation.filter(|_|host_online) {
            Some(true)=>(DiagnosticState::Ready,ready),
            Some(false)=>(missing_state(required),missing),
            None=>(DiagnosticState::Unknown,if host_online {"Not checked on this computer."}else{"Station Host is unreachable; current service status is unknown."}),
        };
        rows.push(DiagnosticRow {id:id.into(),title:title.into(),state,detail:detail.into(),action:Some(DiagnosticAction::EditVoiceSetup {section}),action_label:None});
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

fn tool_rows(facts: &DiagnosticsFacts, target: &str, reachable: bool) -> Vec<DiagnosticRow> {
    use polar_station_protocol::SessionToolStateV1 as Tool;
    let tools = if reachable {
        facts.tools.iter().find(|t| t.execution_target_id == target)
    } else {
        None
    };
    let mut rows = Vec::new();
    for (id, title, state, optional) in [
        ("tmux", "tmux", tools.map(|t| t.tmux), false),
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
    for (id, title) in [
        ("codex_auth", "Codex authentication"),
        ("codex_server", "Codex App Server"),
        ("claude_auth", "Claude authentication"),
    ] {
        rows.push(DiagnosticRow {
            id: id.into(),
            title: title.into(),
            state: DiagnosticState::Unknown,
            detail: "Not verified.".into(),
            action: None,
            action_label: None,
        });
    }
    rows
}




