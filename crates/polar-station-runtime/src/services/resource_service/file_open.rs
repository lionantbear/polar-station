//! Stateless composition of source facts, pure parsing, probing and access
//! planning. The durable-in-flight UI lifecycle remains in FileOpenFlow.
use super::*;
use polar_station_resources::{
    FileOpenContext, FileOpenFailure, FileOpenResolution, FileOpenStage, FileReferenceInput,
    ResolvedFile, ResolvedFileSource, SourcePath,
};

impl ResourceService {
    pub(super) async fn resolve_file_open(
        &self,
        request: ResourcesRequestEnvelopeV1,
        source: Result<ResolvedFileSource, FileOpenFailure>,
        input: FileReferenceInput,
    ) -> ResourcesResponseEnvelopeV1 {
        let resolution = self
            .resolve_open_operations(
                source,
                input,
            )
            .await
            .unwrap_or_else(|failure| FileOpenResolution::Failure { failure });
        self.file_open_response(request, resolution).await
    }

    async fn resolve_open_operations(
        &self,
        source: Result<ResolvedFileSource, FileOpenFailure>,
        input: FileReferenceInput,
    ) -> Result<FileOpenResolution, FileOpenFailure> {
        let captured_text = input.text().to_owned();
        let source = source.map_err(|mut failure| { failure.captured_text = Some(captured_text.clone()); failure })?;
        let mut context = FileOpenContext {
            origin: source.origin.clone(),
            paths: vec![],
            local_path: None,
            checked_path: None,
        };
        let result = async {
            self.check_file_source(&source)?;
            let recognized = polar_station_resources::recognize_file_input(&input, source.flavor)?;
            context.paths = vec![recognized.reference.path.clone()];
            let references = resolve_references(&source, vec![recognized.reference])?;
            for reference in &references {
                if !context.paths.contains(&reference.path) { context.paths.push(reference.path.clone()); }
            }
            context.checked_path = references.first().map(|reference| reference.path.clone());
            let files = self.existing_candidate_files(&source.origin.execution_target_id, &references)
                .await.map_err(|e| FileOpenFailure::new(FileOpenStage::Verify, "source_file_check_failed", e))?;
            let reference = files.into_iter().next().ok_or_else(|| FileOpenFailure::new(
                FileOpenStage::Verify, "file_not_found", "The recognized file does not exist on the source machine."))?;
            self.access_resolution(ResolvedFile { source, reference }).await
        }
        .await;
        result.map_err(|mut failure| {
            failure.captured_text = Some(captured_text);
            failure.with_context(context)
        })
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
            let context = FileOpenContext {
                origin: file.source.origin.clone(),
                paths: vec![file.reference.path.clone()],
                local_path: None,
                checked_path: Some(file.reference.path.clone()),
            };
            let files = self
                .existing_candidate_files(
                    &file.source.origin.execution_target_id,
                    std::slice::from_ref(&file.reference),
                )
                .await
                .map_err(|e| {
                    FileOpenFailure::new(FileOpenStage::Verify, "source_file_check_failed", e)
                        .with_context(context.clone())
                })?;
            if files.is_empty() {
                return Err(FileOpenFailure::new(
                    FileOpenStage::Verify,
                    "file_not_found",
                    "The source file is no longer available.",
                ).with_context(context));
            }
            self.access_resolution(file).await.map_err(|failure| failure.with_context(context))
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
        let catalog = self.catalog.lock().await.clone();
        let statuses = self.statuses.lock().await.clone();
        let local =
            file.source.origin.execution_target_id == crate::target_registry::LOCAL_TARGET_ID;
        let access_plan = polar_station_resources::plan_file_access(&file, &catalog, &statuses, local)
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
        self.verify_ready_file_access(&access_plan, &catalog, &statuses).await.map_err(|failure| {
            let local_path = match &access_plan {
                polar_station_resources::FileAccessPlan::NativeLocal { local_path }
                | polar_station_resources::FileAccessPlan::MappedReady { local_path, .. } => Some(local_path.clone()),
                _ => None,
            };
            failure.with_context(FileOpenContext {
                origin: file.source.origin.clone(),
                paths: vec![file.reference.path.clone()],
                local_path,
                checked_path: Some(file.reference.path.clone()),
            })
        })?;
        self.check_resolved_file(&file)?;
        Ok(FileOpenResolution::Resolved { file, access_plan })
    }

    pub(super) async fn verify_ready_file_access(
        &self,
        plan: &polar_station_resources::FileAccessPlan,
        catalog: &ResourceCatalog,
        statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
    ) -> Result<(), FileOpenFailure> {
        use polar_station_resources::FileAccessPlan;
        let path = match plan {
            FileAccessPlan::NativeLocal {local_path} | FileAccessPlan::MappedReady {local_path,..} => local_path,
            _ => return Ok(()),
        };
        if let FileAccessPlan::MappedReady {resource_id,..} = plan {
            let changed = || FileOpenFailure::new(FileOpenStage::Access,"resource_mapping_changed",
                "The file mapping changed or is unavailable; retry the original file.");
            let spec = catalog.file_mappings.iter().find(|m| m.id == *resource_id).ok_or_else(changed)?;
            let generation = statuses.get(resource_id).ok_or_else(changed)?.generation;
            let recovery = self.recovery.as_ref().ok_or_else(changed)?;
            recovery.verify_file_access(spec,generation).await.map_err(|error|
                FileOpenFailure::new(FileOpenStage::Access,error.code,error.message))?;
            if self.catalog.lock().await.file_mappings.iter().find(|m|m.id == *resource_id) != Some(spec) {
                return Err(changed());
            }
        }
        // Validate the provider before touching a possibly offline mount.
        // Reuse the original regular-file and read-open probe, including for
        // native local files, rather than accepting a cached access plan.
        let found = self.existing_candidate_files(crate::target_registry::LOCAL_TARGET_ID,
            &[polar_station_resources::FileReference {path:path.clone(),location:None}]).await
            .map_err(|message| FileOpenFailure::new(FileOpenStage::Access,"desktop_file_check_failed",message))?;
        if found.is_empty() {
            return Err(FileOpenFailure::new(FileOpenStage::Access,"desktop_file_unavailable",
                "The file is unavailable at the desktop path. Check the file and, for remote files, reconnect its Resource mapping."));
        }
        Ok(())
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
        if file.source.flavor != polar_station_resources::PathFlavor::Posix {
            return Err(FileOpenFailure::new(
                FileOpenStage::Source,
                "source_platform_unsupported",
                "This Host has no file provider for that source platform.",
            ));
        }
        let path = SourcePath::new(file.source.flavor, &file.reference.path)
            .map_err(|e| FileOpenFailure::new(FileOpenStage::Parse, "source_path_invalid", e))?;
        if path.form() != polar_station_resources::PathForm::Absolute {
            return Err(FileOpenFailure::new(
                FileOpenStage::Parse,
                "unresolved_source_path",
                "A resolved source file must have an absolute path.",
            ));
        }
        Ok(())
    }
    pub(super) async fn file_open_response(
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
    parsed: Vec<polar_station_resources::FileReference>,
) -> Result<Vec<polar_station_resources::FileReference>, FileOpenFailure> {
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
    plan: polar_station_resources::FileAccessPlan,
) -> Result<polar_station_resources::FileAccessPlan, String> {
    use polar_station_resources::FileAccessPlan;
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
    plan: polar_station_resources::FileAccessPlan,
) -> polar_station_resources::FileAccess {
    use polar_station_resources::{FileAccess, FileAccessPlan};
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


