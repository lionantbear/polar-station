//! Shared application flow migrated from the native pending-file adapters.
//! No IO, protocol client, window geometry, downloader or persistent state.
use crate::{FileAccess, FileReference, OpenStrategy, ResourceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOrigin {
    pub execution_target_id: String,
    pub cwd: Option<String>,
    /// Existing terminal session identity, used by Host harness adapters.
    #[serde(default)]
    pub terminal_session_id: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileOpenInput {
    Open {
        request_id: String,
        text: String,
        origin: FileOrigin,
    },
    Refresh {
        request_id: String,
    },
    Resolved {
        request_id: String,
        access: FileAccess,
    },
    Failed {
        request_id: String,
        message: String,
    },
    ViewerFailed {
        path: String,
    },
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileOpenEffect {
    Resolve {
        request_id: String,
        origin: FileOrigin,
        path: String,
    },
    StartResource {
        resource_id: ResourceId,
    },
    CreateMapping {
        access: FileAccess,
    },
    OpenLocalFile {
        path: String,
        reference: FileReference,
        strategy: OpenStrategy,
    },
    ShowError {
        message: String,
    },
}
struct Pending {
    request_id: String,
    reference: FileReference,
    origin: FileOrigin,
    awaiting: bool,
    started: Option<ResourceId>,
    mapping_shown: bool,
}
#[derive(Default)]
pub struct FileOpenFlow {
    pending: Option<Pending>,
    rules: &'static [crate::FileTypeRule<'static>],
}
impl FileOpenFlow {
    pub fn with_rules(rules: &'static [crate::FileTypeRule<'static>]) -> Self {
        Self {
            pending: None,
            rules,
        }
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    fn request(pending: &mut Pending, request_id: String) -> FileOpenEffect {
        pending.request_id.clone_from(&request_id);
        pending.awaiting = true;
        FileOpenEffect::Resolve {
            request_id,
            origin: pending.origin.clone(),
            path: pending.reference.path.clone(),
        }
    }
    pub fn dispatch(&mut self, input: FileOpenInput) -> Vec<FileOpenEffect> {
        match input {
            FileOpenInput::Open { request_id, text, origin } => {
                self.pending = None;
                let Some(reference) = crate::parse_file_reference(&text) else {
                    return vec![FileOpenEffect::ShowError { message: "Select a filesystem file reference.".into() }];
                };
                if request_id.is_empty() || origin.execution_target_id.is_empty() {
                    return vec![FileOpenEffect::ShowError { message: "The source session is unavailable.".into() }];
                }
                let mut pending = Pending { request_id: String::new(), reference, origin, awaiting:false, started:None, mapping_shown:false };
                let effect = Self::request(&mut pending, request_id);
                self.pending = Some(pending);
                vec![effect]
            }
            FileOpenInput::Refresh { request_id } => {
                let Some(pending) = self.pending.as_mut().filter(|p| !p.awaiting) else { return vec![] };
                vec![Self::request(pending, request_id)]
            }
            FileOpenInput::Resolved { request_id, access } => {
                let Some(pending) = self.pending.as_mut().filter(|p| p.awaiting && p.request_id == request_id) else { return vec![] };
                pending.awaiting = false;
                match access {
                    FileAccess::Ready { local_path, .. } => {
                        let reference = pending.reference.clone();
                        self.pending = None;
                        let strategy = crate::select_opener(&reference, self.rules);
                        vec![FileOpenEffect::OpenLocalFile { path:local_path, reference, strategy }]
                    }
                    FileAccess::StartResource { resource_id } if pending.started != Some(resource_id) => {
                        pending.started = Some(resource_id);
                        vec![FileOpenEffect::StartResource { resource_id }]
                    }
                    access @ FileAccess::CreateMapping { .. } if !pending.mapping_shown => {
                        pending.mapping_shown = true;
                        vec![FileOpenEffect::CreateMapping { access }]
                    }
                    _ => vec![],
                }
            }
            FileOpenInput::Failed { request_id, message } => {
                if self.pending.as_ref().is_none_or(|p| p.request_id != request_id) { return vec![] }
                self.pending = None;
                vec![FileOpenEffect::ShowError { message }]
            }
            FileOpenInput::ViewerFailed { path } => vec![FileOpenEffect::ShowError { message:format!("The system application could not open {path}. Choose an application or reveal the file in Resources.") }],
            FileOpenInput::Cancel => { self.pending = None; vec![] }
        }
    }
}

