//! Filesystem validation uses the original Host target and its verified transport.
use super::*;
use polar_tmux_resources::{resolve_reference_path, FileAccess, FileOrigin, FileReference};
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
        let mut paths = BTreeMap::new();
        for mut reference in candidates {
            if let Ok(path) = resolve_reference_path(&reference, origin.cwd.as_deref()) {
                reference.path = path.to_string_lossy().into_owned();
                paths.entry(reference.path.clone()).or_insert(reference);
            }
        }
        let refs: Vec<FileReference> = paths.into_values().collect();
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
        let mut response = Box::pin(self.dispatch(ResourcesRequestEnvelopeV1 {
            command: ResourcesCommandV1::ResolveFile {
                execution_target_id: origin.execution_target_id,
                path: reference.path.clone(),
                cwd: None,
                source_session: None,
                client_supports_local_file: true,
            },
            ..request
        }))
        .await;
        if let ResourcesResultV1::FileAccess { access } = &mut response.result {
            *access = FileAccess::Verified {
                reference,
                access: Box::new(access.clone()),
            };
        }
        response
    }
    async fn existing_candidate_files(
        &self,
        target_id: &str,
        candidates: &[FileReference],
    ) -> Result<Vec<FileReference>, &'static str> {
        if target_id == crate::target_registry::LOCAL_TARGET_ID {
            let candidates = candidates.to_vec();
            return tokio::task::spawn_blocking(move || {
                let mut existing = Vec::new();
                for reference in candidates {
                    match std::fs::metadata(&reference.path) {
                        Ok(metadata) if metadata.is_file() => {
                            std::fs::File::open(&reference.path)
                                .map_err(|_| "Permission denied while checking a source file.")?;
                            existing.push(reference);
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
        let mut script = String::from("set --");
        for reference in candidates {
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


