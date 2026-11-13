//! Stateless composition of source facts, pure parsing, probing and access
//! planning. The durable-in-flight UI lifecycle remains in FileOpenFlow.
use super::*;
use polar_tmux_resources::{
    FileOpenFailure, FileOpenResolution, FileOpenStage, FileReferenceInput, ResolvedFile,
    ResolvedFileSource, SourcePath,
};

impl ResourceService {
    pub(super) async fn resolve_file_open(
        &self,
        request: ResourcesRequestEnvelopeV1,
        source: Result<ResolvedFileSource, FileOpenFailure>,
        input: FileReferenceInput,
        #[cfg(feature = "codex-control")] recovery: Option<(
            &super::super::codex_control::CodexControlService,
            polar_tmux_workspace::SessionKey,
        )>,
    ) -> ResourcesResponseEnvelopeV1 {
        let resolution = self
            .resolve_open_operations(
                source,
                input,
                #[cfg(feature = "codex-control")]
                recovery,
            )
            .await
            .unwrap_or_else(|failure| FileOpenResolution::Failure { failure });
        self.file_open_response(request, resolution).await
    }

    async fn resolve_open_operations(
        &self,
        source: Result<ResolvedFileSource, FileOpenFailure>,
        input: FileReferenceInput,
        #[cfg(feature = "codex-control")] recovery: Option<(
            &super::super::codex_control::CodexControlService,
            polar_tmux_workspace::SessionKey,
        )>,
    ) -> Result<FileOpenResolution, FileOpenFailure> {
        let source = source?;
        self.check_file_source(&source)?;
        let parsed = polar_tmux_resources::parse_file_input(&input, source.flavor)?;
        let candidates = resolve_references(&source, parsed.clone())?;
        let mut files = self
            .existing_candidate_files(&source.origin.execution_target_id, &candidates)
            .await
            .map_err(|e| {
                FileOpenFailure::new(FileOpenStage::Verify, "source_file_check_failed", e)
            })?;
        #[cfg(feature = "codex-control")]
        if files.is_empty() {
            if let Some((codex, session)) = recovery {
                let mut recovered = Vec::new();
                for mut reference in parsed {
                    if reference.path.starts_with('/')
                        || reference.path.starts_with("./")
                        || reference.path.starts_with("../")
                        || reference.path.starts_with("~/")
                    {
                        continue;
                    }
                    if let Some((origin, path)) = codex
                        .terminal_file_reference(&session, &reference.path)
                        .await
                        .map_err(|e| {
                            FileOpenFailure::new(FileOpenStage::Source, &e.code, e.message)
                        })?
                    {
                        if origin.execution_target_id != source.origin.execution_target_id
                            || origin.cwd != source.origin.cwd
                        {
                            return Err(FileOpenFailure::new(
                                FileOpenStage::Source,
                                "source_changed",
                                "The original Thread source changed; retry.",
                            ));
                        }
                        reference.path = path;
                        if !recovered.contains(&reference) {
                            recovered.push(reference);
                        }
                    }
                }
                if !recovered.is_empty() {
                    files = self
                        .existing_candidate_files(
                            &source.origin.execution_target_id,
                            &resolve_references(&source, recovered)?,
                        )
                        .await
                        .map_err(|e| {
                            FileOpenFailure::new(
                                FileOpenStage::Verify,
                                "source_file_check_failed",
                                e,
                            )
                        })?;
                }
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files.dedup_by(|a, b| a.path == b.path);
        match files.len() {
            0 => Err(FileOpenFailure::new(
                FileOpenStage::Verify,
                "file_not_found",
                "No matching file exists on the source machine.",
            )),
            1 => {
                self.access_resolution(ResolvedFile {
                    source,
                    reference: files.into_iter().next().unwrap(),
                })
                .await
            }
            _ => Ok(FileOpenResolution::Choices { source, files }),
        }
    }

    pub(super) async fn replan_file_access(
        &self,
        request: ResourcesRequestEnvelopeV1,
        file: ResolvedFile,
    ) -> ResourcesResponseEnvelopeV1 {
        let resolution = async {
            self.check_resolved_file(&file)?;
            if !self
                .current_targets()
                .contains_key(&file.source.origin.execution_target_id)
            {
                return Err(FileOpenFailure::new(
                    FileOpenStage::Source,
                    "source_changed",
                    "The source machine is unavailable.",
                ));
            }
            let files = self
                .existing_candidate_files(
                    &file.source.origin.execution_target_id,
                    std::slice::from_ref(&file.reference),
                )
                .await
                .map_err(|e| {
                    FileOpenFailure::new(FileOpenStage::Verify, "source_file_check_failed", e)
                })?;
            if files.is_empty() {
                return Err(FileOpenFailure::new(
                    FileOpenStage::Verify,
                    "file_not_found",
                    "The source file is no longer available.",
                ));
            }
            self.access_resolution(file).await
        }
        .await
        .unwrap_or_else(|failure| FileOpenResolution::Failure { failure });
        self.file_open_response(request, resolution).await
    }

    pub(super) async fn access_resolution(
        &self,
        file: ResolvedFile,
    ) -> Result<FileOpenResolution, FileOpenFailure> {
        self.check_resolved_file(&file)?;
        let catalog = self.catalog.lock().await;
        let statuses = self.statuses.lock().await;
        let local =
            file.source.origin.execution_target_id == crate::target_registry::LOCAL_TARGET_ID;
        let access_plan = polar_tmux_resources::plan_file_access(&file, &catalog, &statuses, local)
            .map_err(|e| {
                FileOpenFailure::new(FileOpenStage::Access, "file_access_unavailable", e)
            })?;
        let access_plan = materialize_local_access(access_plan).map_err(|message| {
            FileOpenFailure::new(
                FileOpenStage::Access,
                "local_mount_path_unavailable",
                message,
            )
        })?;
        Ok(FileOpenResolution::Resolved { file, access_plan })
    }

    pub(super) fn file_source_revision(&self, target: &str) -> Option<u64> {
        self.target_registry
            .as_ref()?
            .get(target)
            .map(|t| t.configuration_revision())
    }
    fn check_file_source(&self, source: &ResolvedFileSource) -> Result<(), FileOpenFailure> {
        if !self
            .current_targets()
            .contains_key(&source.origin.execution_target_id)
            || self.file_source_revision(&source.origin.execution_target_id)
                != source.target_configuration_revision
        {
            return Err(FileOpenFailure::new(
                FileOpenStage::Source,
                "source_changed",
                "The source connection changed; retry the original file.",
            ));
        }
        Ok(())
    }
    fn check_resolved_file(&self, file: &ResolvedFile) -> Result<(), FileOpenFailure> {
        self.check_file_source(&file.source)?;
        if file.source.flavor != polar_tmux_resources::PathFlavor::Posix {
            return Err(FileOpenFailure::new(
                FileOpenStage::Source,
                "source_platform_unsupported",
                "This Host has no file provider for that source platform.",
            ));
        }
        let path = SourcePath::new(file.source.flavor, &file.reference.path)
            .map_err(|e| FileOpenFailure::new(FileOpenStage::Parse, "source_path_invalid", e))?;
        if path.form() != polar_tmux_resources::PathForm::Absolute {
            return Err(FileOpenFailure::new(
                FileOpenStage::Parse,
                "unresolved_source_path",
                "A resolved source file must have an absolute path.",
            ));
        }
        Ok(())
    }
    async fn file_open_response(
        &self,
        request: ResourcesRequestEnvelopeV1,
        resolution: FileOpenResolution,
    ) -> ResourcesResponseEnvelopeV1 {
        ResourcesResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: request.api,
            request_id: request.request_id,
            revision: self.catalog.lock().await.revision,
            result: ResourcesResultV1::FileOpen { resolution },
        }
    }
}

pub(super) fn resolve_references(
    source: &ResolvedFileSource,
    parsed: Vec<polar_tmux_resources::FileReference>,
) -> Result<Vec<polar_tmux_resources::FileReference>, FileOpenFailure> {
    let mut candidates = Vec::new();
    for mut reference in parsed {
        reference.path = SourcePath::new(source.flavor, &reference.path)
            .and_then(|p| p.resolve(source.origin.cwd.as_deref(), source.home.as_deref()))
            .map_err(|e| FileOpenFailure::new(FileOpenStage::Parse, "source_path_invalid", e))?
            .text;
        if !candidates.contains(&reference) {
            candidates.push(reference);
        }
    }
    Ok(candidates)
}

pub(super) fn materialize_local_access(
    plan: polar_tmux_resources::FileAccessPlan,
) -> Result<polar_tmux_resources::FileAccessPlan, String> {
    use polar_tmux_resources::FileAccessPlan;
    if let FileAccessPlan::MappedReady {
        resource_id,
        local_path,
    } = plan
    {
        // This is the desktop account's mount path, not the source account's
        // Home. Use the exact resolver used by the Resource provider.
        let path = super::super::resource_provider::configured_mount_path(&local_path)
            .map_err(|_| "The desktop account Home is unavailable.")?;
        if !path.is_absolute() {
            return Err("The local mapping path is not absolute.".into());
        }
        Ok(FileAccessPlan::MappedReady {
            resource_id,
            local_path: path
                .to_str()
                .ok_or("The local mapping path is not UTF-8.")?
                .into(),
        })
    } else {
        Ok(plan)
    }
}

pub(super) fn legacy_access(
    plan: polar_tmux_resources::FileAccessPlan,
) -> polar_tmux_resources::FileAccess {
    use polar_tmux_resources::{FileAccess, FileAccessPlan};
    match plan {
        FileAccessPlan::NativeLocal { local_path } => FileAccess::LocalFile { local_path },
        FileAccessPlan::MappedReady {
            resource_id,
            local_path,
        } => FileAccess::Ready {
            resource_id,
            local_path,
        },
        FileAccessPlan::StartExistingMapping { resource_id } => {
            FileAccess::StartResource { resource_id }
        }
        FileAccessPlan::NeedMapping {
            execution_target_id,
            remote_directory,
            source_path,
        } => FileAccess::CreateMapping {
            execution_target_id,
            remote_directory,
            source_path,
        },
    }
}


