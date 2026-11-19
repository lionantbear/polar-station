//! Content provenance only. Native Workspace structure never enters this service.
use super::*;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

impl ControlCenterService {
    pub(super) async fn export_terminal_sources(
        &self,
        bindings: &[PaneContentBindingV1],
        additional_targets: &[String],
    ) -> Result<HostTransferResponseV1> {
        let mut ids = bindings
            .iter()
            .map(|b| b.execution_target.to_string())
            .collect::<Vec<_>>();
        ids.extend_from_slice(additional_targets);
        ids.sort();
        ids.dedup();
        let targets = self.targets.clone();
        let exported = if ids.is_empty() {
            HostExportBundleV1 {
                schema: HOST_EXPORT_SCHEMA_V1.into(),
                hosts: Vec::new(),
            }
        } else {
            tokio::task::spawn_blocking(move || targets.export_hosts(&ids)).await??
        };
        let facts = self.session_facts.lock().await.facts.clone();
        let mut hosts = exported
            .hosts
            .iter()
            .map(|host| (host.source.clone(), host.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut sources = Vec::with_capacity(bindings.len());
        for binding in bindings {
            binding.validate()?;
            let mut host = exported
                .hosts
                .iter()
                .find(|host| self.targets.resolve_host_ref(&host.source).as_deref() == Some(binding.execution_target.as_str()))
                .context("Referenced Host is missing; configure it before exporting")?
                .clone();
            let source = if let Some(source) = &binding.source {
                anyhow::ensure!(
                    self.targets.resolve_host_ref(&source.host).as_deref()
                        == Some(binding.execution_target.as_str()),
                    "Source Host mapping changed; review it before exporting"
                );
                host.source = source.host.clone();
                source.clone()
            } else {
                // Retained observations are exported as claims and rechecked by
                // the normal guarded attach at destination, never trusted as live.
                let session = self
                    .lifecycle
                    .previous_attachment_binding(
                        binding.execution_target.as_str(),
                        &binding.tmux_session_id,
                    )
                    .await
                    .map(|observed| ObservedTmuxSessionV1 {
                        server_generation: format!(
                            "{:x}",
                            Sha256::digest(observed.server_generation.as_bytes())
                        ),
                        session_id: observed.session_id,
                        created_unix_seconds: observed.created_unix_seconds,
                    });
                let original_agent_id = facts
                    .iter()
                    .find(|fact| {
                        fact.identity.execution_target_id.as_str()
                            == binding.execution_target.as_str()
                            && fact.identity.tmux_session_id.as_str() == binding.tmux_session_id
                    })
                    .and_then(|fact| fact.original_agent_id.clone());
                TerminalSourceV1 {
                    host: host.source.clone(),
                    session,
                    original_agent_id,
                }
            };
            source.validate()?;
            hosts.insert(host.source.clone(), host);
            sources.push(source);
        }
        Ok(HostTransferResponseV1::ContentExported {
            bundle: HostExportBundleV1 {
                schema: HOST_EXPORT_SCHEMA_V1.into(),
                hosts: hosts.into_values().collect(),
            },
            sources,
        })
    }
}
