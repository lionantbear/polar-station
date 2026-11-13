//! One pure workflow for a file-open intent. Source lookup, parsing, probing,
//! Resource execution and OS opening are separate operations with typed results.
use crate::{
    FileAccess, FileAccessPlan, FileOpenFailure, FileOpenResolution, FileOpenStage, FileReference,
    FileReferenceInput, OpenStrategy, ResolvedFile, ResolvedFileSource, ResourceId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOrigin {
    pub execution_target_id: String,
    pub cwd: Option<String>,
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
    Resolved {
        request_id: String,
        resolution: FileOpenResolution,
    },
    Choose {
        request_id: String,
        index: Option<usize>,
    },
    MappingDecision {
        request_id: String,
        confirmed: bool,
    },
    Refresh {
        request_id: String,
    },
    Retry {
        request_id: String,
    },
    Failed {
        request_id: String,
        message: String,
    },
    ViewerFinished {
        request_id: String,
        accepted: bool,
    },
    Cancel,
    CancelRequest {
        request_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileOpenEffect {
    ResolveOpen {
        request_id: String,
        origin: FileOrigin,
        input: FileReferenceInput,
    },
    PlanAccess {
        request_id: String,
        file: ResolvedFile,
    },
    ChooseFile {
        request_id: String,
        source: ResolvedFileSource,
        candidates: Vec<FileReference>,
    },
    ConfirmMapping {
        request_id: String,
        target: String,
        path: String,
        directory: String,
    },
    StartResource {
        resource_id: ResourceId,
    },
    CreateMapping {
        access: FileAccess,
    },
    OpenLocalFile {
        request_id: String,
        path: String,
        reference: FileReference,
        strategy: OpenStrategy,
    },
    ShowError {
        request_id: String,
        stage: FileOpenStage,
        code: String,
        message: String,
        can_retry: bool,
    },
    ShowProgress {
        request_id: String,
        message: String,
    },
    DismissProgress {
        request_id: String,
    },
}
#[derive(Clone)]
enum MappingWait {
    Starting(ResourceId),
    Confirmation(FileAccessPlan),
    Configuring,
}
#[derive(Clone)]
enum Phase {
    Resolving,
    Choosing {
        source: ResolvedFileSource,
        files: Vec<FileReference>,
    },
    Planning {
        file: ResolvedFile,
        previous: Option<MappingWait>,
    },
    Waiting {
        file: ResolvedFile,
        action: MappingWait,
    },
    Opening,
    Failed,
}
struct Pending {
    request_id: String,
    origin: FileOrigin,
    input: FileReferenceInput,
    phase: Phase,
    refresh_pending: Option<String>,
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
        self.pending
            .as_ref()
            .is_some_and(|p| !matches!(p.phase, Phase::Failed))
    }
    fn fail(p: &mut Pending, failure: FileOpenFailure) -> Vec<FileOpenEffect> {
        p.phase = Phase::Failed;
        vec![
            FileOpenEffect::DismissProgress {
                request_id: p.request_id.clone(),
            },
            FileOpenEffect::ShowError {
                request_id: p.request_id.clone(),
                stage: failure.stage,
                code: failure.code,
                message: failure.message,
                can_retry: true,
            },
        ]
    }
    fn resolve(p: &mut Pending, request_id: String) -> Vec<FileOpenEffect> {
        p.request_id = request_id;
        p.phase = Phase::Resolving;
        p.refresh_pending = None;
        vec![
            FileOpenEffect::ShowProgress {
                request_id: p.request_id.clone(),
                message: "Checking the source file…".into(),
            },
            FileOpenEffect::ResolveOpen {
                request_id: p.request_id.clone(),
                origin: p.origin.clone(),
                input: p.input.clone(),
            },
        ]
    }
    fn plan(
        p: &mut Pending,
        request_id: String,
        file: ResolvedFile,
        previous: Option<MappingWait>,
    ) -> Vec<FileOpenEffect> {
        p.request_id = request_id;
        p.phase = Phase::Planning {
            file: file.clone(),
            previous,
        };
        vec![
            FileOpenEffect::ShowProgress {
                request_id: p.request_id.clone(),
                message: "Checking file access…".into(),
            },
            FileOpenEffect::PlanAccess {
                request_id: p.request_id.clone(),
                file,
            },
        ]
    }
    pub fn dispatch(&mut self, input: FileOpenInput) -> Vec<FileOpenEffect> {
        if let FileOpenInput::Open {
            request_id,
            text,
            origin,
        } = input
        {
            let mut effects = self.dispatch(FileOpenInput::Cancel);
            let parsed = FileReferenceInput::from_capture(text);
            let parsed = match parsed {
                Ok(value) if !request_id.is_empty() && !origin.execution_target_id.is_empty() => {
                    value
                }
                other => {
                    let failure = other.err().unwrap_or_else(|| {
                        FileOpenFailure::new(
                            FileOpenStage::Source,
                            "source_unavailable",
                            "The source Session is unavailable.",
                        )
                    });
                    effects.push(FileOpenEffect::ShowError {
                        request_id,
                        stage: failure.stage,
                        code: failure.code,
                        message: failure.message,
                        can_retry: false,
                    });
                    return effects;
                }
            };
            let mut pending = Pending {
                request_id: request_id.clone(),
                origin,
                input: parsed,
                phase: Phase::Resolving,
                refresh_pending: None,
            };
            effects.extend(Self::resolve(&mut pending, request_id));
            self.pending = Some(pending);
            return effects;
        }
        let Some(mut p) = self.pending.take() else {
            return vec![];
        };
        let mut effects = match input {
            FileOpenInput::Open { .. } => unreachable!(),
            FileOpenInput::Cancel => {
                return vec![FileOpenEffect::DismissProgress {
                    request_id: p.request_id,
                }]
            }
            FileOpenInput::CancelRequest { request_id } if p.request_id == request_id => {
                return vec![FileOpenEffect::DismissProgress { request_id }];
            }
            FileOpenInput::Retry { request_id }
                if p.request_id == request_id && matches!(p.phase, Phase::Failed) =>
            {
                Self::resolve(&mut p, format!("{request_id}-retry"))
            }
            FileOpenInput::Refresh { request_id } => match p.phase.clone() {
                Phase::Resolving | Phase::Planning { .. } => {
                    p.refresh_pending = Some(request_id);
                    vec![]
                }
                Phase::Waiting {
                    file,
                    action: MappingWait::Starting(_) | MappingWait::Configuring,
                } => {
                    let Phase::Waiting { action, .. } = p.phase.clone() else {
                        unreachable!()
                    };
                    Self::plan(&mut p, request_id, file, Some(action))
                }
                _ => vec![],
            },
            FileOpenInput::Choose { request_id, index } if p.request_id == request_id => {
                if let Phase::Choosing { source, files } = p.phase.clone() {
                    let Some(index) = index else {
                        return vec![FileOpenEffect::DismissProgress { request_id }];
                    };
                    if let Some(reference) = files.get(index) {
                        Self::plan(
                            &mut p,
                            format!("{request_id}-choice"),
                            ResolvedFile {
                                source,
                                reference: reference.clone(),
                            },
                            None,
                        )
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            }
            FileOpenInput::MappingDecision {
                request_id,
                confirmed,
            } if p.request_id == request_id => {
                if let Phase::Waiting {
                    file,
                    action:
                        MappingWait::Confirmation(FileAccessPlan::NeedMapping {
                            execution_target_id,
                            remote_directory,
                            source_path,
                        }),
                } = p.phase.clone()
                {
                    if !confirmed {
                        return vec![FileOpenEffect::DismissProgress { request_id }];
                    }
                    p.phase = Phase::Waiting {
                        file,
                        action: MappingWait::Configuring,
                    };
                    vec![FileOpenEffect::CreateMapping {
                        access: FileAccess::CreateMapping {
                            execution_target_id,
                            remote_directory,
                            source_path,
                        },
                    }]
                } else {
                    vec![]
                }
            }
            FileOpenInput::Failed {
                request_id,
                message,
            } if p.request_id == request_id && !matches!(p.phase, Phase::Failed) => Self::fail(
                &mut p,
                FileOpenFailure::new(FileOpenStage::Source, "file_transport_failed", message),
            ),
            FileOpenInput::ViewerFinished {
                request_id,
                accepted,
            } if p.request_id == request_id && matches!(p.phase, Phase::Opening) => {
                if accepted {
                    return vec![FileOpenEffect::DismissProgress { request_id }];
                }
                Self::fail(&mut p, FileOpenFailure::new(FileOpenStage::Viewer, "viewer_failed", "The system application did not accept the file. Check its file association and retry."))
            }
            FileOpenInput::Resolved {
                request_id,
                resolution,
            } if p.request_id == request_id
                && matches!(p.phase, Phase::Resolving | Phase::Planning { .. }) =>
            {
                let (expected, previous) = match &p.phase {
                    Phase::Planning { file, previous } => (Some(file.clone()), previous.clone()),
                    _ => (None, None),
                };
                match resolution {
                    FileOpenResolution::Failure { failure } => Self::fail(&mut p, failure),
                    FileOpenResolution::Choices { source, files } => {
                        p.phase = Phase::Choosing {
                            source: source.clone(),
                            files: files.clone(),
                        };
                        vec![
                            FileOpenEffect::DismissProgress {
                                request_id: request_id.clone(),
                            },
                            FileOpenEffect::ChooseFile {
                                request_id,
                                source,
                                candidates: files,
                            },
                        ]
                    }
                    FileOpenResolution::Resolved { file, access_plan }
                        if expected.as_ref().is_some_and(|expected| expected != &file) =>
                    {
                        let _ = access_plan;
                        Self::fail(
                            &mut p,
                            FileOpenFailure::new(
                                FileOpenStage::Source,
                                "source_changed",
                                "The original source file changed while opening; retry.",
                            ),
                        )
                    }
                    FileOpenResolution::Resolved { file, access_plan } => match access_plan {
                        FileAccessPlan::NativeLocal { local_path }
                        | FileAccessPlan::MappedReady { local_path, .. } => {
                            let strategy = crate::select_opener(&file.reference, self.rules);
                            p.phase = Phase::Opening;
                            vec![
                                FileOpenEffect::DismissProgress {
                                    request_id: request_id.clone(),
                                },
                                FileOpenEffect::OpenLocalFile {
                                    request_id,
                                    path: local_path,
                                    reference: file.reference,
                                    strategy,
                                },
                            ]
                        }
                        FileAccessPlan::StartExistingMapping { resource_id } => {
                            let already = matches!(previous, Some(MappingWait::Starting(id)) if id == resource_id);
                            p.phase = Phase::Waiting {
                                file,
                                action: MappingWait::Starting(resource_id),
                            };
                            if already {
                                vec![]
                            } else {
                                vec![
                                    FileOpenEffect::ShowProgress {
                                        request_id,
                                        message: "Connecting to the source files…".into(),
                                    },
                                    FileOpenEffect::StartResource { resource_id },
                                ]
                            }
                        }
                        plan @ FileAccessPlan::NeedMapping { .. } => {
                            if matches!(previous, Some(MappingWait::Configuring)) {
                                p.phase = Phase::Waiting {
                                    file,
                                    action: MappingWait::Configuring,
                                };
                                vec![FileOpenEffect::DismissProgress { request_id }]
                            } else {
                                let FileAccessPlan::NeedMapping {
                                    execution_target_id,
                                    remote_directory,
                                    source_path,
                                } = &plan
                                else {
                                    unreachable!()
                                };
                                let effects = vec![
                                    FileOpenEffect::DismissProgress {
                                        request_id: request_id.clone(),
                                    },
                                    FileOpenEffect::ConfirmMapping {
                                        request_id,
                                        target: execution_target_id.clone(),
                                        path: source_path.clone(),
                                        directory: remote_directory.clone(),
                                    },
                                ];
                                p.phase = Phase::Waiting {
                                    file,
                                    action: MappingWait::Confirmation(plan),
                                };
                                effects
                            }
                        }
                    },
                }
            }
            _ => vec![],
        };
        if let Phase::Waiting {
            file,
            action: action @ (MappingWait::Starting(_) | MappingWait::Configuring),
        } = p.phase.clone()
        {
            if let Some(request_id) = p.refresh_pending.take() {
                // A notification can precede the response to an older snapshot.
                // Keep one latest intent, not a timer or a second request queue.
                let not_started = effects
                    .iter()
                    .any(|e| matches!(e, FileOpenEffect::StartResource { .. }));
                effects.retain(|e| !matches!(e, FileOpenEffect::StartResource { .. }));
                effects.extend(Self::plan(
                    &mut p,
                    request_id,
                    file,
                    (!not_started).then_some(action),
                ));
            }
        } else if !matches!(p.phase, Phase::Resolving | Phase::Planning { .. }) {
            p.refresh_pending = None;
        }
        self.pending = Some(p);
        effects
    }
}


