//! Filesystem validation uses the original Host target and its verified transport.
use super::*;
use polar_tmux_resources::{FileAccess, FileOrigin, FileReference, PathFlavor, ResolvedFileSource};
impl ResourceService {
    pub(super) async fn resolve_file_candidates(
        &self,
        request: ResourcesRequestEnvelopeV1,
        origin: FileOrigin,
        candidates: Vec<FileReference>,
    ) -> ResourcesResponseEnvelopeV1 {
        if origin.terminal_session_id.is_some()
            || !self
                .current_targets()
                .contains_key(&origin.execution_target_id)
        {
            return self
                .error_response(
                    request.request_id,
                    "resource_session_origin_unavailable",
                    "The original file source is unavailable.",
                )
                .await;
        }
        let source = ResolvedFileSource {
            target_configuration_revision: self.file_source_revision(&origin.execution_target_id),
            origin: origin.clone(),
            flavor: PathFlavor::Posix,
            home: None,
        };
        let refs = match super::file_open::resolve_references(&source, candidates) {
            Ok(refs) => refs,
            Err(failure) => {
                return self
                    .error_response(request.request_id, "resource_file_invalid", failure.message)
                    .await
            }
        };
        let existing = match self
            .existing_candidate_files(&origin.execution_target_id, &refs)
            .await
        {
            Ok(files) => files,
            Err(message) => {
                return self
                    .error_response(request.request_id, "resource_file_check_failed", message)
                    .await
            }
        };
        if existing.is_empty() {
            return self.error_response(request.request_id,"resource_file_not_found","No matching file exists on the source machine. Select the complete path and try again.").await;
        }
        if existing.len() > 1 {
            let revision = self.catalog.lock().await.revision;
            return ResourcesResponseEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: request.api,
                request_id: request.request_id,
                revision,
                result: ResourcesResultV1::FileAccess {
                    access: FileAccess::ChooseFile {
                        origin,
                        candidates: existing,
                    },
                },
            };
        }
        let reference = existing.into_iter().next().unwrap();
        match self
            .access_resolution(polar_tmux_resources::ResolvedFile {
                source,
                reference: reference.clone(),
            })
            .await
        {
            Ok(polar_tmux_resources::FileOpenResolution::Resolved { access_plan, .. }) => {
                ResourcesResponseEnvelopeV1 {
                    protocol_version: PROTOCOL_VERSION,
                    api: request.api,
                    request_id: request.request_id,
                    revision: self.catalog.lock().await.revision,
                    result: ResourcesResultV1::FileAccess {
                        access: FileAccess::Verified {
                            reference,
                            access: Box::new(super::file_open::legacy_access(access_plan)),
                        },
                    },
                }
            }
            Err(failure) => {
                self.error_response(request.request_id, "resource_file_invalid", failure.message)
                    .await
            }
            _ => unreachable!("access planning does not choose files"),
        }
    }

    pub(super) async fn existing_candidate_files(
        &self,
        target_id: &str,
        candidates: &[FileReference],
    ) -> Result<Vec<FileReference>, &'static str> {
        if target_id == crate::target_registry::LOCAL_TARGET_ID {
            let candidates = candidates.to_vec();
            return tokio::task::spawn_blocking(move || {
                let mut existing = Vec::new();
                for mut reference in candidates {
                    match std::fs::metadata(&reference.path) {
                        Ok(metadata) if metadata.is_file() => {
                            std::fs::File::open(&reference.path)
                                .map_err(|_| "Permission denied while checking a source file.")?;
                            if has_parent_component(&reference.path) {
                                reference.path = std::fs::canonicalize(&reference.path)
                                    .map_err(|_| "Cannot resolve the source file path.")?
                                    .to_str()
                                    .ok_or("The source path is not UTF-8.")?
                                    .to_owned();
                            }
                            if !existing.contains(&reference) {
                                existing.push(reference);
                            }
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => {
                            return Err(
                                "Cannot check files on the source machine; check permissions.",
                            )
                        }
                    }
                }
                Ok(existing)
            })
            .await
            .map_err(|_| "File checking was interrupted.")?;
        }
        let registry = self
            .target_registry
            .as_ref()
            .ok_or("The source machine is unavailable.")?;
        let ssh = self
            .candidate_ssh
            .as_ref()
            .ok_or("The source connection is unavailable.")?;
        let target = registry
            .get(target_id)
            .ok_or("The source machine is unavailable.")?;
        let gate = target.operation_gate();
        let _guard = gate.read().await;
        let lease = target
            .resource_ssh_lease(ssh)
            .await
            .map_err(|_| "Cannot connect to the source machine.")?;
        let transport = if let Some(lease) = &lease {
            lease
                .tmux_target()
                .map_err(|_| "The source connection changed; retry.")?
        } else {
            target.configured_target().clone()
        };
        if let Some(lease) = &lease {
            ssh.validate(lease)
                .await
                .map_err(|_| "The source connection changed; retry.")?;
        }
        // Resolve .. on the source filesystem; lexical collapse is wrong when
        // a preceding component is a symlink. Reuse target directory facts.
        let mut candidates = candidates.to_vec();
        for reference in &mut candidates {
            if has_parent_component(&reference.path) {
                let path = std::path::Path::new(&reference.path);
                let directory = path
                    .parent()
                    .and_then(|p| p.to_str())
                    .ok_or("Invalid source directory.")?;
                let name = path
                    .file_name()
                    .and_then(|p| p.to_str())
                    .ok_or("Invalid source file name.")?;
                let facts = super::super::target_directory::read(
                    &transport,
                    target_id,
                    Some(directory),
                    false,
                )
                .await
                .map_err(|_| "Cannot resolve the source directory.")?;
                reference.path = format!("{}/{name}", facts.path.trim_end_matches('/'));
            }
        }
        let mut script = String::from("set --");
        for reference in &candidates {
            script.push(' ');
            script.push_str(&quote(&reference.path));
        }
        script.push_str("\ni=0; for p do if [ -e \"$p\" ]; then if [ -f \"$p\" ] && [ -r \"$p\" ]; then printf '%s\\n' \"$i\"; elif [ -f \"$p\" ]; then exit 3; fi; elif [ ! -x \"$(dirname -- \"$p\")\" ] && [ -d \"$(dirname -- \"$p\")\" ]; then exit 3; fi; i=$((i+1)); done");
        let bytes = super::super::target_directory::target_output(&transport, &script)
            .await
            .map_err(|_| {
                "Cannot check source files; the connection or file permissions are unavailable."
            })?;
        if let Some(lease) = &lease {
            ssh.validate(lease)
                .await
                .map_err(|_| "The source connection changed; retry.")?;
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| "Invalid source file check response.")?;
        text.lines()
            .map(|line| {
                line.parse::<usize>()
                    .ok()
                    .and_then(|i| candidates.get(i))
                    .cloned()
                    .ok_or("Invalid source file check response.")
            })
            .collect()
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn has_parent_component(path: &str) -> bool {
    std::path::Path::new(path)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
}


